//! The bytes two devices exchange over a connection (docs/DEVICE_SYNC.md §6).
//!
//! Every message is a frame: a `u32` big-endian length, then that many bytes of CBOR. A connection
//! opens with one [`Hello`] each way on its first stream, so a peer speaking another protocol
//! version is told ours and turned away before anything else is said. After that each request
//! travels on its own bidirectional stream: one [`Request`] frame from the asking side, one
//! [`Response`] frame back, then the stream ends. A broken or oversized request costs only its own
//! stream.
//!
//! Everything read from the network is bounded before it is buffered: a frame header announcing
//! more than the caller's limit is refused without reading further, requests are small
//! ([`MAX_REQUEST_BYTES`]), a response is at most [`MAX_FRAME_BYTES`] (a full chunk plus room), and
//! after decoding every length is checked against [`MAX_PAGE_ENTRIES`], [`MAX_CHUNK_BYTES`] and
//! [`MAX_KEY_BYTES`]. Hashes, chunk bytes and chunk lists travel as CBOR byte strings; chunk lists
//! use their compact wire form from `wasl-chunk`.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use wasl_chunk::{ChunkList, Hash};

use crate::group::{GroupOp, MAX_OPS, OpId};
use crate::peer::{
    IndexPage, MAX_CHUNK_BYTES, MAX_GROUP_HEADS, MAX_OFFER_NAME_BYTES, MAX_OFFERS, MAX_PAGE_ENTRIES,
    MAX_PAIR_MESSAGES, MAX_PAIR_NAME_BYTES, Offer, PairReply, PairRound, PeerError, ShareId,
};

/// The protocol version this build speaks.
pub const PROTOCOL_VERSION: u8 = 1;
/// Largest frame body read from a peer: a full chunk and its envelope.
pub const MAX_FRAME_BYTES: usize = 17 << 20;
/// Largest request frame body a device accepts.
pub const MAX_REQUEST_BYTES: usize = 16 << 10;
/// Longest file key (bytes of UTF-8) in a request or an index page.
pub const MAX_KEY_BYTES: usize = 4096;

/// Why bytes from a peer are not a protocol message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ProtoError {
    #[error("frame of {len} bytes is larger than the limit of {max}")]
    TooLarge { len: u64, max: usize },
    #[error("the stream ended inside a frame")]
    Truncated,
    #[error("the stream ended before a frame")]
    Closed,
    #[error("malformed message: {0}")]
    Malformed(String),
    #[error("message breaks a limit: {0}")]
    Limit(String),
    #[error("i/o: {0}")]
    Io(String),
}

impl From<ProtoError> for PeerError {
    fn from(e: ProtoError) -> Self {
        match e {
            ProtoError::Io(m) => PeerError::Connection(m),
            other => PeerError::Protocol(other.to_string()),
        }
    }
}

/// The first message each way on a connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    pub version: u8,
}

/// What a device asks (one per stream). Mirrors [`crate::peer::PeerLink`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Request {
    Shared,
    ChangesSince {
        pair: ShareId,
        since: u64,
        limit: u32,
    },
    ChunkList {
        pair: ShareId,
        key: String,
        #[serde(with = "fixed_bytes")]
        blake3: Hash,
    },
    Chunk {
        pair: ShareId,
        #[serde(with = "fixed_bytes")]
        hash: Hash,
        /// The zstd level the asker accepts (0: raw bytes only). Left out when 0, so a request
        /// reads the same to a device that predates compression (which ignores the field).
        #[serde(default, skip_serializing_if = "is_zero")]
        zstd: u8,
    },
    Notify {
        pair: ShareId,
        seq: u64,
    },
    Pair(PairRound),
    /// The group operations the asker lacks, given its group's heads (at most
    /// [`MAX_GROUP_HEADS`]).
    Group {
        heads: Vec<OpId>,
    },
    /// The asker's group changed: ask it with [`Request::Group`].
    GroupChanged,
    /// The pairs the answering device offers the asker (docs/INTRODUCTIONS.md §3.3). A device
    /// that predates offers cannot decode this and answers with a protocol error, which the asker
    /// reads as no offers.
    Offers,
}

