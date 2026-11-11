//! Robustness of the daemon's local socket (docs/SECURITY.md S5-2). Any process of any user may
//! open the pipe, so the line reader, the handshake and the frames after it must hold against
//! arbitrary and mutated input: no panic, no hang, nothing reaching the handler before the client
//! proved it knows the token, and the daemon still serving this user's real clients afterwards.
//!
//! `wasl-ipc` has no `proptest` dev-dependency, so the cases come from a fixed seed loop over the
//! shared mutator (`engine/robustness/mutate.rs`); a failure names its seed.

#[path = "../../../robustness/mutate.rs"]
mod mutate;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as B64;
use hmac::{Hmac, Mac};
use mutate::{Rng, mutate};
use sha2::Sha256;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use wasl_ipc::transport::{self, BoxStream, MAX_HANDSHAKE_LINE};
use wasl_ipc::{
    Client, ErrorKind, Event, Frame, Handler, NewView, Passphrase, Paths, RemoteError, Request, Server,
    VaultFolder, ok,
};

const SEEDS: u64 = 1024;
/// Each socket case opens a real connection to a real server.
const SOCKET_SEEDS: u64 = 64;

struct Counting {
    calls: AtomicUsize,
}

#[async_trait::async_trait]
impl Handler for Counting {
    async fn handle(&self, request: Request) -> Result<serde_json::Value, RemoteError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        match request {
            Request::Setting { key } => ok(Some(key)),
            _ => Err(RemoteError {
                kind: ErrorKind::Unsupported,
                message: "not in this test".into(),
            }),
        }
    }
}

fn frames() -> Vec<String> {
    let view = NewView {
        name: "Photos".into(),
        account_id: "me@example.com".into(),
        drive_folder_id: "F".into(),
        drive_id: None,
        remote_path: "My Drive/Photos".into(),
        local_path: "C:\\Users\\me\\Photos".into(),
        cache_limit: Some(1 << 30),
    };
    let requests = [
        Request::Status,
        Request::Pair { id: "p-1".into() },
        Request::Setting {
            key: "bandwidth".into(),
        },
        Request::UnlockVault {
            id: "vault-1".into(),
            passphrase: Passphrase::new("correct horse"),
        },
        Request::UnlockVaultFolder {
            folder: VaultFolder {
                account_id: "me@example.com".into(),
                folder_id: "F".into(),
                drive_id: Some("0AD".into()),
                remote_path: "My Drive/Private".into(),
            },
            passphrase: Passphrase::new("correct horse"),
        },
        Request::AddView { view },
        Request::Subscribe,
    ];
    let mut out: Vec<String> = requests
        .into_iter()
        .enumerate()
        .map(|(i, r)| {
            serde_json::to_string(&Frame::Request {
                id: i as u64 + 1,
                request: Box::new(r),
            })
            .unwrap()
        })
        .collect();
    out.push(
        serde_json::to_string(&Frame::Ok {
            id: 9,
            ok: serde_json::json!({"a": [1, "b", null]}),
        })
        .unwrap(),
    );
    out.push(
        serde_json::to_string(&Frame::Err {
            id: 10,
            err: RemoteError {
                kind: ErrorKind::Locked,
                message: "Vault is locked.".into(),
            },
        })
        .unwrap(),
    );
    for event in [
        Event::Changed,
        Event::AccountFailed {
            message: "denied".into(),
        },
        Event::ShuttingDown,
    ] {
        out.push(serde_json::to_string(&Frame::Event { event }).unwrap());
    }
    out
}

