//! The Control D response envelope.
//!
//! Three body shapes exist (keyed, flat, keyed-with-siblings), `body` flips to
//! `[]` on error *and* on some successes, and a `content-type: application/json`
//! response can carry zero bytes — so `body` stays a [`Value`] here and each
//! operation unwraps the shape it verified ([D2](../../docs/decisions.md),
//! [read-verification](../../docs/reference/read-verification.md)).
//!
//! In test builds the envelope types deny unknown fields, so an API field
//! addition fails the fixture tests instead of passing silently — the drift
//! tripwire for an unversioned API (docs/testing.md, Test layers).

use serde::Deserialize;
use serde_json::{Map, Value};

use crate::error::Error;

/// A parsed response envelope. Success is `success == true` — `body: []` is
/// *not* an error marker (successful deletes return it too), and errors are
/// classified from `error`, never from `body`.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct Envelope {
    pub success: Option<bool>,
    pub body: Option<Value>,
    pub error: Option<ApiError>,
    /// Success-ack prose ("Profile has been created"). The read-back
    /// verification is authoritative for every write, so nothing surfaces
    /// this on stderr; modeled only so the strict fixture tests (deny
    /// unknown fields) keep parsing write-ack fixtures.
    #[allow(
        dead_code,
        reason = "modeled for strict fixture parsing; never surfaced"
    )]
    pub message: Option<String>,
}

/// The upstream `error` object, verbatim. Every member is optional — the
/// classifier must survive any of them missing.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiError {
    pub code: Option<i64>,
    pub message: Option<String>,
    /// Server timestamp.
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub date: Option<String>,
    /// Dashboard call-to-action links (the 402 plan gate).
    #[allow(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub actions: Option<Vec<ApiErrorAction>>,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
#[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
pub struct ApiErrorAction {
    pub name: Option<String>,
    pub path: Option<String>,
}

impl Envelope {
    /// `success == true`, the only success marker (branch on `success`/`error`
    /// only — never on the shape of `body`).
    pub fn is_success(&self) -> bool {
        self.success == Some(true)
    }

    /// Keyed: `{"body": {"profiles": [...]}}` — the common shape.
    pub fn keyed(self, key: &str) -> Result<Value, Error> {
        self.keyed_with_siblings(key).map(|(payload, _)| payload)
    }

    /// [`Envelope::keyed`], then deserialized into `T` — a shape mismatch is
    /// still `upstream.error`, exit 8, never a panic.
    pub fn keyed_as<T: serde::de::DeserializeOwned>(self, key: &str) -> Result<T, Error> {
        let payload = self.keyed(key)?;
        serde_json::from_value(payload)
            .map_err(|e| crate::error::upstream_shape(format_args!("in {key}: {e}")))
    }

    /// Flat: `/users`, `/ip` — the body itself is the payload, no controller key.
    pub fn flat(self) -> Result<Value, Error> {
        self.body
            .filter(|body| !matches!(body, Value::Null))
            .ok_or_else(|| shape_error("a body"))
    }

    /// Keyed with siblings: `/network` — `{"network": [...], "time": ...,
    /// "current_pop": ...}`. Returns the keyed payload and the remaining
    /// sibling members.
    pub fn keyed_with_siblings(self, key: &str) -> Result<(Value, Map<String, Value>), Error> {
        let Some(Value::Object(mut object)) = self.body else {
            return Err(shape_error(&format!("an object body holding \"{key}\"")));
        };
        let payload = object
            .remove(key)
            .ok_or_else(|| shape_error(&format!("a \"{key}\" key in the body")))?;
        Ok((payload, object))
    }
}

/// The filters write-ack flips between a family-keyed object and `[]` by
/// emptiness; the spec claims the default rule can do the same. Normalize both
/// to a map — an empty array *is* the empty map, anything else is a shape error.
#[allow(dead_code, reason = "first caller is Phase 5 (`filter`)")]
pub fn object_or_empty_array(value: Value) -> Result<Map<String, Value>, Error> {
    match value {
        Value::Object(map) => Ok(map),
        Value::Array(items) if items.is_empty() => Ok(Map::new()),
        _ => Err(shape_error("an object or an empty array")),
    }
}

/// A 2xx whose body defies the verified shape: success cannot be confirmed,
/// so it classifies exactly like an unparseable 2xx body — exit 8.
fn shape_error(expected: &str) -> Error {
    crate::error::upstream_shape(format_args!("expected {expected}"))
}

