//! The vault v2 chunk sweep's record of when chunks became unreferenced.

use std::collections::HashMap;

use wasl_store::{Pair, Store};

#[test]
fn orphans_round_trip_replace_and_go_with_their_pair() {
    let mut s = Store::open_in_memory().unwrap();
    s.create_pair(&Pair::new("p1", "One", "/tmp/one")).unwrap();
    s.create_pair(&Pair::new("p2", "Two", "/tmp/two")).unwrap();
    assert!(s.vault_orphans("p1").unwrap().is_empty());

    let first = HashMap::from([("a".to_string(), 10), ("b".to_string(), 20)]);
    s.set_vault_orphans("p1", &first).unwrap();
    s.set_vault_orphans("p2", &HashMap::from([("z".to_string(), 5)]))
        .unwrap();
    assert_eq!(first, s.vault_orphans("p1").unwrap());

    // A later sweep replaces the record: "a" was referenced again, "c" is new.
    let second = HashMap::from([("b".to_string(), 20), ("c".to_string(), 30)]);
    s.set_vault_orphans("p1", &second).unwrap();
    assert_eq!(second, s.vault_orphans("p1").unwrap());
    assert_eq!(1, s.vault_orphans("p2").unwrap().len(), "other pairs untouched");

    s.delete_pair("p1").unwrap();
    assert!(
        s.vault_orphans("p1").unwrap().is_empty(),
        "the record goes with the pair"
    );
}
