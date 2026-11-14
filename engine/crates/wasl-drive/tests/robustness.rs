//! Robustness of what `wasl-drive` reads from Google (docs/SECURITY.md S5-2): file resources, file
//! lists, change pages, About, revisions, error bodies and RFC 3339 times, arbitrary and mutated,
//! both parsed directly and served by a fake Drive to the real client. Every answer must be a value
//! or an error: no panic, in bounded time.
//!
//! `wasl-drive` has no `proptest` dev-dependency, so the cases come from a fixed seed loop over the
//! shared mutator (`engine/robustness/mutate.rs`); a failure names its seed.

mod common;
#[path = "../../../robustness/mutate.rs"]
mod mutate;

use std::sync::Arc;
use std::time::Duration;

use common::*;
use mutate::{deep_json, guard, mutate, mutate_text};
use wasl_drive::{
    About, Change, ChangeList, DriveFile, DriveNode, FileList, ListPage, Revision, SharedDrive,
    parse_rfc3339, to_rfc3339,
};

const SEEDS: u64 = 1024;
/// Each HTTP case is a round trip to the local fake server.
const HTTP_SEEDS: u64 = 96;

fn file() -> String {
    r#"{"id":"1AbC","name":"Report – final.pdf","mimeType":"application/pdf","parents":["0Bxy"],"size":"123456789","md5Checksum":"0cc175b9c0f1b6a831c399e269772661","modifiedTime":"2026-01-02T03:04:05.120Z","version":"42","trashed":false,"driveId":"0ADr","shortcutDetails":{"targetId":"1T","targetMimeType":"application/vnd.google-apps.folder"},"shared":true,"permissionIds":["a","b"],"appProperties":{"wasl.m":"AAAA","wasl.dir":"BBBB"}}"#.into()
}

fn file_list() -> String {
    format!(
        r#"{{"nextPageToken":"tok~1","incompleteSearch":false,"files":[{f},{f},{{"id":"2","mimeType":"application/vnd.google-apps.folder"}}]}}"#,
        f = file()
    )
}

fn change_list() -> String {
    format!(
        r#"{{"nextPageToken":"7","newStartPageToken":"8","changes":[{{"changeType":"file","fileId":"1AbC","removed":false,"time":"2026-01-02T03:04:05Z","file":{f}}},{{"changeType":"file","fileId":"9","removed":true}},{{"changeType":"drive","driveId":"0ADr"}}]}}"#,
        f = file()
    )
}

fn about() -> String {
    r#"{"user":{"emailAddress":"me@example.com","displayName":"Me","photoLink":"https://x/y"},"storageQuota":{"limit":"16106127360","usage":"1","usageInDrive":"1","usageInDriveTrash":"0"}}"#.into()
}

fn revisions() -> String {
    r#"{"id":"r1","modifiedTime":"2026-01-02T03:04:05Z","size":"10","md5Checksum":"x","keepForever":true,"originalFilename":"a.txt","mimeType":"text/plain","lastModifyingUser":{"displayName":"Me","emailAddress":"me@example.com"}}"#.into()
}

fn error_body() -> String {
    r#"{"error":{"code":400,"message":"Invalid Value","errors":[{"domain":"global","reason":"invalid","message":"Invalid Value","locationType":"parameter","location":"pageToken"}]}}"#.into()
}

/// Parses `bytes` as every Drive response type; for files, also builds the cache node.
fn parse_all(bytes: Vec<u8>) {
    if let Ok(f) = serde_json::from_slice::<DriveFile>(&bytes) {
        let node = DriveNode::from_file(&f, "me@example.com");
        assert_eq!(node.id, f.id);
        let _ = (f.size_u64(), f.version_u64(), f.is_google_doc());
    }
    if let Ok(list) = serde_json::from_slice::<FileList>(&bytes) {
        for f in &list.files {
            let _ = DriveNode::from_file(f, "me@example.com");
        }
    }
    if let Ok(page) = serde_json::from_slice::<ChangeList>(&bytes) {
        for c in &page.changes {
            if let Some(f) = &c.file {
                let _ = DriveNode::from_file(f, "me@example.com");
            }
            let _ = parse_rfc3339(c.time.as_deref());
        }
    }
    let _ = serde_json::from_slice::<Change>(&bytes);
    let _ = serde_json::from_slice::<About>(&bytes);
    let _ = serde_json::from_slice::<Revision>(&bytes);
    let _ = serde_json::from_slice::<SharedDrive>(&bytes);
}

#[test]
fn mutated_drive_json_parses_or_fails() {
    let samples = [
        file(),
        file_list(),
        change_list(),
        about(),
        revisions(),
        error_body(),
    ];
    for (i, sample) in samples.iter().enumerate() {
        for seed in 0..SEEDS {
            let bytes = mutate(sample.as_bytes(), seed);
            guard(&format!("Drive JSON sample {i} seed {seed}"), move || {
                parse_all(bytes)
            });
        }
    }
}