fn hello_line(nonce: &str) -> String {
    format!(r#"{{"hello":{{"version":1,"nonce":"{nonce}"}}}}"#)
}

fn proof(token: &[u8], role: &str, nonce: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(token).unwrap();
    mac.update(role.as_bytes());
    mac.update(b"\0");
    mac.update(nonce.as_bytes());
    B64.encode(mac.finalize().into_bytes())
}

/// Reads every line the reader yields until it ends; returns how many reads it took.
async fn drain<T: serde::de::DeserializeOwned>(input: Vec<u8>, limit: usize) -> usize {
    let (mut a, b) = tokio::io::duplex(1 << 16);
    let len = input.len();
    let writer = tokio::spawn(async move {
        let _ = a.write_all(&input).await;
        let _ = a.shutdown().await;
    });
    let (mut reader, w) = transport::split(Box::new(b) as BoxStream);
    reader.set_limit(limit);
    let mut reads = 0;
    loop {
        reads += 1;
        assert!(reads <= len + 2, "the reader does not make progress");
        match reader.read::<T>().await {
            Ok(Some(_)) => {}
            Ok(None) => break,
            // A line too long or cut short ends the stream for the caller; a malformed one only
            // that line. Either way the reader keeps working.
            Err(e) if e.kind() == std::io::ErrorKind::InvalidData && e.to_string() == "message too long" => {
                break;
            }
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(_) => {}
        }
    }
    // Like the daemon, drop the connection once reading stops, so the writer is not left waiting
    // on a full pipe.
    drop((reader, w));
    writer.await.unwrap();
    reads
}

fn runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
}

#[test]
fn the_line_reader_survives_mutated_frames() {
    let frames = frames();
    for seed in 0..SEEDS {
        let mut rng = Rng::new(seed);
        // A few lines, one of them mutated, sometimes without the final newline.
        let mut input = Vec::new();
        for _ in 0..1 + rng.below(3) {
            let f = &frames[rng.below(frames.len())];
            input.extend(if rng.below(2) == 0 {
                mutate(f.as_bytes(), rng.next_u64())
            } else {
                f.clone().into_bytes()
            });
            if rng.below(8) != 0 {
                input.push(b'\n');
            }
        }
        let limit = if rng.below(2) == 0 {
            MAX_HANDSHAKE_LINE
        } else {
            512
        };
        mutate::guard(&format!("line reader seed {seed}"), move || {
            runtime().block_on(async {
                drain::<Frame>(input.clone(), limit).await;
                drain::<Request>(input.clone(), limit).await;
                drain::<Event>(input.clone(), limit).await;
                drain::<serde_json::Value>(input, limit).await;
            })
        });
    }
}

#[test]
fn the_line_reader_survives_random_bytes_and_deep_json() {
    let mut rng = Rng::new(7);
    let mut inputs: Vec<Vec<u8>> = (0..SEEDS)
        .map(|_| {
            let n = rng.below(600);
            rng.bytes(n)
        })
        .collect();
    inputs.push(mutate::deep_json(100_000).into_bytes());
    inputs.push(format!("{}\n", mutate::deep_json(2_000)).into_bytes());
    inputs.push(vec![b'\n'; 10_000]);
    inputs.push(vec![b'a'; MAX_HANDSHAKE_LINE * 4]);
    for (i, input) in inputs.into_iter().enumerate() {
        mutate::guard(&format!("random line input {i}"), move || {
            runtime().block_on(async {
                drain::<Frame>(input.clone(), MAX_HANDSHAKE_LINE).await;
                drain::<Frame>(input, transport::MAX_LINE).await;
            })
        });
    }
}

async fn read_line(stream: &mut BoxStream) -> Option<String> {
    let mut line = Vec::new();
    let mut byte = [0u8; 1];
    loop {
        match tokio::time::timeout(Duration::from_secs(8), stream.read(&mut byte)).await {
            Err(_) => panic!("the daemon neither answered nor closed within 8 s"),
            Ok(Ok(0)) | Ok(Err(_)) => return None,
            Ok(Ok(_)) if byte[0] == b'\n' => return Some(String::from_utf8_lossy(&line).into_owned()),
            Ok(Ok(_)) => line.push(byte[0]),
        }
    }
}

fn start(dir: &std::path::Path) -> (Paths, Arc<Counting>) {
    let paths = Paths::at(dir);
    let handler = Arc::new(Counting {
        calls: AtomicUsize::new(0),
    });
    let server = Server::bind(&paths, handler.clone(), "robustness").unwrap();
    tokio::spawn(server.serve());
    (paths, handler)
}

fn token_bytes(paths: &Paths) -> Vec<u8> {
    B64.decode(std::fs::read_to_string(paths.token()).unwrap().trim())
        .unwrap()
}

