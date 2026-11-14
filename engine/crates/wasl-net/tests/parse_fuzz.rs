//! Untrusted input never panics and never parses into something trusted by accident.

use proptest::prelude::*;
use wasl_net::{DeviceId, DeviceKey, GroupOp, GroupOps, PairingCode, ShortCode};

const SYMBOLS: &str = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

fn symbols(max: usize) -> impl Strategy<Value = String> {
    proptest::collection::vec(proptest::sample::select(SYMBOLS.as_bytes().to_vec()), 0..max)
        .prop_map(|v| String::from_utf8(v).unwrap())
}

fn trusted_group() -> (GroupOps, Vec<u8>) {
    let a = DeviceKey::from_seed(&[1; 32]);
    let b = DeviceKey::from_seed(&[2; 32]);
    let (g, _) = GroupOps::create(&a, "Laptop", 1).unwrap();
    let op = g.add_device(b.public(), "Phone", 2, &a).unwrap();
    (g, op.to_json())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_bytes_are_not_codes(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let text = String::from_utf8_lossy(&bytes);
        prop_assert!(PairingCode::from_qr(&text).is_err());
        let _ = ShortCode::parse(&text);
        if let Ok(id) = DeviceId::parse(&text) {
            prop_assert_eq!(DeviceId::parse(&id.to_string()), Ok(id));
        }
    }

    #[test]
    fn random_symbols_are_not_qr_codes(body in symbols(400)) {
        let text = format!("WASL:{body}");
        prop_assert!(PairingCode::from_qr(&text).is_err());
    }

    #[test]
    fn random_symbols_rarely_make_an_id_and_never_panic(body in symbols(40)) {
        if let Ok(id) = DeviceId::parse(&format!("WASL{body}")) {
            prop_assert_eq!(DeviceId::parse(&id.to_string()), Ok(id));
        }
    }

    #[test]
    fn random_bytes_are_not_trusted_operations(bytes in proptest::collection::vec(any::<u8>(), 0..2000)) {
        let (mut g, _) = trusted_group();
        if let Ok(op) = GroupOp::from_json(&bytes) {
            prop_assert!(g.insert(op).is_err());
        }
    }

    #[test]
    fn a_mutated_operation_is_never_accepted_as_different_content(
        pos in any::<prop::sample::Index>(),
        byte in any::<u8>(),
    ) {
        let (g, good) = trusted_group();
        let original = GroupOp::from_json(&good).unwrap();
        let mut bad = good.clone();
        let i = pos.index(bad.len());
        bad[i] = byte;
        if let Ok(op) = GroupOp::from_json(&bad) {
            // An operation whose parents are unknown waits without effect; anything that changes
            // who is trusted must be the original.
            let mut g2 = g.clone();
            if g2.insert(op.clone()).is_ok() && g2.state() != g.state() {
                prop_assert_eq!(op, original);
            }
        }
    }
}
