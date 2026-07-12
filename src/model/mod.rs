//! Wire-shape structs and their normalized output counterparts (D2), one
//! module per noun. Wire types deserialize leniently in the binary and deny
//! unknown fields under test — the same drift tripwire as the envelope.

pub mod action;
pub mod folder;
pub mod profile;
pub mod proxy;

/// Read `tests/fixtures/api/{name}`, unwrap its envelope's `key`, and
/// deserialize into `Vec<T>` — the read/envelope/keyed/deserialize body
/// shared by every per-noun `*_fixture` helper, each defined in its own noun
/// module.
#[cfg(test)]
pub(crate) fn fixture_list<T: serde::de::DeserializeOwned>(name: &str, key: &str) -> Vec<T> {
    let raw = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/api")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("fixture {name} unreadable: {e}"));
    let envelope: crate::api::envelope::Envelope = serde_json::from_str(&raw)
        .unwrap_or_else(|e| panic!("fixture {name} is not an envelope: {e}"));
    let payload = envelope
        .keyed(key)
        .unwrap_or_else(|e| panic!("fixture {name} body holds no {key:?}: {e}"));
    serde_json::from_value(payload)
        .unwrap_or_else(|e| panic!("fixture {name} does not deserialize strictly: {e}"))
}
