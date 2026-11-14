//! Port of `DriveNodeCacheTest.kt`. The Room DAO doubles become [`MemoryNodeStore`]; the two
//! `DriveRemoteFs.listSubtree` cases run against [`DriveNodeCache::list_subtree`], where that logic
//! now lives.

mod common;

use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

use common::*;
use wasl_drive::{
    AccountState, DriveClient, DriveError, DriveFile, DriveNode, DriveNodeCache, FOLDER_MIME,
    MemoryNodeStore, SHORTCUT_MIME, ShortcutDetails,
};

struct Env {
    server: MockDrive,
    store: Arc<MemoryNodeStore>,
    cache: Arc<DriveNodeCache>,
}

async fn env() -> Env {
    let server = MockDrive::start().await;
    let api = Arc::new(server.client(Arc::new(FakeTokens::default())));
    let store = Arc::new(MemoryNodeStore::new());
    let cache = Arc::new(DriveNodeCache::new(api, store.clone()));
    Env { server, store, cache }
}

/// Keeps the `DriveClient` import meaningful: the cache shares the client it is given.
fn _shares_the_client(api: Arc<DriveClient>, store: Arc<MemoryNodeStore>) -> DriveNodeCache {
    DriveNodeCache::new(api, store)
}

fn folder_json(id: &str, name: &str, parent: Option<&str>) -> String {
    file_json(id, name, FOLDER_MIME, parent, "")
}

fn node(id: &str, name: &str, parent: Option<&str>, is_folder: bool) -> DriveNode {
    DriveNode {
        account_id: ACCOUNT.into(),
        id: id.into(),
        name: name.into(),
        mime_type: if is_folder {
            FOLDER_MIME.into()
        } else {
            "text/plain".into()
        },
        parent_id: parent.map(Into::into),
        modified_time: 1000,
        is_folder,
        ..DriveNode::default()
    }
}

fn set(ids: &[&str]) -> BTreeSet<String> {
    ids.iter().map(|s| s.to_string()).collect()
}

fn ids_of(nodes: &[DriveNode]) -> BTreeSet<String> {
    nodes.iter().map(|n| n.id.clone()).collect()
}

impl Env {
    fn account(&self) -> AccountState {
        self.store.account_state(ACCOUNT).unwrap()
    }

    /// Account that already went through a full crawl: root known, token "100", three cached nodes.
    fn seed_crawled(&self) {
        self.store.set_account(AccountState {
            root_folder_id: Some("ROOT".into()),
            changes_token: Some("100".into()),
            ..AccountState::new(ACCOUNT)
        });
        self.store.put(node("A", "a", Some("ROOT"), true));
        self.store.put(node("B", "b.txt", Some("A"), false));
        self.store.put(node("C", "c.txt", Some("ROOT"), false));
    }

    fn enqueue_changes(&self, body: &str) {
        self.server.enqueue(json_response(body));
    }

    fn enqueue_root(&self) {
        self.server.enqueue(json_response(&file_json(
            "ROOT",
            "My Drive",
            FOLDER_MIME,
            None,
            "",
        )));
    }
}

// ================================================================ refresh: full crawl