/// Mutated hellos and proofs from a process that does not know the token: refused, nothing
/// reaches the handler, and the daemon keeps serving.
#[tokio::test(flavor = "multi_thread")]
async fn the_handshake_survives_mutated_openings() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, handler) = start(dir.path());
    let token = token_bytes(&paths);
    for seed in 0..SOCKET_SEEDS {
        let mut rng = Rng::new(seed);
        let mine = B64.encode(rng.bytes(32));
        let mut raw = transport::connect(&paths).await.unwrap();
        let mut hello = mutate(hello_line(&mine).as_bytes(), seed);
        hello.push(b'\n');
        let _ = raw.write_all(&hello).await;
        let Some(answer) = read_line(&mut raw).await else {
            continue;
        };
        // The daemon answered a hello that still parses: it proves itself for the nonce sent.
        let v: serde_json::Value = serde_json::from_str(&answer).unwrap();
        let theirs = v["nonce"].as_str().unwrap().to_owned();
        assert!(v["proof"].is_string(), "seed {seed}: {answer}");
        let good = format!(r#"{{"proof":"{}"}}"#, proof(&token, "wasl", &theirs));
        let mut client_proof = mutate(good.as_bytes(), rng.next_u64());
        let honest = client_proof == good.as_bytes();
        client_proof.push(b'\n');
        let _ = raw.write_all(&client_proof).await;
        if let Some(welcome) = read_line(&mut raw).await {
            // Only a proof that still verifies (an unchanged or equivalent line) is welcomed.
            let sent: serde_json::Value = serde_json::from_slice(&client_proof).unwrap_or_default();
            assert!(
                honest
                    || sent["proof"].as_str().and_then(|p| B64.decode(p).ok())
                        == B64.decode(proof(&token, "wasl", &theirs)).ok(),
                "seed {seed}: welcomed after a wrong proof: {welcome}"
            );
        }
    }
    assert_eq!(0, handler.calls.load(Ordering::SeqCst));
    let c = Client::connect(&paths)
        .await
        .expect("the daemon still serves its user");
    let v: Option<String> = c.call(Request::Setting { key: "k".into() }).await.unwrap();
    assert_eq!(v.as_deref(), Some("k"));
}

/// After an honest handshake, mutated frames: each connection gets an answer to its next valid
/// request or is closed, and the daemon keeps serving.
#[tokio::test(flavor = "multi_thread")]
async fn proven_clients_sending_mutated_frames_cannot_break_the_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let (paths, _) = start(dir.path());
    let token = token_bytes(&paths);
    let frames = frames();
    for seed in 0..SOCKET_SEEDS {
        let mut rng = Rng::new(seed ^ 0xF00D);
        let mut raw = transport::connect(&paths).await.unwrap();
        let mine = B64.encode(rng.bytes(32));
        raw.write_all(format!("{}\n", hello_line(&mine)).as_bytes())
            .await
            .unwrap();
        let answer = read_line(&mut raw).await.expect("daemon proof");
        let v: serde_json::Value = serde_json::from_str(&answer).unwrap();
        let theirs = v["nonce"].as_str().unwrap();
        raw.write_all(format!("{{\"proof\":\"{}\"}}\n", proof(&token, "wasl", theirs)).as_bytes())
            .await
            .unwrap();
        read_line(&mut raw).await.expect("welcome");

        let mut bad = mutate(frames[rng.below(frames.len())].as_bytes(), seed);
        bad.retain(|&b| b != b'\n');
        bad.push(b'\n');
        let _ = raw.write_all(&bad).await;
        let probe = r#"{"id":4242,"request":{"op":"setting","key":"probe"}}"#;
        let _ = raw.write_all(format!("{probe}\n").as_bytes()).await;
        // Read until the probe's answer or the end of the connection.
        while let Some(line) = read_line(&mut raw).await {
            if line.contains("4242") {
                break;
            }
        }
    }
    let c = Client::connect(&paths)
        .await
        .expect("the daemon still serves its user");
    let v: Option<String> = c.call(Request::Setting { key: "after".into() }).await.unwrap();
    assert_eq!(v.as_deref(), Some("after"));
}
