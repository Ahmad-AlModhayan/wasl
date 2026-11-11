//! Port of `DriveErrorsTest.kt`: Drive JSON `reason`/`status` mapping, `Retry-After`, malformed bodies.

use wasl_drive::DriveError;

fn body(reason: Option<&str>, message: &str, code: u16, status: Option<&str>) -> String {
    let errors = match reason {
        None => String::new(),
        Some(r) => format!(r#","errors":[{{"domain":"usageLimits","reason":"{r}","message":"{message}"}}]"#),
    };
    let st = match status {
        None => String::new(),
        Some(s) => format!(r#","status":"{s}""#),
    };
    format!(r#"{{"error":{{"code":{code},"message":"{message}"{errors}{st}}}}}"#)
}

fn body403(reason: &str) -> String {
    body(Some(reason), "Something went wrong", 403, None)
}

fn map(code: u16, body_text: Option<&str>, retry_after: Option<&str>) -> DriveError {
    DriveError::from_response(code, "Err", retry_after, body_text)
}

fn map_msg(code: u16, body_text: Option<&str>, status_text: &str) -> DriveError {
    DriveError::from_response(code, status_text, None, body_text)
}

// ---------------------------------------------------------------- 401 / 403

#[test]
fn unauthorized_is_auth() {
    let e = map(
        401,
        Some(&body(Some("authError"), "Invalid Credentials", 401, None)),
        None,
    );
    match &e {
        DriveError::Auth {
            user_action_required, ..
        } => assert!(!user_action_required),
        other => panic!("expected Auth, got {other:?}"),
    }
    assert!(e.to_string().contains("Invalid Credentials"));
}

#[test]
fn forbidden_rate_limit_reasons_are_rate_limited() {
    for reason in [
        "userRateLimitExceeded",
        "rateLimitExceeded",
        "dailyLimitExceeded",
        "sharingRateLimitExceeded",
    ] {
        let e = map(403, Some(&body403(reason)), None);
        match &e {
            DriveError::RateLimited { retry_after_ms, .. } => assert_eq!(0, *retry_after_ms),
            other => panic!("{reason} should be RateLimited, got {other:?}"),
        }
        assert!(e.is_transient());
    }
}

#[test]
fn forbidden_rate_limit_honours_retry_after_seconds() {
    let e = map(403, Some(&body403("userRateLimitExceeded")), Some("3"));
    assert!(matches!(
        e,
        DriveError::RateLimited {
            retry_after_ms: 3000,
            ..
        }
    ));
}

#[test]
fn forbidden_storage_quota_reasons_are_storage_quota() {
    assert!(matches!(
        map(403, Some(&body403("storageQuotaExceeded")), None),
        DriveError::StorageQuota(_)
    ));
    assert!(matches!(
        map(403, Some(&body403("quotaExceeded")), None),
        DriveError::StorageQuota(_)
    ));
    // reason missing but the message mentions the storage quota
    let b = body(
        None,
        "The user's Drive storage quota has been exceeded.",
        403,
        None,
    );
    assert!(matches!(map(403, Some(&b), None), DriveError::StorageQuota(_)));
}

#[test]
fn forbidden_insufficient_permissions_is_auth() {
    let b = body(
        Some("insufficientPermissions"),
        "Insufficient Permission",
        403,
        None,
    );
    let e = map(403, Some(&b), None);
    assert!(matches!(e, DriveError::Auth { .. }));
    assert!(e.to_string().starts_with("Missing Drive permission"));
}

#[test]
fn forbidden_other_reason_is_http403() {
    let b = body(Some("domainPolicy"), "Blocked by admin", 403, None);
    let e = map(403, Some(&b), None);
    match &e {
        DriveError::Http {
            code,
            reason,
            message,
        } => {
            assert_eq!(403, *code);
            assert_eq!(Some("domainPolicy"), reason.as_deref());
            assert_eq!("Blocked by admin", message);
        }
        other => panic!("expected Http, got {other:?}"),
    }
    assert_eq!("Blocked by admin", e.to_string());
    assert!(!e.is_transient());
}

#[test]
fn reason_falls_back_to_status_when_errors_list_is_missing() {
    let b = body(None, "denied", 403, Some("PERMISSION_DENIED"));
    match map(403, Some(&b), None) {
        DriveError::Http { reason, .. } => assert_eq!(Some("PERMISSION_DENIED"), reason.as_deref()),
        other => panic!("expected Http, got {other:?}"),
    }
}

// ---------------------------------------------------------------- 404 / 412 / 429 / 5xx

#[test]
fn not_found_is_not_found() {
    let b = body(Some("notFound"), "File not found: abc", 404, None);
    let e = map(404, Some(&b), None);
    assert!(matches!(e, DriveError::NotFound(_)));
    assert_eq!("File not found: abc", e.to_string());
}

#[test]
fn precondition_failed_is_http412() {
    let b = body(Some("conditionNotMet"), "Something went wrong", 412, None);
    match map(412, Some(&b), None) {
        DriveError::Http { code, reason, .. } => {
            assert_eq!(412, code);
            assert_eq!(Some("conditionNotMet"), reason.as_deref());
        }
        other => panic!("expected Http, got {other:?}"),
    }
}

#[test]
fn too_many_requests_is_rate_limited_with_retry_after() {
    let b = body(Some("rateLimitExceeded"), "Something went wrong", 429, None);
    assert!(matches!(
        map(429, Some(&b), Some("7")),
        DriveError::RateLimited {
            retry_after_ms: 7000,
            ..
        }
    ));
    // 429 is rate limiting even without a parseable body
    assert!(matches!(
        map(429, Some("<html>slow down</html>"), None),
        DriveError::RateLimited { .. }
    ));
}

#[test]
fn retry_after_that_is_not_seconds_is_ignored() {
    let e = map(429, None, Some("Wed, 21 Oct 2026 07:28:00 GMT"));
    assert!(matches!(
        e,
        DriveError::RateLimited {
            retry_after_ms: 0,
            ..
        }
    ));
}

#[test]
fn server_errors_are_transient_http() {
    for code in [500u16, 502, 503, 504] {
        let b = body(Some("backendError"), "Backend Error", code, None);
        let e = map(code, Some(&b), None);
        match &e {
            DriveError::Http { code: c, reason, .. } => {
                assert_eq!(code, *c);
                assert_eq!(Some("backendError"), reason.as_deref());
            }
            other => panic!("expected Http, got {other:?}"),
        }
        assert!(e.to_string().starts_with("Google Drive server error:"));
        assert!(e.is_transient());
    }
}

#[test]
fn other_client_errors_are_non_transient_http() {
    let b = body(Some("invalid"), "Invalid Value", 400, None);
    let e = map(400, Some(&b), None);
    match &e {
        DriveError::Http { code, reason, .. } => {
            assert_eq!(400, *code);
            assert_eq!(Some("invalid"), reason.as_deref());
        }
        other => panic!("expected Http, got {other:?}"),
    }
    assert_eq!("Invalid Value", e.to_string());
    assert!(!e.is_transient());
}

// ---------------------------------------------------------------- body parsing

#[test]
fn malformed_body_falls_back_to_status_message() {
    let e = map_msg(500, Some("<html>Bad Gateway</html>"), "Internal Server Error");
    match &e {
        DriveError::Http { reason, .. } => assert!(reason.is_none()),
        other => panic!("expected Http, got {other:?}"),
    }
    assert_eq!("Google Drive server error: Internal Server Error", e.to_string());
}

#[test]
fn missing_body_and_blank_status_message_uses_http_code() {
    let e = map_msg(418, None, "");
    assert!(matches!(e, DriveError::Http { code: 418, .. }));
    assert_eq!("HTTP 418", e.to_string());
    assert_eq!(
        "Google Drive refused it (418). The next sync tries again.",
        e.user_hint()
    );
}

#[test]
fn unknown_json_fields_are_ignored() {
    let text = r#"{"error":{"code":404,"message":"nope","details":[{"x":1}],"errors":[{"reason":"notFound","extra":true}]},"other":1}"#;
    let e = map(404, Some(text), None);
    assert!(matches!(e, DriveError::NotFound(_)));
    assert_eq!("nope", e.to_string());
}

// ---------------------------------------------------------------- AppError parity (not in DriveErrorsTest)

/// `AppErrorTest` asserted `isTransient` and a non-blank `userHint` for every subclass; the variants
/// this crate ports keep both properties.
#[test]
fn every_variant_has_a_non_blank_user_hint_and_the_kotlin_transience() {
    let cases: Vec<(DriveError, bool)> = vec![
        (DriveError::auth(""), false),
        (DriveError::Network(String::new()), true),
        (
            DriveError::RateLimited {
                retry_after_ms: 0,
                message: String::new(),
            },
            true,
        ),
        (DriveError::StorageQuota(String::new()), false),
        (DriveError::NotFound(String::new()), false),
        (
            DriveError::Http {
                code: 503,
                reason: None,
                message: String::new(),
            },
            true,
        ),
        (
            DriveError::Http {
                code: 404,
                reason: None,
                message: String::new(),
            },
            false,
        ),
        (DriveError::Io(String::new()), false),
        (DriveError::Integrity(String::new()), false),
        (DriveError::Unknown(String::new()), false),
        (DriveError::Unknown("   ".into()), false),
    ];
    for (e, transient) in cases {
        assert_eq!(transient, e.is_transient(), "{e:?}");
        assert!(!e.user_hint().trim().is_empty(), "{e:?}");
    }
}

/// #145 L5: a huge `Retry-After` is capped, so one answer can't park a run until the watchdog
/// calls it stalled (six waits stay under its 15 minutes).
#[test]
fn a_huge_retry_after_is_capped_at_two_minutes() {
    let e = map(429, None, Some("86400"));
    assert!(
        matches!(
            e,
            DriveError::RateLimited {
                retry_after_ms: 120_000,
                ..
            }
        ),
        "{e:?}"
    );
    let e = map(429, None, Some("30"));
    assert!(
        matches!(
            e,
            DriveError::RateLimited {
                retry_after_ms: 30_000,
                ..
            }
        ),
        "{e:?}"
    );
}

/// #145 L5: a request timeout (408) is a passing failure, retried like a server error.
#[test]
fn a_request_timeout_is_transient() {
    let e = map(408, Some("<html>Request Timeout</html>"), None);
    assert!(e.is_transient(), "{e:?}");
}