/// The answer to a [`Request`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Response {
    Shared(Vec<ShareId>),
    Page(IndexPage),
    ChunkList(#[serde(with = "chunk_list_bytes")] ChunkList),
    Chunk(#[serde(with = "byte_buf")] Vec<u8>),
    /// A chunk compressed with zstd, sent only to a device that asked for it: `len` raw bytes.
    ChunkZstd {
        len: u32,
        #[serde(with = "byte_buf")]
        data: Vec<u8>,
    },
    Notified,
    Pair(PairReply),
    /// Group operations, parents first (at most [`MAX_OPS`]).
    Group(Vec<GroupOp>),
    /// The answer to [`Request::Offers`]: at most [`MAX_OFFERS`], names of at most
    /// [`MAX_OFFER_NAME_BYTES`].
    Offers(Vec<Offer>),
    Error(PeerError),
}

impl Request {
    /// Checks the limits a decoded request must keep.
    pub fn check(&self) -> Result<(), ProtoError> {
        match self {
            Request::ChunkList { key, .. } => check_key(key),
            Request::Pair(round) => {
                if round.msgs.len() > MAX_PAIR_MESSAGES
                    || round.name.as_ref().is_some_and(|n| n.len() > MAX_PAIR_NAME_BYTES)
                {
                    return Err(ProtoError::Limit("pairing round too large".into()));
                }
                Ok(())
            }
            Request::Group { heads } if heads.len() > MAX_GROUP_HEADS => {
                Err(ProtoError::Limit(format!("{} group heads", heads.len())))
            }
            _ => Ok(()),
        }
    }
}

impl Response {
    /// Checks the limits a decoded response must keep.
    pub fn check(&self) -> Result<(), ProtoError> {
        match self {
            Response::Page(page) => {
                if page.entries.len() > MAX_PAGE_ENTRIES {
                    return Err(ProtoError::Limit(format!(
                        "{} index entries in one page",
                        page.entries.len()
                    )));
                }
                page.entries.iter().try_for_each(|(key, _)| check_key(key))
            }
            Response::Pair(reply)
                if reply.msgs.len() > MAX_PAIR_MESSAGES || reply.ops.len() > crate::group::MAX_OPS =>
            {
                Err(ProtoError::Limit("pairing reply too large".into()))
            }
            Response::Group(ops) if ops.len() > MAX_OPS => {
                Err(ProtoError::Limit(format!("{} group operations", ops.len())))
            }
            Response::Offers(offers) if offers.len() > MAX_OFFERS => {
                Err(ProtoError::Limit(format!("{} offers", offers.len())))
            }
            Response::Offers(offers) if offers.iter().any(|o| o.name.len() > MAX_OFFER_NAME_BYTES) => {
                Err(ProtoError::Limit("offer name too long".into()))
            }
            Response::Chunk(bytes) if bytes.len() > MAX_CHUNK_BYTES => {
                Err(ProtoError::Limit(format!("chunk of {} bytes", bytes.len())))
            }
            Response::ChunkZstd { len, data }
                if *len as usize > MAX_CHUNK_BYTES || data.len() > MAX_CHUNK_BYTES =>
            {
                Err(ProtoError::Limit(format!(
                    "compressed chunk of {} bytes for {len}",
                    data.len()
                )))
            }
            Response::Error(PeerError::Connection(m) | PeerError::Protocol(m) | PeerError::Remote(m))
                if m.len() > MAX_ERROR_BYTES =>
            {
                Err(ProtoError::Limit(format!("error text of {} bytes", m.len())))
            }
            _ => Ok(()),
        }
    }
}

fn is_zero(v: &u8) -> bool {
    *v == 0
}

/// Longest error text a peer may send.
pub const MAX_ERROR_BYTES: usize = 1024;

fn check_key(key: &str) -> Result<(), ProtoError> {
    if key.len() > MAX_KEY_BYTES {
        return Err(ProtoError::Limit(format!("key of {} bytes", key.len())));
    }
    Ok(())
}

/// The frame for `msg`: length prefix and CBOR body.
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    let mut out = vec![0u8; 4];
    ciborium::into_writer(msg, &mut out).expect("protocol messages always serialize to memory");
    let len = u32::try_from(out.len() - 4).expect("protocol messages are below 4 GiB");
    out[..4].copy_from_slice(&len.to_be_bytes());
    out
}

