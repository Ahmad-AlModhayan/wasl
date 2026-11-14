//! Device sync tables: share IDs, the index, peer progress and the chunk table.

mod common;

use common::root;

use std::collections::HashMap;
use wasl_core::Route;
use wasl_core::nway::{Index, Scanned};
use wasl_store::{ChunkRow, NewOffer, OfferState, Pair, Store};

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.create_pair(&Pair::new("p", "p", &root("data"))).unwrap();
    s.create_pair(&Pair::new("q", "q", &root("other"))).unwrap();
    s
}

fn file(key: &str, hash: &str, mtime: i64) -> Scanned {
    Scanned {
        key: key.into(),
        is_dir: false,
        size: 3,
        mtime,
        hash: Some(hash.into()),
    }
}

#[test]
fn share_ids_map_both_ways() {
    let s = store();
    assert_eq!(None, s.share_id("p").unwrap());
    s.set_share_id("p", "aa").unwrap();
    s.set_share_id("p", "bb").unwrap();
    assert_eq!(Some("bb".into()), s.share_id("p").unwrap());
    assert_eq!(Some("p".into()), s.pair_for_share("bb").unwrap());
    assert_eq!(None, s.pair_for_share("aa").unwrap());
    assert!(s.set_share_id("q", "bb").is_err(), "a share ID names one pair");
    assert_eq!(vec![("p".to_string(), "bb".to_string())], s.shares().unwrap());
}

#[test]
fn the_index_round_trips_with_tombstones_and_large_ids() {
    let mut s = store();
    assert_eq!(None, s.load_index("p").unwrap());
    let me = u64::MAX - 7; // short IDs use the whole u64
    let mut idx = Index::new(me);
    idx.apply_scan(&[file("a.txt", "h1", 10), file("b.txt", "h2", 20)], 100);
    idx.apply_scan(&[file("a.txt", "h1", 10)], 200); // b.txt deleted
    s.save_index("p", &idx, None, &[]).unwrap();
    let back = s.load_index("p").unwrap().unwrap();
    assert_eq!(back, idx);
    assert!(back.get("b.txt").unwrap().deleted);
    assert_eq!(Some(1), back.get("a.txt").unwrap().vv.get(me).into());

    // Saving only some keys leaves the rest; a pruned tombstone is deleted.
    idx.apply_scan(&[file("a.txt", "h3", 30)], 300);
    idx.prune_tombstones(250);
    s.save_index("p", &idx, Some(&["a.txt".to_string()]), &["b.txt".to_string()])
        .unwrap();
    assert_eq!(s.load_index("p").unwrap().unwrap(), idx);
    assert_eq!(None, s.load_index("q").unwrap(), "pairs are apart");
}

#[test]
fn peer_progress_is_per_pair_and_peer() {
    let s = store();
    assert_eq!(0, s.peer_since("p", "WASL-X").unwrap());
    s.set_peer_since("p", "WASL-X", 41).unwrap();
    s.set_peer_since("p", "WASL-X", 42).unwrap();
    s.set_peer_since("q", "WASL-X", 7).unwrap();
    assert_eq!(42, s.peer_since("p", "WASL-X").unwrap());
    assert_eq!(0, s.peer_since("p", "WASL-Y").unwrap());
}

#[test]
fn chunk_lists_and_locations() {
    let mut s = store();
    let (c1, c2, whole) = ([1u8; 32], [2u8; 32], [9u8; 32]);
    s.put_chunk_list("p", "big.bin", &whole, b"list", &[(c1, 0, 10), (c2, 10, 5)])
        .unwrap();
    assert_eq!(
        Some(b"list".to_vec()),
        s.chunk_list("p", "big.bin", &whole).unwrap()
    );
    assert_eq!(
        None,
        s.chunk_list("p", "big.bin", &[0; 32]).unwrap(),
        "other content"
    );
    assert_eq!(Some(whole), s.chunked_blake3("p", "big.bin").unwrap());
    assert_eq!(
        vec![ChunkRow {
            key: "big.bin".into(),
            offset: 10,
            len: 5
        }],
        s.locate_chunks("p", &c2).unwrap()
    );
    assert!(s.locate_chunks("q", &c2).unwrap().is_empty());

    // Replacing the list replaces the locations.
    s.put_chunk_list("p", "big.bin", &[8; 32], b"list2", &[(c1, 0, 15)])
        .unwrap();
    assert!(s.locate_chunks("p", &c2).unwrap().is_empty());
    s.forget_chunks("p", "big.bin").unwrap();
    assert!(s.locate_chunks("p", &c1).unwrap().is_empty());
    assert_eq!(None, s.chunked_blake3("p", "big.bin").unwrap());
}