#[tokio::test(start_paused = true)]
async fn first_refresh_crawls_everything_and_takes_the_start_page_token_before_listing() {
    let t = env().await;
    t.store.set_account(AccountState::new(ACCOUNT));
    t.enqueue_root(); // files/root
    t.server.enqueue(json_response(r#"{"startPageToken":"100"}"#));
    t.server.enqueue(json_response(&format!(
        r#"{{"nextPageToken":"p2","files":[{},{}]}}"#,
        folder_json("A", "a", Some("ROOT")),
        file_json(
            "B",
            "b.txt",
            "text/plain",
            Some("A"),
            r#","size":"12","md5Checksum":"m","modifiedTime":"2026-01-02T03:04:05.000Z","version":"3""#
        )
    )));
    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{}]}}"#,
        file_json("C", "c.txt", "text/plain", Some("ROOT"), "")
    )));
    t.enqueue_changes(r#"{"newStartPageToken":"100","changes":[]}"#);

    assert!(t.cache.refresh(ACCOUNT, None, false).await.unwrap());

    let mut urls = Vec::new();
    for _ in 0..5 {
        urls.push(t.server.take().await);
    }
    assert_eq!("/drive/v3/files/root", path(&urls[0]));
    assert_eq!(
        "/drive/v3/changes/startPageToken",
        path(&urls[1]),
        "the changes token is taken BEFORE the crawl so nothing is lost"
    );
    assert_eq!("/drive/v3/files", path(&urls[2]));
    assert_eq!(Some("trashed = false".into()), query(&urls[2], "q"));
    assert_eq!(None, query(&urls[2], "pageToken"));
    assert_eq!(Some("user".into()), query(&urls[2], "corpora"));
    assert_eq!(Some("p2".into()), query(&urls[3], "pageToken"));
    assert_eq!("/drive/v3/changes", path(&urls[4]));
    assert_eq!(Some("100".into()), query(&urls[4], "pageToken"));
    assert_eq!(5, t.server.request_count().await);

    assert_eq!(set(&["A", "B", "C"]), t.store.ids(ACCOUNT));
    let b = t.store.node(ACCOUNT, "B").unwrap();
    assert_eq!(Some("A"), b.parent_id.as_deref());
    assert_eq!(Some(12), b.size);
    assert_eq!(Some("m"), b.md5.as_deref());
    assert_eq!(Some(3), b.version);
    assert!(!b.is_folder);
    assert_eq!(1_767_323_045_000, b.modified_time);
    assert!(t.store.node(ACCOUNT, "A").unwrap().is_folder);
    assert_eq!(
        Some("ROOT"),
        t.store.node(ACCOUNT, "C").unwrap().parent_id.as_deref()
    );

    let acct = t.account();
    assert_eq!(Some("ROOT"), acct.root_folder_id.as_deref());
    assert_eq!(Some("100"), acct.changes_token.as_deref());
    assert!(acct.last_crawl_at.is_some());
    assert!(acct.last_delta_at.is_some());
}

#[tokio::test(start_paused = true)]
async fn refresh_unknown_account_returns_false_without_requests() {
    let t = env().await;
    assert!(!t.cache.refresh("nobody@example.com", None, false).await.unwrap());
    assert_eq!(0, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn refresh_recrawls_when_the_node_table_is_empty() {
    let t = env().await;
    t.store.set_account(AccountState {
        root_folder_id: Some("ROOT".into()),
        changes_token: Some("100".into()),
        ..AccountState::new(ACCOUNT)
    });
    t.enqueue_root();
    t.server.enqueue(json_response(r#"{"startPageToken":"150"}"#));
    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{}]}}"#,
        file_json("X", "x.txt", "text/plain", Some("ROOT"), "")
    )));
    t.enqueue_changes(r#"{"newStartPageToken":"150","changes":[]}"#);
    assert!(t.cache.refresh(ACCOUNT, None, false).await.unwrap());
    assert_eq!(set(&["X"]), t.store.ids(ACCOUNT));
    assert_eq!(Some("150"), t.account().changes_token.as_deref());
}