/// Decodes one frame body. Trailing bytes are an error.
pub fn decode<T: DeserializeOwned>(body: &[u8]) -> Result<T, ProtoError> {
    let mut rest = body;
    let msg = ciborium::from_reader(&mut rest).map_err(|e| ProtoError::Malformed(e.to_string()))?;
    if !rest.is_empty() {
        return Err(ProtoError::Malformed(format!(
            "{} bytes after the message",
            rest.len()
        )));
    }
    Ok(msg)
}

/// Decodes and checks a request frame body.
pub fn decode_request(body: &[u8]) -> Result<Request, ProtoError> {
    let req: Request = decode(body)?;
    req.check()?;
    Ok(req)
}

/// Decodes and checks a response frame body.
pub fn decode_response(body: &[u8]) -> Result<Response, ProtoError> {
    let resp: Response = decode(body)?;
    resp.check()?;
    Ok(resp)
}

/// Reads one frame body of at most `max` bytes. A longer frame is refused from its header alone;
/// the body buffer grows only as bytes arrive, so a peer that announces a large frame and stalls
/// holds no memory for it.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R, max: usize) -> Result<Vec<u8>, ProtoError> {
    let mut header = [0u8; 4];
    let mut got = 0;
    while got < header.len() {
        let n = r.read(&mut header[got..]).await.map_err(io_error)?;
        if n == 0 {
            return Err(if got == 0 {
                ProtoError::Closed
            } else {
                ProtoError::Truncated
            });
        }
        got += n;
    }
    let len = u32::from_be_bytes(header);
    if len as usize > max {
        return Err(ProtoError::TooLarge {
            len: u64::from(len),
            max,
        });
    }
    let mut body = Vec::new();
    r.take(u64::from(len))
        .read_to_end(&mut body)
        .await
        .map_err(io_error)?;
    if body.len() != len as usize {
        return Err(ProtoError::Truncated);
    }
    Ok(body)
}

/// Writes one encoded frame (from [`encode`]).
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, frame: &[u8]) -> Result<(), ProtoError> {
    w.write_all(frame).await.map_err(io_error)
}

fn io_error(e: std::io::Error) -> ProtoError {
    ProtoError::Io(e.to_string())
}

/// `[u8; N]` as a CBOR byte string.
mod fixed_bytes {
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer, const N: usize>(b: &[u8; N], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(b)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, const N: usize>(d: D) -> Result<[u8; N], D::Error> {
        let v = super::byte_buf::deserialize(d)?;
        v.try_into()
            .map_err(|v: Vec<u8>| serde::de::Error::invalid_length(v.len(), &format!("{N} bytes").as_str()))
    }
}

/// `Vec<u8>` as a CBOR byte string (serde would otherwise write an array of integers).
mod byte_buf {
    use std::fmt;

    use serde::de::Visitor;
    use serde::{Deserializer, Serializer};

    pub fn serialize<S: Serializer>(b: &[u8], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(b)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<u8>, D::Error> {
        struct Bytes;
        impl Visitor<'_> for Bytes {
            type Value = Vec<u8>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("a byte string")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Vec<u8>, E> {
                Ok(v.to_vec())
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Vec<u8>, E> {
                Ok(v)
            }
        }
        d.deserialize_byte_buf(Bytes)
    }
}

/// A [`ChunkList`] in its compact wire form, as a byte string; decoding checks it.
mod chunk_list_bytes {
    use serde::{Deserializer, Serializer};
    use wasl_chunk::ChunkList;