#[test]
fn deleting_a_pair_clears_its_device_state() {
    let mut s = store();
    s.set_share_id("p", "aa").unwrap();
    let mut idx = Index::new(1);
    idx.apply_scan(&[file("a.txt", "h", 1)], 1);
    s.save_index("p", &idx, None, &[]).unwrap();
    s.set_peer_since("p", "WASL-X", 3).unwrap();
    s.put_chunk_list("p", "a.txt", &[1; 32], b"l", &[([1; 32], 0, 3)])
        .unwrap();
    s.delete_pair("p").unwrap();
    for table in [
        "pair_shares",
        "device_index",
        "device_index_meta",
        "peer_progress",
        "chunk_lists",
        "chunks",
    ] {
        let n: i64 = s
            .connection()
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |r| r.get(0))
            .unwrap();
        assert_eq!(0, n, "{table}");
    }
}

#[test]
fn changes_since_pages_by_sequence() {
    let mut s = store();
    let mut idx = Index::new(1);
    let files: Vec<Scanned> = (0..5)
        .map(|n| file(&format!("f{n}"), &format!("h{n}"), n))
        .collect();
    idx.apply_scan(&files, 10);
    s.save_index("p", &idx, None, &[]).unwrap();
    let (page, more) = s.index_changes_since("p", 0, 3).unwrap();
    assert_eq!(3, page.len());
    assert!(more);
    let last = page.last().unwrap().2;
    let (rest, more) = s.index_changes_since("p", last, 3).unwrap();
    assert_eq!(2, rest.len());
    assert!(!more);
    let mut seqs: Vec<u64> = page.iter().chain(&rest).map(|(_, _, q)| *q).collect();
    let sorted = {
        let mut v = seqs.clone();
        v.sort();
        v
    };
    assert_eq!(seqs, sorted);
    seqs.dedup();
    assert_eq!(5, seqs.len());
    for (k, v, _) in page.iter().chain(&rest) {
        assert_eq!(idx.get(k).unwrap(), v);
    }
    s.put_chunk_list("p", "f1", &[5; 32], b"L", &[]).unwrap();
    assert_eq!(
        Some(("f1".to_string(), b"L".to_vec())),
        s.chunk_list_by_content("p", &[5; 32]).unwrap()
    );
    assert_eq!(None, s.chunk_list_by_content("q", &[5; 32]).unwrap());
}

// ------------------------------------------------------------- introductions (2026-10)

#[test]
fn a_share_is_for_all_known_devices_until_given_a_list() {
    let mut s = store();
    assert_eq!(None, s.share_all_devices("p").unwrap());
    assert!(
        s.set_share_scope("p", false, &[]).is_err(),
        "only a shared pair has a scope"
    );
    s.set_share_id("p", "aa").unwrap();
    assert_eq!(
        Some(true),
        s.share_all_devices("p").unwrap(),
        "the default, as before v10"
    );
    s.set_share_scope("p", false, &["WASL-B".into(), "WASL-C".into(), "WASL-B".into()])
        .unwrap();
    assert_eq!(Some(false), s.share_all_devices("p").unwrap());
    assert_eq!(vec!["WASL-B", "WASL-C"], s.share_devices("p").unwrap());
    assert!(s.shared_with("p", "WASL-C").unwrap());
    assert!(!s.shared_with("q", "WASL-C").unwrap());
    assert_eq!(
        vec![("p".to_string(), "aa".to_string())],
        s.shares_offered_to("WASL-B").unwrap()
    );
    // Sharing the same pair again keeps its scope; a new list replaces the old one.
    s.set_share_id("p", "aa").unwrap();
    assert_eq!(Some(false), s.share_all_devices("p").unwrap());
    s.set_share_scope("p", true, &["WASL-C".into()]).unwrap();
    assert!(s.shares_offered_to("WASL-B").unwrap().is_empty());
    // Unsharing drops the list with the share.
    s.remove_share_id("p").unwrap();
    assert!(s.share_devices("p").unwrap().is_empty());
    assert!(s.shares_offered_to("WASL-C").unwrap().is_empty());
}

