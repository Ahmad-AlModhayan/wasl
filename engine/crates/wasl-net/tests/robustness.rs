//! Robustness of everything `wasl-net` reads from outside (docs/SECURITY.md S5-2): protocol frames
//! and messages from peers, group operations and the persisted group file, rendezvous records and
//! offers from the Drive app-data folder, pairing codes and messages, STUN answers, LAN probes and
//! typed codes. Each parser gets arbitrary bytes and mutations of real valid encodings and must
//! answer with a value or an error: no panic, in bounded time, with output bounded by its input,
//! and never accepting a tampered signed value as genuine.
//!
//! Coverage-guided fuzzing of the same parsers: `engine/fuzz` (cargo-fuzz, nightly, Linux/macOS).

#[path = "../../../robustness/mutate.rs"]
mod mutate;

use std::net::SocketAddr;
use std::time::Duration;

use mutate::{deep_cbor, deep_json, guard, mutate, mutate_text};
use proptest::prelude::*;
use wasl_chunk::{ChunkList, ChunkParams, chunk_reader};
use wasl_core::nway::Version;
use wasl_core::vv::VersionVector;
use wasl_net::discover::{Packet, Token};
use wasl_net::group::canonical_json;
use wasl_net::proto::{
    self, Hello, MAX_FRAME_BYTES, MAX_REQUEST_BYTES, ProtoError, Request, Response, decode_request,
    decode_response,
};
use wasl_net::rendezvous::{Addrs, Offer, PeerRecord};
use wasl_net::{
    CodeKind, DeviceId, DeviceKey, GroupOp, GroupOps, IndexPage, Invitation, JoinCode, Message, OpId,
    PairReply, PairRound, PairingCode, PeerError, Session, ShareId, ShortCode, State, stun,
};

const CASES: u32 = 512;
const NOW: u64 = 1_700_000_000;

fn key(n: u8) -> DeviceKey {
    DeviceKey::from_seed(&[n; 32])
}

/// A group of three devices with one removed: create, two adds, a remove.
fn group() -> (GroupOps, Vec<GroupOp>) {
    let (a, b, c) = (key(1), key(2), key(3));
    let (mut g, create) = GroupOps::create(&a, "Laptop", NOW).unwrap();
    let add_b = g.add_device(b.public(), "Phone", NOW + 1, &a).unwrap();
    g.insert(add_b.clone()).unwrap();
    let add_c = g.add_device(c.public(), "Tablet", NOW + 2, &b).unwrap();
    g.insert(add_c.clone()).unwrap();
    let remove_c = g.remove_device(c.id(), &a).unwrap();
    g.insert(remove_c.clone()).unwrap();
    (g, vec![create, add_b, add_c, remove_c])
}

fn chunk_list() -> ChunkList {
    let data: Vec<u8> = (0..40_000u32).map(|i| (i * 7 % 251) as u8).collect();
    chunk_reader(&data[..], &ChunkParams::new(1024, 4096, 16384).unwrap()).unwrap()
}

fn version() -> Version {
    Version {
        is_dir: false,
        deleted: false,
        hash: Some("ab".repeat(32)),
        size: 40_000,
        mtime: 1_700_000_000_123,
        vv: VersionVector::new().ticked(7).ticked(9),
        author: 9,
    }
}

fn pair_messages() -> Vec<Message> {
    vec![
        Message::Hello {
            pk: [3; 32],
            nonce: [4; 32],
            kind: CodeKind::Short,
        },
        Message::Proof { mac: [5; 32] },
        Message::Confirm,
        Message::Abort,
    ]
}