    pub fn serialize<S: Serializer>(list: &ChunkList, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&list.to_bytes())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<ChunkList, D::Error> {
        let bytes = super::byte_buf::deserialize(d)?;
        ChunkList::from_bytes(&bytes).map_err(serde::de::Error::custom)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pairing::Message as PairMessage;
    use proptest::prelude::*;
    use wasl_chunk::Chunk;
    use wasl_core::nway::Version;
    use wasl_core::vv::VersionVector;

    fn share_id() -> impl Strategy<Value = ShareId> {
        any::<[u8; 16]>().prop_map(ShareId)
    }

    fn key() -> impl Strategy<Value = String> {
        proptest::collection::vec(any::<char>(), 0..40).prop_map(|c| c.into_iter().collect())
    }

    fn version() -> impl Strategy<Value = Version> {
        (
            any::<bool>(),
            any::<bool>(),
            proptest::option::of(any::<[u8; 32]>()),
            any::<u64>(),
            any::<i64>(),
            proptest::collection::vec((any::<u64>(), 1..5u8), 0..4),
            any::<u64>(),
        )
            .prop_map(|(is_dir, deleted, hash, size, mtime, ticks, author)| {
                let mut vv = VersionVector::new();
                for (id, n) in ticks {
                    for _ in 0..n {
                        vv.tick(id);
                    }
                }
                Version {
                    is_dir,
                    deleted,
                    hash: hash.map(|h| h.iter().map(|b| format!("{b:02x}")).collect()),
                    size,
                    mtime,
                    vv,
                    author,
                }
            })
    }

    fn chunk_list() -> impl Strategy<Value = ChunkList> {
        (
            any::<[u8; 32]>(),
            proptest::collection::vec((1..100_000u32, any::<[u8; 32]>()), 0..20),
        )
            .prop_map(|(blake3, parts)| {
                let mut offset = 0u64;
                let chunks: Vec<Chunk> = parts
                    .into_iter()
                    .map(|(len, hash)| {
                        let c = Chunk { offset, len, hash };
                        offset += u64::from(len);
                        c
                    })
                    .collect();
                ChunkList {
                    size: offset,
                    blake3,
                    chunks,
                }
            })
    }

    fn peer_error() -> impl Strategy<Value = PeerError> {
        prop_oneof![
            Just(PeerError::NotShared),
            Just(PeerError::Gone),
            key().prop_map(PeerError::Connection),
            key().prop_map(PeerError::Protocol),
            key().prop_map(PeerError::Remote),
        ]
    }

    fn request() -> impl Strategy<Value = Request> {
        prop_oneof![
            Just(Request::Shared),
            (share_id(), any::<u64>(), any::<u32>()).prop_map(|(pair, since, limit)| Request::ChangesSince {
                pair,
                since,
                limit
            }),
            (share_id(), key(), any::<[u8; 32]>()).prop_map(|(pair, key, blake3)| Request::ChunkList {
                pair,
                key,
                blake3
            }),
            (share_id(), any::<[u8; 32]>()).prop_map(|(pair, hash)| Request::Chunk { pair, hash, zstd: 0 }),
            (share_id(), any::<u64>()).prop_map(|(pair, seq)| Request::Notify { pair, seq }),
            (any::<[u8; 32]>(), any::<[u8; 32]>(), proptest::option::of(key())).prop_map(
                |(pk, nonce, name)| {
                    Request::Pair(PairRound {
                        msgs: vec![
                            PairMessage::Hello {
                                pk,
                                nonce,
                                kind: crate::pairing::CodeKind::Qr,
                            },
                            PairMessage::Proof { mac: nonce },
                            PairMessage::Confirm,
                            PairMessage::Abort,
                        ],
                        name,
                    })
                }
            ),
        ]
    }

    fn response() -> impl Strategy<Value = Response> {
        prop_oneof![
            proptest::collection::vec(share_id(), 0..10).prop_map(Response::Shared),
            (
                proptest::collection::vec((key(), version()), 0..10),
                any::<u64>(),
                any::<bool>()
            )
                .prop_map(|(entries, next_since, more)| Response::Page(IndexPage {
                    entries,
                    next_since,
                    more
                })),
            chunk_list().prop_map(Response::ChunkList),
            proptest::collection::vec(any::<u8>(), 0..2000).prop_map(Response::Chunk),
            Just(Response::Notified),
            Just(Response::Pair(PairReply {
                msgs: vec![PairMessage::Confirm],
                ops: Vec::new(),
            })),
            peer_error().prop_map(Response::Error),
        ]
    }

    fn body(frame: &[u8]) -> &[u8] {
        let len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
        assert_eq!(len, frame.len() - 4);
        &frame[4..]
    }

    proptest! {
        #[test]
        fn requests_round_trip(req in request()) {
            prop_assert_eq!(decode_request(body(&encode(&req))).unwrap(), req);
        }

        #[test]
        fn responses_round_trip(resp in response()) {
            prop_assert_eq!(decode_response(body(&encode(&resp))).unwrap(), resp);
        }

        #[test]
        fn hello_round_trips(version in any::<u8>()) {
            let hello = Hello { version };
            prop_assert_eq!(decode::<Hello>(body(&encode(&hello))).unwrap(), hello);
        }

        #[test]
        fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..300)) {
            let _ = decode_request(&bytes);
            let _ = decode_response(&bytes);
            let _ = decode::<Hello>(&bytes);
        }
    }