#[test]
fn device_notes_are_made_once_and_changed_on_purpose() {
    let s = store();
    assert!(!s.has_device_notes().unwrap());
    assert_eq!(None, s.device_known("WASL-A").unwrap());
    assert!(s.note_device("WASL-A", false).unwrap());
    assert!(
        !s.note_device("WASL-A", true).unwrap(),
        "a note is never overwritten by noting"
    );
    assert_eq!(Some(false), s.device_known("WASL-A").unwrap());
    s.set_device_known("WASL-A", true).unwrap();
    assert_eq!(Some(true), s.device_known("WASL-A").unwrap());
    assert!(s.has_device_notes().unwrap());
    assert_eq!(vec![("WASL-A".to_string(), true)], s.device_notes().unwrap());
}

fn offer(share: &str, name: &str) -> NewOffer {
    NewOffer {
        share: share.into(),
        name: name.into(),
        files: 3,
        bytes: 300,
    }
}

fn shares(s: &Store) -> Vec<(String, String)> {
    s.received_offers()
        .unwrap()
        .into_iter()
        .map(|o| (o.share, o.from_device))
        .collect()
}

#[test]
fn received_offers_follow_what_the_sender_offers_now() {
    let mut s = store();
    assert!(
        s.put_received_offers("WASL-A", &[offer("s1", "Projects"), offer("s2", "Photos")], 1)
            .unwrap()
    );
    assert!(
        !s.put_received_offers("WASL-A", &[offer("s1", "Projects"), offer("s2", "Photos")], 9)
            .unwrap(),
        "nothing new"
    );
    let got = s.received_offers().unwrap();
    assert_eq!(2, got.len());
    assert_eq!(
        ("WASL-A", 1, OfferState::New),
        (got[0].from_device.as_str(), got[0].first_seen, got[0].state),
        "first seen by this device's clock, kept"
    );

    // A renamed folder updates; a declined offer stays declined.
    assert!(s.set_offer_state("s1", "WASL-A", OfferState::Declined).unwrap());
    assert!(
        s.put_received_offers("WASL-A", &[offer("s1", "Work"), offer("s2", "Photos")], 5)
            .unwrap()
    );
    let s1 = s.received_offer("s1", "WASL-A").unwrap().unwrap();
    assert_eq!(
        ("Work", OfferState::Declined, 1),
        (s1.name.as_str(), s1.state, s1.first_seen)
    );

    // Withdrawn offers go, unless accepted.
    s.set_offer_state("s2", "WASL-A", OfferState::Accepted).unwrap();
    assert!(s.put_received_offers("WASL-A", &[], 6).unwrap());
    assert_eq!(vec![("s2".to_string(), "WASL-A".to_string())], shares(&s));
    assert!(!s.set_offer_state("gone", "WASL-A", OfferState::Declined).unwrap());
}

#[test]
fn a_device_offering_anothers_share_never_takes_its_place() {
    // Security review 2026-10: a rogue member offers the share ID B offers, first, under another
    // name. Both offers are kept, each with its own sender; B's stays B's.
    let mut s = store();
    s.put_received_offers("WASL-ROGUE", &[offer("s1", "Bait")], 1)
        .unwrap();
    s.put_received_offers("WASL-B", &[offer("s1", "Projects")], 2)
        .unwrap();
    assert_eq!(
        vec![
            ("s1".to_string(), "WASL-ROGUE".to_string()),
            ("s1".to_string(), "WASL-B".to_string())
        ],
        shares(&s)
    );
    assert_eq!(
        "Projects",
        s.received_offer("s1", "WASL-B").unwrap().unwrap().name
    );
    // Declining one sender's offer leaves the other's as it was.
    s.set_offer_state("s1", "WASL-ROGUE", OfferState::Declined)
        .unwrap();
    assert_eq!(
        OfferState::New,
        s.received_offer("s1", "WASL-B").unwrap().unwrap().state
    );
    // The rogue withdrawing its offer drops only its own.
    s.put_received_offers("WASL-ROGUE", &[], 3).unwrap();
    assert_eq!(vec![("s1".to_string(), "WASL-B".to_string())], shares(&s));
}