/// Frame bodies (no length prefix) of every request.
fn requests() -> Vec<Vec<u8>> {
    let pair = ShareId([7; 16]);
    [
        Request::Shared,
        Request::ChangesSince {
            pair,
            since: 41,
            limit: 1000,
        },
        Request::ChunkList {
            pair,
            key: "Docs/report final.pdf".into(),
            blake3: [8; 32],
        },
        Request::Chunk {
            pair,
            hash: [9; 32],
            zstd: 0,
        },
        Request::Notify { pair, seq: u64::MAX },
        Request::Pair(PairRound {
            msgs: pair_messages(),
            name: Some("A laptop".into()),
        }),
        Request::Group {
            heads: vec![OpId([1; 32]), OpId([2; 32])],
        },
        Request::GroupChanged,
    ]
    .iter()
    .map(|r| proto::encode(r)[4..].to_vec())
    .collect()
}

/// Frame bodies of every response.
fn responses() -> Vec<Vec<u8>> {
    let (_, ops) = group();
    [
        Response::Shared(vec![ShareId([1; 16]), ShareId([2; 16])]),
        Response::Page(IndexPage {
            entries: vec![("a/b.txt".into(), version()), ("c".into(), version())],
            next_since: 99,
            more: true,
        }),
        Response::ChunkList(chunk_list()),
        Response::Chunk((0..300u16).map(|i| i as u8).collect()),
        Response::Notified,
        Response::Pair(PairReply {
            msgs: pair_messages(),
            ops: ops.clone(),
        }),
        Response::Group(ops),
        Response::Error(PeerError::Remote("disk full".into())),
        Response::Error(PeerError::NotShared),
    ]
    .iter()
    .map(|r| proto::encode(r)[4..].to_vec())
    .collect()
}

/// A decoded message re-encodes to roughly its input size: decoding cannot amplify memory.
fn assert_not_amplified<T: serde::Serialize>(msg: &T, input: usize) {
    let out = proto::encode(msg).len();
    assert!(
        out <= 4 * input + 64,
        "{input} input bytes decoded to a {out}-byte message"
    );
}

fn check_request(body: Vec<u8>) {
    let len = body.len();
    let r = guard("decode_request", move || decode_request(&body));
    if let Ok(req) = r {
        req.check().expect("a decoded request keeps its limits");
        assert_not_amplified(&req, len);
        let again = proto::encode(&req);
        assert_eq!(decode_request(&again[4..]).as_ref(), Ok(&req), "round trip");
    }
}

fn check_response(body: Vec<u8>) {
    let len = body.len();
    let r = guard("decode_response", move || decode_response(&body));
    if let Ok(resp) = r {
        resp.check().expect("a decoded response keeps its limits");
        assert_not_amplified(&resp, len);
        if let Response::ChunkList(list) = &resp {
            list.check()
                .expect("a chunk list from a peer is checked on decode");
        }
        let again = proto::encode(&resp);
        assert_eq!(decode_response(&again[4..]).as_ref(), Ok(&resp), "round trip");
    }
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread().build().unwrap()
}

/// Every key ever added to the group (trusted or removed).
fn known_keys(g: &GroupOps) -> Vec<[u8; 32]> {
    g.ops()
        .into_iter()
        .filter_map(|o| match &o.change {
            wasl_net::Change::Create(d) | wasl_net::Change::Add(d) => Some(d.pk),
            _ => None,
        })
        .collect()
}

fn lan_addrs() -> Addrs {
    Addrs {
        lan: vec!["192.168.1.20:41000".parse().unwrap()],
        ipv6: vec!["[2a00:1450:4001:81c::200e]:41000".parse().unwrap()],
        stun: Some("93.184.216.34:41000".parse().unwrap()),
        mapped: None,
    }
}

fn trust_1_and_2(pk: &[u8; 32]) -> bool {
    *pk == key(1).public() || *pk == key(2).public()
}

fn qr_code() -> PairingCode {
    PairingCode {
        device_id: key(1).id(),
        secret: [0xA5; 20],
        addrs: vec![
            "192.168.1.20:22000".parse().unwrap(),
            "[2a00:1450:4001:81c::200e]:22001".parse().unwrap(),
        ],
    }
}