#[tokio::test(start_paused = true)]
async fn an_empty_account_is_crawled_once_then_only_asked_for_changes() {
    let t = env().await;
    t.store.set_account(AccountState::new(ACCOUNT));
    t.enqueue_root();
    t.server.enqueue(json_response(r#"{"startPageToken":"100"}"#));
    t.server.enqueue(json_response(r#"{"files":[]}"#));
    t.enqueue_changes(r#"{"newStartPageToken":"100","changes":[]}"#);
    assert!(t.cache.refresh(ACCOUNT, None, false).await.unwrap());
    assert_eq!(4, t.server.request_count().await);

    // An empty Drive is read: the next refresh and listing ask only for changes.
    t.enqueue_changes(r#"{"newStartPageToken":"100","changes":[]}"#);
    assert!(!t.cache.refresh(ACCOUNT, None, false).await.unwrap());
    t.enqueue_changes(r#"{"newStartPageToken":"100","changes":[]}"#);
    let listing = t.cache.list_subtree(ACCOUNT, "ROOT", None).await.unwrap();
    assert!(listing.entries.is_empty());
    assert_eq!(6, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn refresh_force_recrawls_and_replaces_the_cache() {
    let t = env().await;
    t.seed_crawled();
    t.enqueue_root();
    t.server.enqueue(json_response(r#"{"startPageToken":"300"}"#));
    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{}]}}"#,
        file_json("X", "x.txt", "text/plain", Some("ROOT"), "")
    )));
    t.enqueue_changes(r#"{"newStartPageToken":"300","changes":[]}"#);
    assert!(t.cache.refresh(ACCOUNT, None, true).await.unwrap());
    assert_eq!(
        set(&["X"]),
        t.store.ids(ACCOUNT),
        "stale nodes are cleared by the crawl"
    );
    assert_eq!(Some("300"), t.account().changes_token.as_deref());
    assert_eq!(4, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn invalidate_drops_the_changes_token_so_the_next_refresh_recrawls() {
    let t = env().await;
    t.seed_crawled();
    t.cache.invalidate(ACCOUNT).await.unwrap();
    assert_eq!(None, t.account().changes_token);
    t.enqueue_root();
    t.server.enqueue(json_response(r#"{"startPageToken":"400"}"#));
    t.server.enqueue(json_response(r#"{"files":[]}"#));
    t.enqueue_changes(r#"{"newStartPageToken":"400","changes":[]}"#);
    assert!(t.cache.refresh(ACCOUNT, None, false).await.unwrap());
    assert_eq!("/drive/v3/files/root", path(&t.server.take().await));
    assert_eq!(Some("400"), t.account().changes_token.as_deref());
}

// ================================================================ refresh: deltas

#[tokio::test(start_paused = true)]
async fn refresh_applies_deltas_across_pages() {
    let t = env().await;
    t.seed_crawled();
    t.enqueue_changes(&format!(
        r#"{{"nextPageToken":"101","changes":[
            {{"changeType":"file","fileId":"B","removed":true}},
            {{"changeType":"file","fileId":"C","file":{}}}
        ]}}"#,
        file_json(
            "C",
            "c2.txt",
            "text/plain",
            Some("ROOT"),
            r#","md5Checksum":"new","version":"2""#
        )
    ));
    t.enqueue_changes(&format!(
        r#"{{"newStartPageToken":"200","changes":[
            {{"changeType":"drive","driveId":"D","fileId":"C","removed":true}},
            {{"changeType":"file","fileId":"D1","file":{}}},
            {{"changeType":"file","fileId":"A","file":{}}},
            {{"changeType":"file","removed":true}}
        ]}}"#,
        file_json("D1", "new.txt", "text/plain", Some("A"), ""),
        file_json("A", "a", FOLDER_MIME, Some("ROOT"), r#","trashed":true"#)
    ));

    assert!(t.cache.refresh(ACCOUNT, None, false).await.unwrap());

    assert_eq!(
        2,
        t.server.request_count().await,
        "only the two change pages were requested"
    );
    assert_eq!(Some("100".into()), query(&t.server.take().await, "pageToken"));
    assert_eq!(Some("101".into()), query(&t.server.take().await, "pageToken"));

    assert_eq!(set(&["C", "D1"]), t.store.ids(ACCOUNT));
    let c = t.store.node(ACCOUNT, "C").unwrap();
    assert_eq!("c2.txt", c.name);
    assert_eq!(Some("new"), c.md5.as_deref());
    assert_eq!(Some(2), c.version);
    assert_eq!(
        Some("A"),
        t.store.node(ACCOUNT, "D1").unwrap().parent_id.as_deref()
    );
    assert!(t.store.node(ACCOUNT, "B").is_none(), "removed");
    assert!(t.store.node(ACCOUNT, "A").is_none(), "trashed");

    let acct = t.account();
    assert_eq!(Some("200"), acct.changes_token.as_deref());
    assert!(acct.last_delta_at.is_some());
}

/// #144: an item removed on one page and restored on a later page of the same delta is there:
/// the last change wins, so a restored folder does not read as deleted on Drive.
#[tokio::test(start_paused = true)]
async fn refresh_applies_changes_in_order_so_a_restore_after_a_removal_wins() {
    let t = env().await;
    t.seed_crawled();
    t.enqueue_changes(&format!(
        r#"{{"nextPageToken":"101","changes":[
            {{"changeType":"file","fileId":"B","removed":true}},
            {{"changeType":"file","fileId":"A","file":{}}}
        ]}}"#,
        file_json("A", "a", FOLDER_MIME, Some("ROOT"), r#","trashed":true"#)
    ));
    t.enqueue_changes(&format!(
        r#"{{"newStartPageToken":"200","changes":[
            {{"changeType":"file","fileId":"B","file":{}}},
            {{"changeType":"file","fileId":"A","file":{}}},
            {{"changeType":"file","fileId":"C","file":{}}},
            {{"changeType":"file","fileId":"C","removed":true}}
        ]}}"#,
        file_json("B", "b.txt", "text/plain", Some("ROOT"), ""),
        file_json("A", "a", FOLDER_MIME, Some("ROOT"), ""),
        file_json("C", "c.txt", "text/plain", Some("ROOT"), "")
    ));

    assert!(t.cache.refresh(ACCOUNT, None, false).await.unwrap());

    assert_eq!(
        set(&["A", "B"]),
        t.store.ids(ACCOUNT),
        "the last change of each item wins"
    );
}

#[tokio::test(start_paused = true)]
async fn refresh_without_changes_returns_false_and_keeps_the_token() {
    let t = env().await;
    t.seed_crawled();
    t.enqueue_changes(r#"{"newStartPageToken":"100","changes":[]}"#);
    assert!(!t.cache.refresh(ACCOUNT, None, false).await.unwrap());
    assert_eq!(Some("100"), t.account().changes_token.as_deref());
    assert_eq!(set(&["A", "B", "C"]), t.store.ids(ACCOUNT));
    assert_eq!(1, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn refresh_change_without_file_metadata_is_treated_as_removal() {
    let t = env().await;
    t.seed_crawled();
    t.enqueue_changes(
        r#"{"newStartPageToken":"101","changes":[{"changeType":"file","fileId":"C","removed":false}]}"#,
    );
    assert!(t.cache.refresh(ACCOUNT, None, false).await.unwrap());
    assert!(t.store.node(ACCOUNT, "C").is_none());
}

// ================================================================ refresh: shared drives

fn shared(id: &str, name: &str, mime: &str, parent: &str) -> String {
    file_json(id, name, mime, Some(parent), r#","driveId":"D1""#)
}

#[tokio::test(start_paused = true)]
async fn refresh_shared_drive_is_crawled_once_on_first_use() {
    let t = env().await;
    t.seed_crawled();
    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{}]}}"#,
        shared("S1", "s1", FOLDER_MIME, "D1")
    )));
    t.enqueue_changes(r#"{"newStartPageToken":"101","changes":[]}"#);

    assert!(t.cache.refresh(ACCOUNT, Some("D1"), false).await.unwrap());
    let list = t.server.take().await;
    assert_eq!("/drive/v3/files", path(&list));
    assert_eq!(Some("drive".into()), query(&list, "corpora"));
    assert_eq!(Some("D1".into()), query(&list, "driveId"));
    assert_eq!(Some("trashed = false".into()), query(&list, "q"));
    assert_eq!("/drive/v3/changes", path(&t.server.take().await));
    assert_eq!(
        Some("D1"),
        t.store.node(ACCOUNT, "S1").unwrap().drive_id.as_deref()
    );

    // second refresh for the same drive: only the delta
    t.enqueue_changes(r#"{"newStartPageToken":"101","changes":[]}"#);
    assert!(!t.cache.refresh(ACCOUNT, Some("D1"), false).await.unwrap());
    assert_eq!(3, t.server.request_count().await);
}

#[tokio::test(start_paused = true)]
async fn refresh_shared_drive_crawl_that_fails_leaves_nothing_half_read_and_is_crawled_again() {
    let t = env().await;
    t.seed_crawled();
    t.server.enqueue(json_response(&format!(
        r#"{{"nextPageToken":"p2","files":[{}]}}"#,
        shared("S1", "s1", FOLDER_MIME, "D1")
    )));
    t.server.enqueue(api_error(400, "badRequest", "Bad page"));

    assert!(t.cache.refresh(ACCOUNT, Some("D1"), false).await.is_err());
    assert!(
        t.store.node(ACCOUNT, "S1").is_none(),
        "half a shared drive reads as files deleted on Drive"
    );

    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{},{}]}}"#,
        shared("S1", "s1", FOLDER_MIME, "D1"),
        shared("S2", "s2.txt", "text/plain", "S1")
    )));
    t.enqueue_changes(r#"{"newStartPageToken":"101","changes":[]}"#);
    assert!(t.cache.refresh(ACCOUNT, Some("D1"), false).await.unwrap());
    assert!(t.store.node(ACCOUNT, "S2").is_some());
}

