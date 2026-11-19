//! Why an account must sign in again (issue #33): the cause is kept typed beside the account's
//! sign-in problem, so nothing words a sign-in from the daemon's raw text.

use wasl_proto::SignInCause::{Ended, Unreadable};
use wasl_store::Store;

fn store() -> Store {
    let s = Store::open_in_memory().unwrap();
    s.set_account_email("a1", "me@example.com").unwrap();
    s
}

#[test]
fn an_account_that_signs_in_fine_has_no_cause() {
    let s = store();
    assert_eq!(None, s.account_sign_in_cause("a1").unwrap());
    assert!(s.accounts_to_sign_in().unwrap().is_empty());
}

#[test]
fn a_problem_is_kept_with_its_cause_and_forgotten_with_it() {
    let s = store();
    s.set_account_sign_in_problem("a1", Unreadable, "keychain entry missing")
        .unwrap();
    assert_eq!(Some(Unreadable), s.account_sign_in_cause("a1").unwrap());
    assert_eq!(
        vec![("a1".to_owned(), Unreadable)],
        s.accounts_to_sign_in().unwrap()
    );

    // The person signs in again: the problem and its cause both go.
    s.set_account_auth_error("a1", None).unwrap();
    assert_eq!(None, s.account_sign_in_cause("a1").unwrap());
    assert!(s.accounts_to_sign_in().unwrap().is_empty());
    // A later problem with no cause of its own reads as Google ending the sign-in, not as the old one.
    s.set_account_auth_error("a1", Some("invalid_grant")).unwrap();
    assert_eq!(Some(Ended), s.account_sign_in_cause("a1").unwrap());
}

#[test]
fn a_problem_recorded_again_keeps_the_cause_already_known() {
    let s = store();
    s.set_account_sign_in_problem("a1", Unreadable, "keychain entry missing")
        .unwrap();
    s.set_account_auth_error("a1", Some("still failing")).unwrap();
    assert_eq!(Some(Unreadable), s.account_sign_in_cause("a1").unwrap());
    s.set_account_sign_in_problem("a1", Ended, "refresh refused")
        .unwrap();
    assert_eq!(Some(Ended), s.account_sign_in_cause("a1").unwrap());
}

#[test]
fn removing_an_account_forgets_its_cause() {
    let s = store();
    s.set_account_sign_in_problem("a1", Unreadable, "keychain entry missing")
        .unwrap();
    assert!(s.remove_account("a1").unwrap());
    // Added again, it starts with no problem and no stale cause.
    s.set_account_email("a1", "me@example.com").unwrap();
    s.set_account_auth_error("a1", Some("x")).unwrap();
    assert_eq!(Some(Ended), s.account_sign_in_cause("a1").unwrap());
}