fn stun_packets() -> Vec<Vec<u8>> {
    let tx = [0x42; 12];
    let v4: SocketAddr = "93.184.216.34:41000".parse().unwrap();
    let v6: SocketAddr = "[2a00:1450:4001:81c::200e]:41000".parse().unwrap();
    vec![
        stun::encode_request(&tx),
        stun::encode_success(&tx, v4),
        stun::encode_success(&tx, v6),
        stun::encode_error(&tx, 487),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(CASES))]

    // ---- Protocol frames (peer → device, the most exposed parser) ----

    #[test]
    fn mutated_requests_decode_or_fail_cleanly(which in 0usize..8, seed in any::<u64>()) {
        check_request(mutate(&requests()[which], seed));
    }

    #[test]
    fn mutated_responses_decode_or_fail_cleanly(which in 0usize..9, seed in any::<u64>()) {
        check_response(mutate(&responses()[which], seed));
    }

    #[test]
    fn random_bytes_are_not_messages(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        check_request(bytes.clone());
        check_response(bytes.clone());
        let _ = guard("decode hello", move || proto::decode::<Hello>(&bytes));
    }

    #[test]
    fn frames_are_read_within_their_limit(
        stream in proptest::collection::vec(any::<u8>(), 0..256),
        header in any::<u32>(),
        use_header in any::<bool>(),
    ) {
        let mut input = Vec::new();
        if use_header {
            input.extend_from_slice(&(header % 512).to_be_bytes());
        }
        input.extend_from_slice(&stream);
        let len = input.len();
        let r = guard("read_frame", move || {
            runtime().block_on(proto::read_frame(&mut &input[..], 256))
        });
        match r {
            Ok(body) => prop_assert!(body.len() <= 256 && body.len() + 4 <= len),
            Err(ProtoError::TooLarge { len, max }) => prop_assert!(len > max as u64),
            Err(ProtoError::Closed | ProtoError::Truncated) => {}
            Err(other) => prop_assert!(false, "unexpected {other:?}"),
        }
    }

    // ---- Group operations and the group file ----

    #[test]
    fn mutated_operations_never_change_trust_unless_genuine(which in 0usize..4, seed in any::<u64>()) {
        let (g, ops) = group();
        let original = ops[which].clone();
        let bytes = mutate(&original.to_json(), seed);
        let parsed = guard("GroupOp::from_json", move || GroupOp::from_json(&bytes));
        if let Ok(op) = parsed {
            // Canonical form is stable.
            prop_assert_eq!(GroupOp::from_json(&op.to_json()).ok(), Some(op.clone()));
            let mut g2 = g.clone();
            if g2.insert(op.clone()).is_ok() && g2.state() != g.state() {
                prop_assert_eq!(op, original);
            }
        }
    }

    #[test]
    fn a_mutated_group_file_loads_only_genuine_devices(seed in any::<u64>()) {
        let (g, _) = group();
        let genesis = g.genesis();
        let known = known_keys(&g);
        let bytes = mutate(&g.to_json(), seed);
        let loaded = guard("GroupOps::from_json", move || GroupOps::from_json(&bytes, genesis));
        if let Ok(g2) = loaded {
            for d in g2.state().devices() {
                prop_assert!(known.contains(&d.pk), "a device nobody signed for: {:?}", d.id);
            }
            let again = GroupOps::from_json(&g2.to_json(), genesis).unwrap();
            prop_assert_eq!(again.state(), g2.state());
        }
    }

    #[test]
    fn bootstrap_from_mutated_operations_is_safe(seed in any::<u64>()) {
        let (g, _) = group();
        let known = known_keys(&g);
        let bytes = mutate(&g.to_json(), seed);
        let peer = key(2).public();
        let r = guard("GroupOps::bootstrap", move || {
            let ops: Vec<GroupOp> = serde_json::from_slice(&bytes).ok()?;
            GroupOps::bootstrap(&ops, &peer).ok()
        });
        if let Some(g2) = r {
            for d in g2.state().devices() {
                prop_assert!(known.contains(&d.pk));
            }
        }
    }

    #[test]
    fn canonical_json_is_stable_on_any_json(seed in any::<u64>()) {
        let (_, ops) = group();
        let text = mutate(&ops[1].to_json(), seed);
        let r = guard("canonical_json", move || {
            let value: serde_json::Value = serde_json::from_slice(&text).ok()?;
            let canon = canonical_json(&value);
            let back: serde_json::Value = serde_json::from_str(&canon).expect("canonical JSON parses");
            Some((value, back, canon))
        });
        if let Some((value, back, canon)) = r {
            prop_assert_eq!(&back, &value);
            prop_assert_eq!(canonical_json(&back), canon);
        }
    }

    // ---- Rendezvous records and offers (Drive app-data folder) ----

    #[test]
    fn a_mutated_peer_record_is_refused_or_identical(seed in any::<u64>()) {
        let record = PeerRecord::new(&key(1), lan_addrs(), NOW, Duration::from_secs(300)).unwrap();
        let bytes = mutate(&record.to_json(), seed);
        let expected = key(1).id();
        let r = guard("PeerRecord::verify", move || {
            PeerRecord::verify(&bytes, expected, &trust_1_and_2, NOW + 10)
        });
        if let Ok(got) = r {
            prop_assert_eq!(got, record);
        }
    }

    #[test]
    fn a_mutated_offer_is_refused_or_identical(seed in any::<u64>(), answer in any::<bool>()) {
        let offer = Offer::new(&key(1), key(2).id(), lan_addrs(), NOW * 1000 + 500, NOW, Duration::from_secs(60)).unwrap();
        let (offer, from, me) = if answer {
            let a = offer.answer(&key(2), lan_addrs(), NOW * 1000 + 900, NOW, Duration::from_secs(60)).unwrap();
            (a, key(2).id(), key(1).id())
        } else {
            (offer, key(1).id(), key(2).id())
        };
        let bytes = mutate(&offer.to_json(), seed);
        let r = guard("Offer::verify", move || Offer::verify(&bytes, from, me, &trust_1_and_2, NOW + 10));
        if let Ok(got) = r {
            prop_assert_eq!(got, offer);
        }
    }

    #[test]
    fn random_bytes_are_not_records(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        let b2 = bytes.clone();
        let record_refused = guard("PeerRecord::verify", move || {
            PeerRecord::verify(&bytes, key(1).id(), &|_| true, NOW).is_err()
        });
        let offer_refused = guard("Offer::verify", move || {
            Offer::verify(&b2, key(1).id(), key(2).id(), &|_| true, NOW).is_err()
        });
        prop_assert!(record_refused && offer_refused);
    }

    // ---- Pairing ----

    #[test]
    fn a_mutated_qr_code_is_refused_or_canonical(seed in any::<u64>()) {
        let text = mutate_text(&qr_code().to_qr(), seed);
        let t2 = text.clone();
        let r = guard("PairingCode::from_qr", move || PairingCode::from_qr(&t2));
        if let Ok(code) = r {
            prop_assert_eq!(code.to_qr(), text);
        }
    }

    #[test]
    fn typed_codes_parse_or_fail(text in "\\PC{0,64}", seed in any::<u64>()) {
        let short = ShortCode::parse("ABCD-EFGH").map(|c| c.to_string()).unwrap();
        let mutated = mutate_text(&short, seed);
        for t in [text.clone(), mutated] {
            let r = guard("ShortCode::parse", move || ShortCode::parse(&t).map(|c| c.to_string()));
            if let Ok(shown) = r {
                prop_assert_eq!(ShortCode::parse(&shown).map(|c| c.to_string()), Ok(shown.clone()));
            }
        }
        let id = key(4).id().to_string();
        for t in [text, mutate_text(&id, seed)] {
            if let Ok(parsed) = guard("DeviceId::parse", move || DeviceId::parse(&t)) {
                prop_assert_eq!(DeviceId::parse(&parsed.to_string()), Ok(parsed));
            }
        }
    }

    /// An inviter fed any sequence of messages from someone without the code never pairs.
    #[test]
    fn a_pairing_session_survives_any_messages(
        msgs in proptest::collection::vec(
            prop_oneof![
                (any::<[u8; 32]>(), any::<[u8; 32]>(), any::<bool>()).prop_map(|(pk, nonce, short)| Message::Hello {
                    pk,
                    nonce,
                    kind: if short { CodeKind::Short } else { CodeKind::Qr },
                }),
                any::<[u8; 32]>().prop_map(|mac| Message::Proof { mac }),
                Just(Message::Confirm),
                Just(Message::Abort),
            ],
            0..12,
        ),
        steps in proptest::collection::vec(0u64..700, 12),
        confirm_at in 0usize..12,
    ) {
        let reached = guard("pairing session", move || {
            let mut inv = Invitation::new(key(1).id(), Vec::new(), NOW).unwrap();
            inv.enable_short_code();
            let mut s = inv.start(key(1).public(), NOW).unwrap();
            let mut now = NOW;
            let mut states = Vec::new();
            for (i, m) in msgs.iter().enumerate() {
                now += steps[i];
                s.handle(m, now);
                if i == confirm_at {
                    s.confirm(now);
                }
                s.tick(now);
                states.push(s.state());
            }
            (states, s.paired_peer().is_some())
        });
        let (states, paired) = reached;
        prop_assert!(!paired);
        for st in states {
            prop_assert!(
                matches!(st, State::AwaitingHello | State::AwaitingProof | State::Aborted(_)),
                "reached {st:?} without the code"
            );
        }
    }

    #[test]
    fn a_joiner_survives_any_messages(
        msgs in proptest::collection::vec(
            prop_oneof![
                (any::<[u8; 32]>(), any::<[u8; 32]>()).prop_map(|(pk, nonce)| Message::Hello { pk, nonce, kind: CodeKind::Qr }),
                any::<[u8; 32]>().prop_map(|mac| Message::Proof { mac }),
                Just(Message::Confirm),
                Just(Message::Abort),
            ],
            0..12,
        ),
    ) {
        let paired = guard("joiner session", move || {
            let (mut s, _hello) = Session::join(&JoinCode::Qr(qr_code()), key(2).public(), NOW).unwrap();
            for m in &msgs {
                s.handle(m, NOW + 1);
                s.confirm(NOW + 1);
            }
            s.paired_peer().is_some()
        });
        prop_assert!(!paired);
    }

    // ---- STUN, LAN probes, share codes ----

    #[test]
    fn stun_answers_decode_or_fail(which in 0usize..4, seed in any::<u64>(), raw in proptest::collection::vec(any::<u8>(), 0..1400)) {
        for packet in [mutate(&stun_packets()[which], seed), raw] {
            let p2 = packet.clone();
            if let Ok(msg) = guard("stun::decode", move || stun::decode(&p2)) {
                prop_assert!(stun::looks_like_stun(&packet));
                prop_assert_eq!(msg.tx(), <[u8; 12]>::try_from(&packet[8..20]).unwrap());
            }
        }
    }

    #[test]
    fn probe_packets_round_trip_or_are_ignored(bytes in proptest::collection::vec(any::<u8>(), 0..64), seed in any::<u64>()) {
        let valid = Packet::Reply { token: Token([6; 16]), port: 41414 }.encode();
        for b in [bytes, mutate(&valid, seed)] {
            if let Some(p) = Packet::decode(&b) {
                prop_assert_eq!(p.encode(), b);
            }
        }
    }

    #[test]
    fn share_codes_and_tokens_never_panic(text in "\\PC{0,40}", ascii in "[0-9a-fA-F+ -]{32}") {
        for t in [text, ascii] {
            let _ = ShareId::from_hex(&t);
            let _ = Token::from_hex(&t);
        }
    }
}