#[tokio::test(start_paused = true)]
async fn refresh_full_crawl_that_fails_is_started_again_next_time() {
    let t = env().await;
    t.store.set_account(AccountState {
        root_folder_id: Some("ROOT".into()),
        changes_token: Some("100".into()),
        ..AccountState::new(ACCOUNT)
    });
    t.enqueue_root();
    t.server.enqueue(json_response(r#"{"startPageToken":"150"}"#));
    // A first page big enough that part of it is already stored when the second page fails.
    let first_page = (0..600)
        .map(|i| {
            file_json(
                &format!("F{i}"),
                &format!("f{i}.txt"),
                "text/plain",
                Some("ROOT"),
                "",
            )
        })
        .collect::<Vec<_>>()
        .join(",");
    t.server.enqueue(json_response(&format!(
        r#"{{"nextPageToken":"p2","files":[{first_page}]}}"#
    )));
    t.server.enqueue(api_error(400, "badRequest", "Bad page"));
    assert!(t.cache.refresh(ACCOUNT, None, false).await.is_err());

    t.enqueue_root();
    t.server.enqueue(json_response(r#"{"startPageToken":"160"}"#));
    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{},{}]}}"#,
        file_json("X", "x.txt", "text/plain", Some("ROOT"), ""),
        file_json("Y", "y.txt", "text/plain", Some("ROOT"), "")
    )));
    t.enqueue_changes(r#"{"newStartPageToken":"160","changes":[]}"#);
    assert!(t.cache.refresh(ACCOUNT, None, false).await.unwrap());

    assert_eq!(
        set(&["X", "Y"]),
        t.store.ids(ACCOUNT),
        "a half-read cache was trusted"
    );
    assert_eq!(Some("160"), t.account().changes_token.as_deref());
}

