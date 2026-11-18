//! The version range in the handshake, on a real socket, with hand-built peers from before ranges.

use crate::auth::{self, ClientProof, DaemonProof, Token};
use crate::proto::{Behind, VersionRange};
use crate::{Client, ClientError, Handler, Paths, RemoteError, Server, transport};
use serde_json::{Value, json};
use std::sync::Arc;

struct Nothing;

#[async_trait::async_trait]
impl Handler for Nothing {
    async fn handle(&self, _: crate::Request) -> Result<Value, RemoteError> {
        Ok(Value::Null)
    }
}

fn r(min: u32, max: u32) -> VersionRange {
    VersionRange { min, max }
}

fn daemon(dir: &std::path::Path, range: VersionRange) -> Paths {
    let paths = Paths::at(dir);
    let server = Server::bind(&paths, Arc::new(Nothing), "test")
        .unwrap()
        .with_protocol_range(range);
    tokio::spawn(server.serve());
    paths
}

#[test]
fn ranges_meet_at_the_highest_common_version_or_say_who_is_behind() {
    assert_eq!(Some(2), r(1, 2).highest_common(r(2, 3)));
    assert_eq!(Some(1), r(1, 1).highest_common(r(1, 1)));
    assert_eq!(None, r(1, 1).highest_common(r(2, 3)));
    assert_eq!(Behind::This, r(1, 1).behind(r(2, 3)));
    assert_eq!(Behind::Peer, r(2, 3).behind(r(1, 1)));
}

#[tokio::test(flavor = "multi_thread")]
async fn a_current_client_and_daemon_speak_version_one() {
    let dir = tempfile::tempdir().unwrap();
    let paths = daemon(dir.path(), VersionRange::CURRENT);
    assert_eq!(1, Client::connect(&paths).await.unwrap().protocol);
}

#[tokio::test(flavor = "multi_thread")]
async fn overlapping_ranges_use_the_highest_common_version() {
    let dir = tempfile::tempdir().unwrap();
    let paths = daemon(dir.path(), r(2, 3));
    let c = Client::connect_with_range(&paths, r(1, 2)).await.unwrap();
    assert_eq!(2, c.protocol);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_needs_a_newer_version_is_told_the_daemon_is_behind() {
    let dir = tempfile::tempdir().unwrap();
    let paths = daemon(dir.path(), r(1, 1));
    let err = Client::connect_with_range(&paths, r(2, 3)).await.err().unwrap();
    assert!(
        matches!(err, ClientError::NeedsUpdate { behind: Behind::Peer }),
        "{err:?}"
    );
}

#[tokio::test(flavor = "multi_thread")]
async fn a_client_that_only_speaks_older_versions_is_told_it_is_behind() {
    let dir = tempfile::tempdir().unwrap();
    let paths = daemon(dir.path(), r(2, 3));
    let err = Client::connect(&paths).await.err().unwrap();
    assert!(
        matches!(err, ClientError::NeedsUpdate { behind: Behind::This }),
        "{err:?}"
    );
}

/// A client from before ranges: `{"hello":{"version":1,"nonce":..}}`, then a `Welcome` it reads
/// with only `version` and `protocol`.
#[tokio::test(flavor = "multi_thread")]
async fn a_hand_built_old_client_handshake_is_answered_in_kind() {
    let dir = tempfile::tempdir().unwrap();
    let paths = daemon(dir.path(), VersionRange::CURRENT);
    let token = Token::read(&paths.token()).unwrap();
    let (mut reader, mut writer) = transport::split(transport::connect(&paths).await.unwrap());
    let nonce = auth::nonce().unwrap();
    writer
        .write(&json!({"hello": {"version": 1, "nonce": nonce}}))
        .await
        .unwrap();
    let theirs: DaemonProof = reader.read().await.unwrap().unwrap();
    assert!(token.verify(auth::DAEMON_ROLE, &nonce, &theirs.proof));
    writer
        .write(&ClientProof {
            proof: token.proof(auth::CLIENT_ROLE, &theirs.nonce),
        })
        .await
        .unwrap();
    let welcome: Value = reader.read().await.unwrap().unwrap();
    assert_eq!(
        Some(1),
        welcome["protocol"].as_u64(),
        "an old client compares this with 1"
    );
    assert_eq!(Some("test"), welcome["version"].as_str());
}

/// A daemon from before ranges answers with `{"version":..,"protocol":1}` and no range.
#[tokio::test(flavor = "multi_thread")]
async fn a_current_client_connects_to_a_hand_built_old_daemon() {
    let dir = tempfile::tempdir().unwrap();
    let paths = Paths::at(dir.path());
    let token = Token::create(&paths.token()).unwrap();
    let mut listener = transport::Listener::bind(&paths).unwrap();
    tokio::spawn(async move {
        let (mut reader, mut writer) = transport::split(listener.accept().await.unwrap());
        let hello: Value = reader.read().await.unwrap().unwrap();
        assert_eq!(
            Some(1),
            hello["hello"]["version"].as_u64(),
            "the lowest version, which an old daemon ignores"
        );
        let nonce = auth::nonce().unwrap();
        writer
            .write(&DaemonProof {
                proof: token.proof(auth::DAEMON_ROLE, hello["hello"]["nonce"].as_str().unwrap()),
                nonce: nonce.clone(),
            })
            .await
            .unwrap();
        let _: ClientProof = reader.read().await.unwrap().unwrap();
        writer
            .write(&json!({"version": "0.3.9", "protocol": 1}))
            .await
            .unwrap();
        let _ = reader.read::<Value>().await;
    });
    let c = Client::connect(&paths).await.unwrap();
    assert_eq!("0.3.9", c.daemon_version);
    assert_eq!(1, c.protocol);
}