/// Finding S5-2-F1 (low): `ShareId::from_hex` and `discover::Token::from_hex` use
/// `u8::from_str_radix` per byte pair, which accepts a leading `+`, so `"+1+1…"` parses as the code
/// `0101…`. Not a crash; two spellings of one share code. Enable once the parsers accept hex digits
/// only.
#[test]
fn share_codes_are_canonical() {
    let plus = "+1".repeat(16);
    assert_eq!(ShareId::from_hex(&plus), None, "ShareId accepted {plus}");
    assert_eq!(Token::from_hex(&plus), None, "Token accepted {plus}");
}

#[test]
fn share_codes_round_trip() {
    let id = ShareId([0xAB; 16]);
    assert_eq!(ShareId::from_hex(&id.to_hex()), Some(id));
    assert_eq!(ShareId::from_hex(&id.to_hex().to_uppercase()), Some(id));
    // 32 bytes but not 32 characters, and multi-byte characters at odd offsets.
    assert_eq!(ShareId::from_hex(&"é".repeat(16)), None);
    assert_eq!(ShareId::from_hex(&format!("a{}", "é".repeat(15))), None);
}

#[test]
fn deep_nesting_and_huge_lengths_fail_fast() {
    for body in [
        deep_cbor(100_000),
        // An indefinite array that never ends.
        [vec![0x9F; 50_000], vec![0x00]].concat(),
        // A request variant whose payload claims 2^64 bytes, 2^64 entries or a 4 GiB string.
        [
            vec![0xA1, 0x64],
            b"Pair".to_vec(),
            vec![0xA2, 0x64],
            b"msgs".to_vec(),
            vec![0x9B],
            vec![0xFF; 8],
        ]
        .concat(),
        [vec![0xA1, 0x65], b"Chunk".to_vec(), vec![0x5B], vec![0xFF; 8]].concat(),
        [
            vec![0xA1, 0x65],
            b"Chunk".to_vec(),
            vec![0x5A, 0xFF, 0xFF, 0xFF, 0xFF],
        ]
        .concat(),
        [
            vec![0xA1, 0x66],
            b"Shared".to_vec(),
            vec![0x9A, 0xFF, 0xFF, 0xFF, 0xFF],
        ]
        .concat(),
    ] {
        let b2 = body.clone();
        assert!(guard("deep request", move || decode_request(&b2).is_err()));
        assert!(guard("deep response", move || decode_response(&body).is_err()));
    }
    let deep = deep_json(100_000).into_bytes();
    let d2 = deep.clone();
    assert!(guard("deep op", move || GroupOp::from_json(&d2).is_err()));
    assert!(guard("deep group", move || GroupOps::from_json(
        &deep,
        OpId([0; 32])
    )
    .is_err()));
}