#[tokio::test(start_paused = true)]
async fn refresh_when_drive_rejects_the_changes_token_recrawls_instead_of_failing_forever() {
    let t = env().await;
    t.seed_crawled();
    t.server.enqueue(api_error(400, "invalid", "Invalid Value"));
    t.enqueue_root();
    t.server.enqueue(json_response(r#"{"startPageToken":"500"}"#));
    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{}]}}"#,
        file_json("X", "x.txt", "text/plain", Some("ROOT"), "")
    )));
    t.enqueue_changes(r#"{"newStartPageToken":"500","changes":[]}"#);

    assert!(t.cache.refresh(ACCOUNT, None, false).await.unwrap());

    assert_eq!(set(&["X"]), t.store.ids(ACCOUNT));
    assert_eq!(Some("500"), t.account().changes_token.as_deref());
}

#[tokio::test(start_paused = true)]
async fn list_subtree_of_a_folder_that_is_gone_from_drive_fails_instead_of_looking_empty() {
    let t = env().await;
    t.seed_crawled();
    t.enqueue_changes(r#"{"newStartPageToken":"100","changes":[]}"#);

    // Not in the cache: Drive itself is asked, and says it is gone.
    t.server
        .enqueue(api_error(404, "notFound", "File not found: GONE."));

    let e = t.cache.list_subtree(ACCOUNT, "GONE", None).await.unwrap_err();
    assert!(matches!(e, DriveError::NotFound(_)), "{e:?}");
}

/// Drive's change list reports a new folder seconds after it was made. A folder made a moment
/// ago (by `pair add --create-drive-folder` or `vault create`) is read live, not called gone
/// (E2E 2026-10: both failed on every first try).
#[tokio::test(start_paused = true)]
async fn list_subtree_of_a_folder_the_change_list_has_not_reported_yet_reads_it_live() {
    let t = env().await;
    t.seed_crawled();
    t.enqueue_changes(r#"{"newStartPageToken":"100","changes":[]}"#);
    t.server
        .enqueue(json_response(&folder_json("NEW", "new", Some("ROOT"))));
    t.server.enqueue(json_response(&format!(
        r#"{{"files":[{}]}}"#,
        file_json("N1", "n.txt", "text/plain", Some("NEW"), "")
    )));

    let l = t.cache.list_subtree(ACCOUNT, "NEW", None).await.unwrap();
    assert_eq!(
        vec!["n.txt"],
        l.entries.iter().map(|e| e.key.as_str()).collect::<Vec<_>>()
    );
}

#[tokio::test(start_paused = true)]
async fn list_subtree_of_my_drive_and_of_a_folder_lists() {
    let t = env().await;
    t.seed_crawled();
    for _ in 0..2 {
        t.enqueue_changes(r#"{"newStartPageToken":"100","changes":[]}"#);
    }

    let root = t.cache.list_subtree(ACCOUNT, "ROOT", None).await.unwrap();
    assert_eq!(
        set(&["a", "a/b.txt", "c.txt"]),
        root.entries.iter().map(|e| e.key.clone()).collect()
    );
    let a = t.cache.list_subtree(ACCOUNT, "A", None).await.unwrap();
    assert_eq!(
        vec!["b.txt"],
        a.entries.iter().map(|e| e.key.as_str()).collect::<Vec<_>>()
    );
}

// ================================================================ snapshot

#[tokio::test(start_paused = true)]
async fn snapshot_exposes_nodes_and_children() {
    let t = env().await;
    t.seed_crawled();
    let snap = t.cache.snapshot(ACCOUNT).await.unwrap();
    assert_eq!(ACCOUNT, snap.account_id);
    assert_eq!("ROOT", snap.root_id);
    assert_eq!(set(&["A", "C"]), ids_of(snap.children_of("ROOT")));
    assert_eq!(
        vec!["B"],
        snap.children_of("A")
            .iter()
            .map(|n| n.id.as_str())
            .collect::<Vec<_>>()
    );
    assert!(snap.children_of("nope").is_empty());
    assert_eq!("b.txt", snap.node("B").unwrap().name);
    assert!(snap.node("nope").is_none());
}

#[tokio::test(start_paused = true)]
async fn snapshot_ignores_trashed_nodes() {
    let t = env().await;
    t.seed_crawled();
    t.store.put(DriveNode {
        trashed: true,
        ..node("T", "t.txt", Some("ROOT"), false)
    });
    let snap = t.cache.snapshot(ACCOUNT).await.unwrap();
    assert!(snap.node("T").is_none());
    assert_eq!(set(&["A", "C"]), ids_of(snap.children_of("ROOT")));
}

#[tokio::test(start_paused = true)]
async fn snapshot_root_id_falls_back_to_alias_before_the_first_crawl() {
    let t = env().await;
    t.store.set_account(AccountState::new(ACCOUNT));
    assert_eq!("root", t.cache.snapshot(ACCOUNT).await.unwrap().root_id);
}

#[tokio::test(start_paused = true)]
async fn path_of_prefixes_my_drive_when_the_root_itself_is_not_cached() {
    // files.list never returns the root folder, so this is the production shape of the cache.
    let t = env().await;
    t.seed_crawled();
    let snap = t.cache.snapshot(ACCOUNT).await.unwrap();
    let none = HashMap::new();
    assert_eq!("My Drive/a/b.txt", snap.path_of("B", &none));
    assert_eq!("My Drive/a", snap.path_of("A", &none));
    assert_eq!("My Drive/c.txt", snap.path_of("C", &none));
    assert_eq!("", snap.path_of("unknown", &none));
}

#[tokio::test(start_paused = true)]
async fn path_of_does_not_duplicate_the_root_name_when_the_root_is_cached() {
    let t = env().await;
    t.seed_crawled();
    t.store.put(node("ROOT", "My Drive", None, true));
    let snap = t.cache.snapshot(ACCOUNT).await.unwrap();
    let none = HashMap::new();
    assert_eq!("My Drive/a/b.txt", snap.path_of("B", &none));
    assert_eq!("My Drive", snap.path_of("ROOT", &none));
}

#[tokio::test(start_paused = true)]
async fn path_of_shared_drive_uses_the_drive_name_or_a_generic_label() {
    let t = env().await;
    t.seed_crawled();
    t.store.put(DriveNode {
        drive_id: Some("D1".into()),
        ..node("S1", "s1", Some("D1"), true)
    });
    t.store.put(DriveNode {
        drive_id: Some("D1".into()),
        ..node("S2", "s2.txt", Some("S1"), false)
    });
    let snap = t.cache.snapshot(ACCOUNT).await.unwrap();
    let names = HashMap::from([("D1".to_string(), "Team".to_string())]);
    assert_eq!("Team/s1/s2.txt", snap.path_of("S2", &names));
    assert_eq!("Shared drive/s1/s2.txt", snap.path_of("S2", &HashMap::new()));
}

#[tokio::test(start_paused = true)]
async fn path_of_shared_with_me_item_has_no_root_prefix_and_no_duplicated_name() {
    let t = env().await;
    t.seed_crawled();
    t.store.put(node("W", "shared folder", None, true));
    t.store.put(node("W1", "inner.txt", Some("W"), false));
    let snap = t.cache.snapshot(ACCOUNT).await.unwrap();
    let none = HashMap::new();
    assert_eq!("shared folder", snap.path_of("W", &none));
    assert_eq!("shared folder/inner.txt", snap.path_of("W1", &none));
}

// ================================================================ remember / forget

#[tokio::test(start_paused = true)]
async fn remember_upserts_the_file_and_forget_removes_it() {
    let t = env().await;
    t.seed_crawled();
    t.cache
        .remember(
            ACCOUNT,
            &DriveFile {
                id: "N".into(),
                name: "n.txt".into(),
                mime_type: "text/plain".into(),
                parents: Some(vec!["ROOT".into()]),
                size: Some("5".into()),
                md5_checksum: Some("q".into()),
                modified_time: Some("2026-01-02T03:04:05Z".into()),
                version: Some("1".into()),
                ..DriveFile::default()
            },
        )
        .await
        .unwrap();
    let n = t.store.node(ACCOUNT, "N").unwrap();
    assert_eq!("n.txt", n.name);
    assert_eq!(Some("ROOT"), n.parent_id.as_deref());
    assert_eq!(Some(5), n.size);
    assert_eq!(Some("q"), n.md5.as_deref());
    assert_eq!(Some(1), n.version);
    assert_eq!(1_767_323_045_000, n.modified_time);
    assert!(!n.is_folder);
    let snap = t.cache.snapshot(ACCOUNT).await.unwrap();
    let new_children: BTreeSet<String> = ids_of(snap.children_of("ROOT"))
        .difference(&set(&["A", "C"]))
        .cloned()
        .collect();
    assert_eq!(set(&["N"]), new_children);

    t.cache
        .remember(
            ACCOUNT,
            &DriveFile {
                id: "N".into(),
                name: "renamed.txt".into(),
                mime_type: "text/plain".into(),
                parents: Some(vec!["A".into()]),
                ..DriveFile::default()
            },
        )
        .await
        .unwrap();
    assert_eq!("renamed.txt", t.store.node(ACCOUNT, "N").unwrap().name);
    assert_eq!(
        Some("A"),
        t.store.node(ACCOUNT, "N").unwrap().parent_id.as_deref()
    );

    t.cache.forget(ACCOUNT, "N").await.unwrap();
    assert!(t.store.node(ACCOUNT, "N").is_none());
    assert_eq!(0, t.server.request_count().await);
}

#[test]
fn to_node_maps_shortcuts_and_missing_fields() {
    let shortcut = DriveFile {
        id: "S".into(),
        name: "s".into(),
        mime_type: SHORTCUT_MIME.into(),
        shortcut_details: Some(ShortcutDetails {
            target_id: Some("T".into()),
            target_mime_type: Some(FOLDER_MIME.into()),
        }),
        ..DriveFile::default()
    };
    let n = DriveNode::from_file(&shortcut, ACCOUNT);
    assert!(n.is_shortcut);
    assert!(!n.is_folder);
    assert_eq!(Some("T"), n.shortcut_target_id.as_deref());
    assert_eq!(Some(FOLDER_MIME), n.shortcut_target_mime.as_deref());
    assert_eq!(None, n.parent_id);
    assert_eq!(None, n.size);
    assert_eq!(0, n.modified_time);
    assert_eq!(None, n.version);
}

// ================================================================ Rust-only additions

/// Kotlin's `pagesRead` feeds the sync's stall watchdog during long crawls.
#[tokio::test(start_paused = true)]
async fn pages_read_counts_every_page_of_crawls_and_deltas() {
    let t = env().await;
    t.seed_crawled();
    t.enqueue_changes(r#"{"nextPageToken":"101","changes":[]}"#);
    t.enqueue_changes(r#"{"newStartPageToken":"102","changes":[]}"#);
    let before = t.cache.pages_read();
    t.cache.refresh(ACCOUNT, None, false).await.unwrap();
    assert_eq!(2, t.cache.pages_read() - before);
}

/// Duplicates (case-insensitive) keep the newest and shortcuts are reported, not listed.
#[tokio::test(start_paused = true)]
async fn list_subtree_reports_duplicates_and_shortcuts() {
    let t = env().await;
    t.seed_crawled();
    t.store.put(DriveNode {
        modified_time: 5000,
        ..node("C2", "C.TXT", Some("ROOT"), false)
    });
    t.store.put(DriveNode {
        mime_type: SHORTCUT_MIME.into(),
        is_shortcut: true,
        ..node("L", "link", Some("ROOT"), false)
    });
    t.enqueue_changes(r#"{"newStartPageToken":"100","changes":[]}"#);
    let listing = t.cache.list_subtree(ACCOUNT, "ROOT", None).await.unwrap();
    let c = listing
        .entries
        .iter()
        .find(|e| e.key.eq_ignore_ascii_case("c.txt"))
        .unwrap();
    assert_eq!("C2", c.id, "the most recently modified duplicate wins");
    assert_eq!(vec!["c.txt"], listing.duplicates);
    assert_eq!(vec!["link"], listing.shortcuts);
}
