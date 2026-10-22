// SPDX-License-Identifier: GPL-3.0-or-later
//! The wire format against fixtures shared with the UI's tests
//! (ui/test/proto.test.ts), so the two sides can't drift apart silently.

use serde_json::Value;
use web888_web::proto::*;

fn fixtures() -> Value {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/proto/messages.json");
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

/// Numbers compare as f64 (serde writes 7074000.0 where JSON had 7074000).
fn same(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(x), Value::Number(y)) => x.as_f64() == y.as_f64(),
        (Value::Object(x), Value::Object(y)) => {
            x.len() == y.len() && x.iter().all(|(k, v)| y.get(k).is_some_and(|w| same(v, w)))
        }
        (Value::Array(x), Value::Array(y)) => {
            x.len() == y.len() && x.iter().zip(y).all(|(v, w)| same(v, w))
        }
        _ => a == b,
    }
}

#[test]
fn client_fixtures_parse() {
    for m in fixtures()["client"].as_array().unwrap() {
        let parsed: ClientMsg =
            serde_json::from_value(m.clone()).unwrap_or_else(|e| panic!("{m}: {e}"));
        // optional fields that were present survive a round trip
        let back = serde_json::to_value(&parsed).unwrap();
        for (k, v) in m.as_object().unwrap() {
            assert!(same(v, &back[k]), "{m}: field {k}");
        }
    }
}

#[test]
fn server_fixtures_round_trip() {
    for m in fixtures()["server"].as_array().unwrap() {
        let parsed: ServerMsg =
            serde_json::from_value(m.clone()).unwrap_or_else(|e| panic!("{m}: {e}"));
        let back = serde_json::to_value(&parsed).unwrap();
        assert!(same(m, &back), "{m}\n{back}");
    }
}

#[test]
fn frame_fixtures_match_the_encoders() {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/proto/frames.json");
    let f: Value = serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    let a = audio_frame(
        FLAG_OVERLOAD | FLAG_GAP,
        513,
        12_000,
        -73.5,
        &[0.25, -0.5, 1.0],
    );
    assert_eq!(f["audio"]["hex"], hex(&a));
    let w = wf_frame(
        FLAG_NEW_GEN,
        70_000,
        7_000_123.5,
        468.75,
        -160.0,
        0.625,
        1.7e12,
        &[0, 1, 255],
    );
    assert_eq!(f["wf"]["hex"], hex(&w));
}