    #[test]
    fn group_requests_round_trip_and_are_bounded() {
        let key = crate::identity::DeviceKey::from_seed(&[4; 32]);
        let (g, _) = crate::group::GroupOps::create(&key, "A", 1).unwrap();
        let ask = Request::Group { heads: g.heads() };
        assert_eq!(decode_request(body(&encode(&ask))).unwrap(), ask);
        let ops: Vec<GroupOp> = g.ops().into_iter().cloned().collect();
        let answer = Response::Group(ops);
        assert_eq!(decode_response(body(&encode(&answer))).unwrap(), answer);
        assert_eq!(
            decode_request(body(&encode(&Request::GroupChanged))).unwrap(),
            Request::GroupChanged
        );

        let many = Request::Group {
            heads: vec![OpId([1; 32]); MAX_GROUP_HEADS + 1],
        };
        let frame = encode(&many);
        assert!(
            frame.len() - 4 <= MAX_REQUEST_BYTES,
            "the largest ask fits a request"
        );
        assert!(matches!(decode_request(body(&frame)), Err(ProtoError::Limit(_))));
        let op = g.ops()[0].clone();
        let flood = Response::Group(vec![op; MAX_OPS + 1]);
        assert!(matches!(
            decode_response(body(&encode(&flood))),
            Err(ProtoError::Limit(_))
        ));
    }

    #[test]
    fn offers_round_trip_and_are_bounded() {
        assert_eq!(
            decode_request(body(&encode(&Request::Offers))).unwrap(),
            Request::Offers
        );
        let offer = |i: u8, name: &str| Offer {
            share: ShareId([i; 16]),
            name: name.into(),
            files: 1_240,
            bytes: 2_100_000_000,
        };
        let answer = Response::Offers(vec![offer(1, "Projects"), offer(2, "صور العائلة")]);
        assert_eq!(decode_response(body(&encode(&answer))).unwrap(), answer);
        let full = Response::Offers(
            (0..MAX_OFFERS as u8)
                .map(|i| offer(i, &"n".repeat(MAX_OFFER_NAME_BYTES)))
                .collect(),
        );
        assert_eq!(decode_response(body(&encode(&full))).unwrap(), full);

        let too_many = Response::Offers((0..=MAX_OFFERS as u8).map(|i| offer(i, "x")).collect());
        assert!(matches!(
            decode_response(body(&encode(&too_many))),
            Err(ProtoError::Limit(_))
        ));
        let long_name = Response::Offers(vec![offer(1, &"n".repeat(MAX_OFFER_NAME_BYTES + 1))]);
        assert!(matches!(
            decode_response(body(&encode(&long_name))),
            Err(ProtoError::Limit(_))
        ));
    }

    #[test]
    fn hashes_and_chunks_travel_as_byte_strings() {
        let frame = encode(&Request::Chunk {
            zstd: 0,
            pair: ShareId([1; 16]),
            hash: [7; 32],
        });
        // An array of 32 small integers would take 32+ bytes for the hash alone; a byte string
        // takes 34. The whole request stays small.
        assert!(frame.len() < 80, "{} bytes", frame.len());
        let chunk = encode(&Response::Chunk(vec![0xAB; 1000]));
        assert!(chunk.len() < 1020, "{} bytes", chunk.len());
    }

    #[test]
    fn trailing_bytes_are_refused() {
        let mut frame = encode(&Request::Shared);
        frame.push(0);
        assert!(matches!(
            decode_request(&frame[4..]),
            Err(ProtoError::Malformed(_))
        ));
    }

    #[test]
    fn limits_are_checked_after_decoding() {
        let long_key = Request::ChunkList {
            pair: ShareId([0; 16]),
            key: "k".repeat(MAX_KEY_BYTES + 1),
            blake3: [0; 32],
        };
        assert!(matches!(
            decode_request(body(&encode(&long_key))),
            Err(ProtoError::Limit(_))
        ));

        let v = Version {
            is_dir: true,
            deleted: false,
            hash: None,
            size: 0,
            mtime: 0,
            vv: VersionVector::new(),
            author: 1,
        };
        let big_page = Response::Page(IndexPage {
            entries: vec![("a".into(), v); MAX_PAGE_ENTRIES + 1],
            next_since: 0,
            more: false,
        });
        assert!(matches!(
            decode_response(body(&encode(&big_page))),
            Err(ProtoError::Limit(_))
        ));

        let big_chunk = Response::Chunk(vec![0; MAX_CHUNK_BYTES + 1]);
        assert!(matches!(
            decode_response(body(&encode(&big_chunk))),
            Err(ProtoError::Limit(_))
        ));
    }