#[test]
fn received_offers_are_bounded_per_device_then_in_all() {
    let mut s = store();
    let many: Vec<_> = (0..6).map(|i| offer(&format!("a{i}"), "x")).collect();
    // Offered one at a time, so each has its own first sighting.
    for i in 0..many.len() {
        s.put_received_offers("WASL-A", &many[..=i], i as i64).unwrap();
    }
    s.set_offer_state("a5", "WASL-A", OfferState::Declined).unwrap();
    s.set_offer_state("a0", "WASL-A", OfferState::Accepted).unwrap();
    s.put_received_offers("WASL-B", &[offer("b0", "y"), offer("b1", "y")], 10)
        .unwrap();
    s.trim_received_offers(4, 5).unwrap();
    // 1. A over its 4: its declined a5 goes, then its oldest new a1 (a0 is accepted).
    // 2. Then 6 in all over 5: taken from A, which holds the most, never from B.
    let left = shares(&s);
    assert_eq!(5, left.len());
    assert!(left.contains(&("b0".into(), "WASL-B".into())));
    assert!(left.contains(&("b1".into(), "WASL-B".into())));
    assert!(left.contains(&("a0".into(), "WASL-A".into())), "accepted stays");
    assert!(!left.iter().any(|(s, _)| s == "a5" || s == "a1" || s == "a2"));
}

#[test]
fn one_device_flooding_offers_does_not_push_out_the_others() {
    let mut s = store();
    s.put_received_offers("WASL-B", &[offer("b0", "Projects")], 1)
        .unwrap();
    let flood: Vec<_> = (0..64).map(|i| offer(&format!("f{i}"), "junk")).collect();
    for n in 0..4 {
        s.put_received_offers(&format!("WASL-F{n}"), &flood, 100 + n)
            .unwrap();
    }
    s.trim_received_offers(64, 64).unwrap();
    let left = shares(&s);
    assert_eq!(64, left.len());
    assert!(
        left.contains(&("b0".into(), "WASL-B".into())),
        "the device with the fewest offers keeps them"
    );
}

#[test]
fn index_totals_count_live_files() {
    let mut s = store();
    assert_eq!((0, 0), s.index_totals("p").unwrap());
    let mut idx = Index::new(1);
    idx.apply_scan(&[file("a.txt", "h1", 10), file("b.txt", "h2", 20)], 100);
    idx.apply_scan(&[file("a.txt", "h1", 10)], 200);
    s.save_index("p", &idx, None, &[]).unwrap();
    assert_eq!((1, 3), s.index_totals("p").unwrap());
}

#[test]
fn one_rule_says_who_a_pair_syncs_with() {
    let mut s = store();
    assert!(!s.share_allows("p", "WASL-A").unwrap(), "not shared");
    s.set_share_id("p", "aa").unwrap();
    assert!(!s.share_allows("p", "WASL-A").unwrap(), "no note: not known");
    s.note_device("WASL-A", false).unwrap();
    assert!(!s.share_allows("p", "WASL-A").unwrap(), "new here");
    s.set_device_known("WASL-A", true).unwrap();
    assert!(s.share_allows("p", "WASL-A").unwrap(), "all devices, known");
    s.set_share_scope("p", false, &["WASL-B".into()]).unwrap();
    assert!(!s.share_allows("p", "WASL-A").unwrap(), "chosen devices only");
    assert!(
        s.share_allows("p", "WASL-B").unwrap(),
        "listed, even with no note"
    );
}

#[test]
fn a_received_share_is_shared_with_its_sender_only() {
    let mut s = store();
    s.set_device_known("WASL-K", true).unwrap();
    s.accept_received_share("q", "bb", "WASL-A", None).unwrap();
    assert_eq!(Some("bb".into()), s.share_id("q").unwrap());
    assert_eq!(Some(false), s.share_all_devices("q").unwrap());
    assert_eq!(vec!["WASL-A"], s.share_devices("q").unwrap());
    assert!(
        !s.share_allows("q", "WASL-K").unwrap(),
        "a known device is not served it"
    );
    s.set_share_id("p", "cc").unwrap();
    assert!(
        s.accept_received_share("p", "bb", "WASL-A", None).is_err(),
        "one share, one pair"
    );
}