/// Test-only fixture loader, shared with the error classifier's tests.
#[cfg(test)]
pub(crate) fn fixture(name: &str) -> Envelope {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/api")
        .join(name);
    let raw =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("fixture {name} unreadable: {e}"));
    serde_json::from_str(&raw).unwrap_or_else(|e| panic!("fixture {name} must deserialize: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Exit;
    use std::fs;
    use std::path::PathBuf;

    /// Every read fixture deserializes strictly — the drift tripwire.
    #[test]
    fn every_fixture_deserializes() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/api");
        let mut checked = 0;
        for entry in fs::read_dir(&dir).expect("fixture dir exists") {
            let path = entry.expect("readable entry").path();
            if path.extension().is_some_and(|e| e == "json")
                && fs::metadata(&path).expect("metadata").len() > 0
            {
                fixture(
                    path.file_name()
                        .and_then(|n| n.to_str())
                        .expect("utf-8 name"),
                );
                checked += 1;
            }
        }
        assert!(checked > 30, "expected the full fixture set, saw {checked}");
    }

    #[test]
    fn keyed_shape_unwraps() {
        let profiles = fixture("profiles.json")
            .keyed("profiles")
            .expect("keyed body");
        let list = profiles.as_array().expect("profiles is an array");
        assert!(!list.is_empty());
        assert!(list[0]["PK"].is_string());
    }

    #[test]
    fn flat_shape_unwraps() {
        let user = fixture("users.json").flat().expect("flat body");
        assert_eq!(user["email"], "user@example.com");
        let ip = fixture("ip.json").flat().expect("flat body");
        assert_eq!(ip["pop"], "JFK");
    }

    #[test]
    fn keyed_with_siblings_unwraps() {
        let (pops, siblings) = fixture("network.json")
            .keyed_with_siblings("network")
            .expect("keyed body with siblings");
        assert!(pops.as_array().is_some_and(|a| !a.is_empty()));
        assert!(siblings["time"].is_i64());
        assert_eq!(siblings["current_pop"], "MAD");
    }

    #[test]
    fn error_envelopes_flip_body_to_empty_array() {
        let envelope = fixture("err_bare_array.json");
        assert!(!envelope.is_success());
        assert_eq!(envelope.body, Some(serde_json::json!([])));
        let error = envelope.error.expect("error member");
        assert_eq!(error.code, Some(40003));
        assert_eq!(error.message.as_deref(), Some("hostnames must be an array"));
    }

    #[test]
    fn success_can_carry_empty_array_body_and_message() {
        // Deletes ack with body: [] and success: true — [] is not an error marker.
        let envelope = fixture("write_profile_create.json");
        assert!(envelope.is_success());
        assert_eq!(
            envelope.message.as_deref(),
            Some("Profile has been created")
        );
    }

    #[test]
    fn plan_gate_carries_actions() {
        let error = fixture("err_plan_gated_402.json")
            .error
            .expect("error member");
        assert_eq!(error.code, Some(40201));
        let actions = error.actions.expect("402 carries dashboard actions");
        assert_eq!(actions[0].name.as_deref(), Some("Upgrade"));
    }

    #[test]
    fn filters_ack_flips_between_map_and_empty_array() {
        // Non-empty: a family-keyed map.
        let body = fixture("write_filter_single.json")
            .keyed("filters")
            .expect("keyed");
        let map = object_or_empty_array(body).expect("map form");
        assert!(map.contains_key("ads"));

        // Empty: [] means the empty map.
        let body = fixture("write_filters_none_enabled.json")
            .keyed("filters")
            .expect("keyed");
        let map = object_or_empty_array(body).expect("empty-array form");
        assert!(map.is_empty());

        // Anything else is a shape error, exit 8.
        let err = object_or_empty_array(serde_json::json!([1])).expect_err("non-empty array");
        assert_eq!(err.exit(), Exit::Retryable);
    }

    #[test]
    fn zero_byte_body_fails_json_parse() {
        // The 0-byte 500: the body must reach the unparseable path, never a
        // default Envelope.
        let raw = fs::read_to_string(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/api/err_empty_body_500.json"),
        )
        .expect("fixture exists");
        assert!(raw.is_empty(), "the fixture is the empty body itself");
        assert!(serde_json::from_str::<Envelope>(&raw).is_err());
    }

    #[test]
    fn shape_mismatches_cannot_confirm_success() {
        let err = fixture("users.json")
            .keyed("profiles")
            .expect_err("flat body has no key");
        assert_eq!(err.code, "upstream.error");
        assert_eq!(err.exit(), Exit::Retryable);
    }
}