    #[test]
    fn a_chunk_list_that_does_not_cover_its_file_is_malformed() {
        let bad = Response::ChunkList(ChunkList {
            size: 20,
            blake3: [0; 32],
            chunks: vec![Chunk {
                offset: 0,
                len: 10,
                hash: [1; 32],
            }],
        });
        assert!(matches!(
            decode_response(body(&encode(&bad))),
            Err(ProtoError::Malformed(_))
        ));
    }

    /// A reader that yields a fixed prefix, then fails: reading past it would be a bug.
    struct Exactly(Vec<u8>);

    impl AsyncRead for Exactly {
        fn poll_read(
            mut self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            buf: &mut tokio::io::ReadBuf<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            assert!(!self.0.is_empty(), "read past the frame header");
            let n = buf.remaining().min(self.0.len());
            buf.put_slice(&self.0[..n]);
            self.0.drain(..n);
            std::task::Poll::Ready(Ok(()))
        }
    }

    #[tokio::test]
    async fn a_huge_length_is_refused_from_the_header() {
        let mut r = Exactly(u32::MAX.to_be_bytes().to_vec());
        assert_eq!(
            read_frame(&mut r, MAX_FRAME_BYTES).await,
            Err(ProtoError::TooLarge {
                len: u64::from(u32::MAX),
                max: MAX_FRAME_BYTES
            })
        );
        let mut r = Exactly(((MAX_REQUEST_BYTES + 1) as u32).to_be_bytes().to_vec());
        assert!(matches!(
            read_frame(&mut r, MAX_REQUEST_BYTES).await,
            Err(ProtoError::TooLarge { .. })
        ));
    }

    #[tokio::test]
    async fn short_streams_are_reported() {
        let mut empty: &[u8] = &[];
        assert_eq!(read_frame(&mut empty, 100).await, Err(ProtoError::Closed));
        let mut half: &[u8] = &[0, 0];
        assert_eq!(read_frame(&mut half, 100).await, Err(ProtoError::Truncated));
        let mut short: &[u8] = &[0, 0, 0, 5, 1, 2];
        assert_eq!(read_frame(&mut short, 100).await, Err(ProtoError::Truncated));
        let frame = encode(&Request::Shared);
        let mut whole: &[u8] = &frame;
        assert_eq!(read_frame(&mut whole, 100).await.unwrap(), frame[4..].to_vec());
    }

    /// A device from before compression, as far as chunks go: it knows `Chunk` without `zstd`.
    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    enum OldRequest {
        Chunk {
            pair: ShareId,
            #[serde(with = "fixed_bytes")]
            hash: Hash,
        },
    }

    #[test]
    fn compression_is_invisible_to_a_device_that_predates_it() {
        let (pair, hash) = (ShareId([3; 16]), [9u8; 32]);
        // An older device's request reads as "raw bytes only".
        let old = encode(&OldRequest::Chunk { pair, hash });
        assert_eq!(
            Request::Chunk { pair, hash, zstd: 0 },
            decode_request(&old[4..]).unwrap()
        );
        // A raw request is byte for byte what an older device sends.
        assert_eq!(old, encode(&Request::Chunk { pair, hash, zstd: 0 }));
        // An older device answering a request that asks for compression ignores the wish.
        let new = encode(&Request::Chunk { pair, hash, zstd: 3 });
        assert_eq!(
            OldRequest::Chunk { pair, hash },
            decode::<OldRequest>(&new[4..]).unwrap()
        );
    }

    #[test]
    fn a_compressed_chunk_round_trips_and_is_bounded() {
        let resp = Response::ChunkZstd {
            len: 70_000,
            data: vec![7; 1000],
        };
        assert_eq!(resp, decode_response(&encode(&resp)[4..]).unwrap());
        let claims_too_much = Response::ChunkZstd {
            len: MAX_CHUNK_BYTES as u32 + 1,
            data: vec![7; 10],
        };
        assert!(matches!(
            decode_response(&encode(&claims_too_much)[4..]),
            Err(ProtoError::Limit(_))
        ));
    }
}
