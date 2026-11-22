//! The wire form of every kind of sealed blob, pinned in `docs/blob-wire-vectors.json`. The Kotlin app decodes and
//! re-encodes these in its own unit test (`SealWireVectorsTest`), so a shape one side produces and the other refuses
//! (an empty link key, a v2 hybrid blob) fails in CI instead of on a phone. Regenerate with `UPDATE_VECTORS=1`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use privcord_domain::blob::{PublicJwk, SealedBlob};
use serde_json::{json, Value};

fn cases() -> Vec<(&'static str, SealedBlob)> {
    let key = |x: &str, y: &str| PublicJwk::p256(x, y);
    vec![
        (
            // Device linking: no key of its own, the message is already encrypted under the link code.
            "link-request",
            SealedBlob {
                v: 1,
                to: "link".into(),
                from: "link-request".into(),
                epk: key("", ""),
                iv: String::new(),
                ct: "opaque-request".into(),
                sig: "0f0f".into(),
                kem: None,
                key: None,
            },
        ),
        (
            "classic-v1",
            SealedBlob {
                v: 1,
                to: "uid-to".into(),
                from: "uid-from".into(),
                epk: key("AQID", "BAUG"),
                iv: "aXY".into(),
                ct: "Y3Q".into(),
                sig: "c2ln".into(),
                kem: None,
                key: None,
            },
        ),
        (
            // Protocol v2: hybrid key agreement (offline messages sealed to a one-time key).
            "hybrid-v2",
            SealedBlob {
                v: 2,
                to: "uid-to".into(),
                from: "uid-from".into(),
                epk: key("AQID", "BAUG"),
                iv: "aXY".into(),
                ct: "Y3Q".into(),
                sig: "c2ln".into(),
                kem: Some("a2Vt".into()),
                key: Some("a2V5".into()),
            },
        ),
    ]
}

fn path() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/blob-wire-vectors.json")
}

#[test]
fn the_wire_forms_match_the_pinned_vectors_and_round_trip() {
    let produced: Vec<Value> = cases()
        .into_iter()
        .map(|(name, blob)| {
            let wire = blob.encode();
            assert_eq!(
                SealedBlob::decode(&wire).as_ref(),
                Some(&blob),
                "{name} round trip"
            );
            json!({ "name": name, "wire": wire })
        })
        .collect();
    let value = json!({ "blobs": produced });
    let text = serde_json::to_string_pretty(&value).unwrap() + "\n";
    if std::env::var("UPDATE_VECTORS").is_ok() {
        std::fs::write(path(), &text).unwrap();
    }
    // Compared as JSON, not text: a Windows checkout may turn the file's line endings into CRLF.
    let pinned: Value = serde_json::from_str(
        &std::fs::read_to_string(path())
            .expect("docs/blob-wire-vectors.json (run with UPDATE_VECTORS=1)"),
    )
    .unwrap();
    assert_eq!(
        pinned, value,
        "the wire form changed: run with UPDATE_VECTORS=1 and check the Kotlin test still passes"
    );
}