#[test]
fn a_frame_header_beyond_the_limit_is_refused_before_reading() {
    for (len, max) in [
        (u32::MAX, MAX_FRAME_BYTES),
        (MAX_REQUEST_BYTES as u32 + 1, MAX_REQUEST_BYTES),
        (MAX_FRAME_BYTES as u32 + 1, MAX_FRAME_BYTES),
    ] {
        let r = guard("read_frame header", move || {
            let input = len.to_be_bytes();
            runtime().block_on(proto::read_frame(&mut &input[..], max))
        });
        assert_eq!(
            r,
            Err(ProtoError::TooLarge {
                len: u64::from(len),
                max
            })
        );
    }
    // A header announcing the full limit with nothing behind it allocates nothing up front.
    let r = guard("read_frame truncated", move || {
        let input = (MAX_FRAME_BYTES as u32).to_be_bytes();
        runtime().block_on(proto::read_frame(&mut &input[..], MAX_FRAME_BYTES))
    });
    assert_eq!(r, Err(ProtoError::Truncated));
}

#[test]
fn oversized_operations_and_records_are_refused() {
    let big = vec![b' '; wasl_net::group::MAX_OP_BYTES + 1];
    assert!(GroupOp::from_json(&big).is_err());
    let mut record = PeerRecord::new(&key(1), lan_addrs(), NOW, Duration::from_secs(60))
        .unwrap()
        .to_json();
    record.extend(std::iter::repeat_n(b' ', wasl_net::rendezvous::MAX_RECORD_BYTES));
    assert!(PeerRecord::verify(&record, key(1).id(), &|_| true, NOW + 1).is_err());
}