/// Review of #28: accepting an offer into a pair that is already shared re-keyed it, cutting off
/// the devices syncing its old share; and the route changed in a second step after the share was
/// set, so a failure in between left a half-accepted offer.
#[test]
fn accepting_never_re_keys_a_shared_pair_and_sets_share_route_and_offer_together() {
    let mut s = store();
    s.put_received_offers(
        "WASL-A",
        &[NewOffer {
            share: "bb".into(),
            name: "Projects".into(),
            files: 1,
            bytes: 1,
        }],
        1,
    )
    .unwrap();
    s.set_share_id("p", "cc").unwrap();
    assert!(s.accept_received_share("p", "bb", "WASL-A", None).is_err());
    assert_eq!(Some("cc".into()), s.share_id("p").unwrap(), "the old share stays");
    assert_eq!(
        OfferState::New,
        s.received_offers().unwrap()[0].state,
        "not accepted either"
    );

    s.accept_received_share("q", "bb", "WASL-A", Some(wasl_core::Route::DriveAndDevices))
        .unwrap();
    assert_eq!(
        wasl_core::Route::DriveAndDevices,
        s.get_pair("q").unwrap().unwrap().route
    );
    assert_eq!(OfferState::Accepted, s.received_offers().unwrap()[0].state);
}

#[test]
fn moving_a_pair_drops_its_device_state_vault_records_and_share() {
    let mut s = store();
    for id in ["p", "q"] {
        let mut idx = Index::new(7);
        idx.apply_scan(&[file("a.txt", "h1", 10)], 100);
        s.save_index(id, &idx, None, &[]).unwrap();
        s.set_peer_since(id, "peer", 5).unwrap();
        s.put_chunk_list(id, "big.bin", &[9; 32], b"list", &[([1; 32], 0, 10)])
            .unwrap();
        s.note_vault_version(id, "f1", 3).unwrap();
        s.set_vault_orphans(id, &HashMap::from([("c1".to_string(), 1)]))
            .unwrap();
    }
    s.set_share_id("p", "aa").unwrap();
    s.set_share_scope("p", false, &["WASL-PEER".to_string()]).unwrap();
    let seq = s.load_index("p").unwrap().unwrap().seq;

    let mut renamed = s.get_pair("q").unwrap().unwrap();
    renamed.name = "Renamed".into();
    assert!(!s.update_pair(&renamed).unwrap(), "a new name keeps the state");
    assert_eq!(1, s.load_index("q").unwrap().unwrap().entries.len());

    // Another folder and a Drive folder too: the pair keeps syncing with Drive, not with devices.
    let mut moved = s.get_pair("p").unwrap().unwrap();
    moved.local_root = root("elsewhere");
    moved.drive_folder_id = Some("folder".into());
    moved.route = Route::DriveAndDevices;
    assert!(s.update_pair(&moved).unwrap());
    let got = s.get_pair("p").unwrap().unwrap();
    assert_eq!(
        Route::Drive,
        got.route,
        "the share went, so devices no longer sync it"
    );
    assert!(got.status.hold_for_preview);
    let idx = s.load_index("p").unwrap().unwrap();
    assert!(idx.entries.is_empty());
    assert_eq!(seq, idx.seq, "other devices keep reading this device's index");
    assert_eq!(0, s.peer_since("p", "peer").unwrap());
    assert_eq!(None, s.chunk_list("p", "big.bin", &[9; 32]).unwrap());
    assert!(s.locate_chunks("p", &[1; 32]).unwrap().is_empty());
    assert_eq!(None, s.vault_version("p", "f1").unwrap());
    assert!(s.vault_orphans("p").unwrap().is_empty());
    assert_eq!(None, s.share_id("p").unwrap());
    assert_eq!(
        None,
        s.pair_for_share("aa").unwrap(),
        "peers on the old folder are refused"
    );
    assert!(s.share_devices("p").unwrap().is_empty());

    // A pair synced with devices only stays one: with no Drive folder it can't take the Drive route.
    let mut solo = Pair::new("r", "Solo", &root("r"));
    solo.route = Route::Devices;
    s.create_pair(&solo).unwrap();
    s.set_share_id("r", "bb").unwrap();
    solo.local_root = root("solo");
    assert!(s.update_pair(&solo).unwrap());
    assert_eq!(Route::Devices, s.get_pair("r").unwrap().unwrap().route);
    assert_eq!(None, s.share_id("r").unwrap());

    // The other pair keeps all of its own.
    assert_eq!(1, s.load_index("q").unwrap().unwrap().entries.len());
    assert_eq!(5, s.peer_since("q", "peer").unwrap());
    assert!(s.chunk_list("q", "big.bin", &[9; 32]).unwrap().is_some());
    assert_eq!(1, s.locate_chunks("q", &[1; 32]).unwrap().len());
    assert_eq!(Some(3), s.vault_version("q", "f1").unwrap());
    assert_eq!(1, s.vault_orphans("q").unwrap().len());
}
