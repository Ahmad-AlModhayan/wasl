//! On-demand views in the store.

use wasl_store::{Store, View};

fn view(id: &str, local: &str) -> View {
    View {
        id: id.into(),
        name: format!("View {id}"),
        account_id: "me".into(),
        drive_folder_id: "F".into(),
        drive_id: None,
        remote_path: "My Drive/Photos".into(),
        local_path: local.into(),
        vault_id: None,
        cache_limit: 20 << 30,
        enabled: true,
        created_at: 1,
        last_refresh_at: None,
        last_error: None,
    }
}

#[test]
fn views_round_trip_and_keep_one_per_local_path() {
    let s = Store::open_in_memory().unwrap();
    s.create_view(&view("a", "/v/a")).unwrap();
    s.create_view(&view("b", "/v/b")).unwrap();
    assert!(s.create_view(&view("c", "/v/a")).is_err(), "one view per folder");
    assert_eq!(2, s.views().unwrap().len());
    assert_eq!(view("a", "/v/a"), s.view("a").unwrap().unwrap());

    s.set_view_enabled("a", false).unwrap();
    s.set_view_cache_limit("a", 5).unwrap();
    s.set_view_refreshed("a", 99, Some("offline")).unwrap();
    let a = s.view("a").unwrap().unwrap();
    assert_eq!(
        (false, 5, Some(99), Some("offline".to_string())),
        (a.enabled, a.cache_limit, a.last_refresh_at, a.last_error)
    );
    s.set_view_refreshed("a", 100, None).unwrap();
    assert_eq!(None, s.view("a").unwrap().unwrap().last_error);
    s.delete_view("a").unwrap();
    assert_eq!(None, s.view("a").unwrap());
}

#[test]
fn a_view_is_linked_to_the_vault_its_folder_holds() {
    let s = Store::open_in_memory().unwrap();
    s.create_view(&view("a", "/v/a")).unwrap();
    s.set_view_vault("a", Some("vault-1")).unwrap();
    assert_eq!(
        Some("vault-1".to_string()),
        s.view("a").unwrap().unwrap().vault_id
    );
    s.set_view_vault("a", None).unwrap();
    assert_eq!(None, s.view("a").unwrap().unwrap().vault_id);
}
