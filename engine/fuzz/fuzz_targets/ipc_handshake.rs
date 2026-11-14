//! What the daemon reads from its local socket before and after the handshake, through the real
//! `wasl_ipc::transport` line reader with the daemon's limits: the opening hello and the client's
//! proof (at most `MAX_HANDSHAKE_LINE` each, from any process of any user), then frames.
//!
//! The handshake message types are private to `wasl-ipc`; the structs below mirror their JSON
//! shape (`src/auth.rs`). Keep them in step if the handshake changes.
//!
//! Invariants: no panic, the reader always makes progress, no line beyond its limit is parsed.

#![no_main]

use std::sync::OnceLock;

use libfuzzer_sys::fuzz_target;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use wasl_ipc::Frame;
use wasl_ipc::transport::{self, BoxStream, MAX_HANDSHAKE_LINE, MAX_LINE};

#[allow(dead_code)]
#[derive(Deserialize)]
struct Hello {
    version: u32,
    nonce: String,
}

#[allow(dead_code)]
#[derive(Deserialize)]
enum Opening {
    #[serde(rename = "hello")]
    Hello(Hello),
}

#[allow(dead_code)]
#[derive(Deserialize)]
struct ClientProof {
    proof: String,
}

fn runtime() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Builder::new_current_thread().build().unwrap())
}

fuzz_target!(|data: &[u8]| {
    let input = data.to_vec();
    runtime().block_on(async move {
        let (mut a, b) = tokio::io::duplex(1 << 16);
        let len = input.len();
        let writer = tokio::spawn(async move {
            let _ = a.write_all(&input).await;
            let _ = a.shutdown().await;
        });
        let (mut reader, w) = transport::split(Box::new(b) as BoxStream);
        reader.set_limit(MAX_HANDSHAKE_LINE);
        let opened = matches!(reader.read::<Opening>().await, Ok(Some(_)))
            && matches!(reader.read::<ClientProof>().await, Ok(Some(_)));
        if opened {
            reader.set_limit(MAX_LINE);
            let mut reads = 0;
            loop {
                reads += 1;
                assert!(reads <= len + 2, "the line reader stopped making progress");
                match reader.read::<Frame>().await {
                    Ok(Some(_)) => {}
                    Ok(None) | Err(_) => break,
                }
            }
        }
        // Like the daemon, drop the connection once reading stops; the writer then finishes.
        drop((reader, w));
        let _ = writer.await;
    });
});