#[test]
fn random_bytes_are_not_drive_json() {
    let mut rng = mutate::Rng::new(99);
    for seed in 0..SEEDS {
        let len = rng.below(400);
        let bytes = rng.bytes(len);
        guard(&format!("random Drive JSON {seed}"), move || parse_all(bytes));
    }
}

#[test]
fn deep_and_huge_drive_json_fail_fast() {
    let many_files = format!(r#"{{"files":[{}]}}"#, vec![file(); 20_000].join(","));
    for text in [
        deep_json(100_000),
        format!(r#"{{"files":{}}}"#, deep_json(10_000)),
        format!(r#"{{"id":"x","appProperties":{}}}"#, deep_json(10_000)),
        r#"{"id":"x","size":"99999999999999999999999","version":"-1","modifiedTime":"+275760-09-13T00:00:00Z"}"#.into(),
        many_files,
    ] {
        guard("deep Drive JSON", move || parse_all(text.into_bytes()));
    }
}

#[test]
fn rfc3339_parsing_is_total() {
    let valid = "2026-01-02T03:04:05.120+03:00";
    let mut cases: Vec<String> = (0..SEEDS).map(|s| mutate_text(valid, s)).collect();
    cases.extend(
        [
            "9999-12-31T23:59:59.999999999Z",
            "0000-01-01T00:00:00Z",
            "-0001-01-01T00:00:00Z",
            "2026-02-30T00:00:00Z",
            "2026-01-01T24:00:00Z",
            "2026-01-01T00:00:60Z",
            "2026-01-01T00:00:00+99:99",
            "",
        ]
        .map(String::from),
    );
    for text in cases {
        let t = text.clone();
        if let Some(ms) = guard("parse_rfc3339", move || parse_rfc3339(Some(&t))) {
            // Anything Drive could send that parses formats back without panicking.
            guard(&format!("to_rfc3339({ms}) from {text:?}"), move || to_rfc3339(ms));
        }
    }
}

/// Finding S5-2-F2 (low): `to_rfc3339` panics ("resulting value is out of range" in the `time`
/// crate) for instants outside years -9999..=9999, e.g. `i64::MAX` ms. It formats a local file's
/// modification time for every upload (`UploadRequest::modified_time_ms`, from
/// `wasl-engine/src/drive_remote.rs`), and a file's mtime can be set to any 64-bit value on Linux and
/// macOS (`touch -d @99999999999999`), so one such file in a synced folder panics the sync run.
/// Enable once `to_rfc3339` clamps or the caller omits out-of-range times.
#[test]
fn formatting_any_time_never_panics() {
    for ms in [
        i64::MAX,
        i64::MIN,
        253_402_300_800_000,
        -62_167_219_200_001,
        1 << 60,
    ] {
        guard(&format!("to_rfc3339({ms})"), move || to_rfc3339(ms));
    }
}

#[test]
fn formatting_times_drive_accepts_works() {
    for ms in [
        0,
        1,
        -1,
        1_735_787_045_120,
        253_402_300_799_999,
        -62_135_596_800_000,
    ] {
        let text = to_rfc3339(ms);
        assert_eq!(parse_rfc3339(Some(&text)), Some(ms), "{text}");
    }
}

/// The real client against a fake Drive answering with mutated bodies: each call ends with a
/// value or an error, promptly.
#[tokio::test(flavor = "multi_thread")]
async fn the_client_survives_mutated_responses() {
    let drive = MockDrive::start().await;
    let api = drive.client(Arc::new(FakeTokens::default()));
    let samples = [change_list(), file_list(), about(), file(), error_body()];
    for seed in 0..HTTP_SEEDS {
        for (i, sample) in samples.iter().enumerate() {
            let body = mutate(sample.as_bytes(), seed);
            // Error bodies come with an error status that is not retried (no backoff waits).
            let code = if i == 4 { 400 } else { 200 };
            drive.enqueue(
                wiremock::ResponseTemplate::new(code).set_body_raw(body, "application/json; charset=utf-8"),
            );
            let call = async {
                match i {
                    0 => api.changes_page(ACCOUNT, "5", None).await.map(|_| ()),
                    1 => api.list_page(ACCOUNT, &ListPage::default()).await.map(|_| ()),
                    2 => api.about(ACCOUNT).await.map(|_| ()),
                    _ => api.get_file(ACCOUNT, "1AbC").await.map(|_| ()),
                }
            };
            let r = tokio::time::timeout(Duration::from_secs(20), call).await;
            assert!(r.is_ok(), "sample {i} seed {seed}: no answer in 20 s");
        }
    }
}
