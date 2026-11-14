//! Frames from a paired (or pairing) device: the CBOR request and response bodies, the opening
//! `Hello`, and the length-prefixed frame reader in front of them. The first byte picks the
//! decoder so one corpus covers all of them.
//!
//! Invariants: no panic; anything that decodes keeps its limits and round-trips.

#![no_main]

use libfuzzer_sys::fuzz_target;
use wasl_net::proto::{self, Hello, MAX_REQUEST_BYTES, decode_request, decode_response};

fuzz_target!(|data: &[u8]| {
    let Some((&which, body)) = data.split_first() else {
        return;
    };
    match which % 4 {
        0 => {
            if let Ok(req) = decode_request(body) {
                req.check().expect("decoded requests keep their limits");
                let again = proto::encode(&req);
                assert_eq!(decode_request(&again[4..]).ok(), Some(req));
            }
        }
        1 => {
            if let Ok(resp) = decode_response(body) {
                resp.check().expect("decoded responses keep their limits");
                let again = proto::encode(&resp);
                assert_eq!(decode_response(&again[4..]).ok(), Some(resp));
            }
        }
        2 => {
            let _ = proto::decode::<Hello>(body);
        }
        _ => {
            let rt = tokio::runtime::Builder::new_current_thread().build().unwrap();
            let mut input = body;
            while let Ok(frame) = rt.block_on(proto::read_frame(&mut input, MAX_REQUEST_BYTES)) {
                assert!(frame.len() <= MAX_REQUEST_BYTES);
                let _ = decode_request(&frame);
            }
        }
    }
});
