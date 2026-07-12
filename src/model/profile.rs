//! The `/profiles` wire shape and the normalized profile (D2,
//! commands.md#profile). Counts track **enabled** items only — the `enabled_`
//! output names carry that semantic.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::action::{Action, ApiAction};
use crate::error::Error;
use crate::output::time;

/// One entry of `body.profiles[]`. A disabled profile *replaces* the
/// `disable: null` key with `disable_ttl: <unix ts>` — both are modeled and
/// `enabled`/`disabled_until` derive from whichever arrived.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiProfile {
    #[serde(rename = "PK")]
    pub pk: String,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub user: Option<Value>,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub org: Option<Value>,
    pub name: String,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub featured: Option<Value>,
    pub updated: i64,
    #[serde(default)]
    #[expect(
        dead_code,
        reason = "modeled for the strict fixture tests; never read (only disable_ttl is)"
    )]
    pub disable: Option<Value>,
    #[serde(default)]
    pub disable_ttl: Option<i64>,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub lock: Option<Value>,
    pub profile: ApiProfileStats,
}

#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiProfileStats {
    // A missing count must fail loud as a shape error, not print a
    // confident 0 (`flt`/`rule`/`svc`/`grp`/`opt` are all documented,
    // always-present fields). `cflt`/`ipflt` keep their default: they are
    // modeled only for the strict fixture tests and never read.
    pub flt: Count,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub cflt: Count,
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub ipflt: Count,
    pub rule: Count,
    pub svc: Count,
    pub grp: Count,
    pub opt: OptCount,
    pub da: DefaultActionField,
}

/// `u64`, not `i64`: a negative count is unrepresentable, so serde rejects
/// it and the failure classifies as a shape error — the same fail-loud
/// stance as the timestamp checks, for free.
#[derive(Debug, Default, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct Count {
    pub count: u64,
}

#[derive(Debug, Default, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct OptCount {
    pub count: u64,
    /// Option values ride along in the list payload; the typed option
    /// surface is Phase 5, so they stay opaque here. Tripwire-only, so it
    /// keeps its default rather than failing on an absent `data`.
    #[serde(default)]
    #[expect(dead_code, reason = "modeled for the strict fixture tests; never read")]
    pub data: Vec<Value>,
}

/// `profile.da`: live it is always an action object, but the spec types it
/// `[]` when unset — tolerate both (read-verification.md).
#[derive(Debug)]
pub enum DefaultActionField {
    Unset,
    Action(ApiAction),
}

impl<'de> Deserialize<'de> for DefaultActionField {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let value = Value::deserialize(deserializer)?;
        match value {
            Value::Array(items) if items.is_empty() => Ok(Self::Unset),
            // `serde_json::from_value` re-enters serde on the extracted
            // `Value` rather than the outer deserializer, but `ApiAction`'s
            // `cfg(test) deny_unknown_fields` is a struct attribute, so it
            // still applies here.
            Value::Object(_) => serde_json::from_value::<ApiAction>(value)
                .map(Self::Action)
                .map_err(D::Error::custom),
            _ => Err(D::Error::custom(
                "da must be an action object or the []-when-unset shape",
            )),
        }
    }
}

/// The normalized profile — the documented JSON schema, stable key set.
#[derive(Debug, Serialize)]
pub struct Profile {
    pub id: String,
    pub name: String,
    pub enabled_rules: u64,
    pub enabled_filters: u64,
    pub enabled_services: u64,
    pub folders: u64,
    pub options: u64,
    pub default_action: DefaultAction,
    pub enabled: bool,
    pub disabled_until: Option<String>,
    pub updated: String,
    /// `updated` as Unix seconds, for the table's relative-time column.
    /// Never serialized: the JSON schema stays the RFC-3339 `updated` only.
    #[serde(skip)]
    pub updated_unix: i64,
}

#[derive(Debug, Serialize)]
pub struct DefaultAction {
    pub action: Action,
    pub via: Option<String>,
    pub enabled: bool,
}

impl std::fmt::Display for DefaultAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.action)?;
        if let Some(via) = &self.via {
            write!(f, " via {via}")?;
        }
        if !self.enabled {
            write!(f, " (disabled)")?;
        }
        Ok(())
    }
}

impl Profile {
    pub fn from_api(api: &ApiProfile) -> Result<Self, Error> {
        // The API has no timestamps before its own existence; a negative
        // value is upstream garbage that must fail loud, never silently
        // clamp (that clamp lives in `time::rfc3339` as last-resort display
        // safety, not as normalization's defense).
        if api.updated < 0 {
            return Err(crate::error::upstream_shape(format_args!(
                "negative timestamp updated={}",
                api.updated
            )));
        }
        match api.disable_ttl {
            Some(ttl) if ttl < 0 => {
                return Err(crate::error::upstream_shape(format_args!(
                    "negative timestamp disable_ttl={ttl}"
                )));
            }
            _ => {}
        }

        let default_action = match &api.profile.da {
            DefaultActionField::Action(action) => {
                // The default rule is a rule: a missing `do` is an error,
                // never coerced to a default.
                let action_name = action.action()?.ok_or_else(|| {
                    crate::error::upstream_shape("the default action carries no `do`")
                })?;
                DefaultAction {
                    action: action_name,
                    via: action.via.clone(),
                    enabled: action.enabled()?,
                }
            }
            // The spec's []-when-unset shape: the implicit default.
            DefaultActionField::Unset => DefaultAction {
                action: Action::Bypass,
                via: None,
                enabled: true,
            },
        };
        // `0` re-enables, never "disabled indefinitely" (write-verification.md).
        let disabled_until = match api.disable_ttl {
            Some(ttl) if ttl > 0 => Some(time::rfc3339(ttl)),
            _ => None,
        };
        Ok(Self {
            id: api.pk.clone(),
            name: api.name.clone(),
            enabled_rules: api.profile.rule.count,
            enabled_filters: api.profile.flt.count,
            enabled_services: api.profile.svc.count,
            folders: api.profile.grp.count,
            options: api.profile.opt.count,
            default_action,
            enabled: disabled_until.is_none(),
            disabled_until,
            updated: time::rfc3339(api.updated),
            updated_unix: api.updated,
        })
    }
}

/// Shared by every test in this crate that needs `Vec<ApiProfile>` from a
/// captured fixture (this module and `commands::scope`).
#[cfg(test)]
pub(crate) fn profiles_fixture(name: &str) -> Vec<ApiProfile> {
    let raw = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/api")
            .join(name),
    )
    .expect("fixture exists");
    let envelope: crate::api::envelope::Envelope =
        serde_json::from_str(&raw).expect("fixture is an envelope");
    serde_json::from_value(
        envelope
            .keyed("profiles")
            .expect("fixture body holds profiles"),
    )
    .expect("profiles deserialize strictly")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_captured_list_normalizes() {
        let profiles = profiles_fixture("profiles.json");
        assert_eq!(profiles.len(), 4);
        let kids = Profile::from_api(&profiles[2]).expect("normalizes");
        assert_eq!(kids.id, "pk69038f3c");
        assert_eq!(kids.name, "Kids");
        assert_eq!(kids.enabled_rules, 0);
        assert_eq!(kids.enabled_filters, 13);
        assert_eq!(kids.enabled_services, 16);
        assert_eq!(kids.folders, 0);
        assert_eq!(kids.options, 7);
        assert_eq!(kids.default_action.action, Action::Bypass);
        assert_eq!(kids.default_action.via, None);
        assert!(kids.default_action.enabled);
        assert!(kids.enabled);
        assert_eq!(kids.disabled_until, None);
        assert_eq!(kids.updated, "2025-03-11T11:41:01Z");
    }

    #[test]
    fn disable_ttl_derives_disabled_until() {
        let profiles = profiles_fixture("profiles_dup_names.json");
        let disabled = profiles
            .iter()
            .find(|p| p.disable_ttl.is_some())
            .expect("fixture carries a disabled profile");
        let normalized = Profile::from_api(disabled).expect("normalizes");
        assert!(!normalized.enabled);
        assert_eq!(
            normalized.disabled_until.as_deref(),
            Some("2026-07-15T08:00:00Z")
        );
    }

    #[test]
    fn empty_array_da_normalizes_to_the_implicit_default() {
        let profiles = profiles_fixture("profiles_dup_names.json");
        let unset = profiles
            .iter()
            .find(|p| matches!(p.profile.da, DefaultActionField::Unset))
            .expect("fixture carries the spec's []-shaped da");
        let normalized = Profile::from_api(unset).expect("normalizes");
        assert_eq!(normalized.default_action.action, Action::Bypass);
        assert!(normalized.default_action.enabled);
    }

    #[test]
    fn a_default_action_missing_do_is_an_error() {
        let api: ApiProfile = serde_json::from_value(serde_json::json!({
            "PK": "pk1", "name": "X", "updated": 0,
            "profile": {
                "flt": {"count": 0}, "rule": {"count": 0}, "svc": {"count": 0},
                "grp": {"count": 0}, "opt": {"count": 0},
                "da": {"status": 1}
            }
        }))
        .expect("minimal profile deserializes; other fields default");
        let error = Profile::from_api(&api).expect_err("missing do");
        assert!(error.message.contains("no `do`"));
    }

    #[test]
    fn a_profile_missing_a_required_count_fails_to_deserialize() {
        let result: Result<ApiProfile, _> = serde_json::from_value(serde_json::json!({
            "PK": "pk1", "name": "X", "updated": 0,
            "profile": {
                "flt": {"count": 0}, "svc": {"count": 0},
                "grp": {"count": 0}, "opt": {"count": 0},
                "da": []
            }
        }));
        assert!(
            result.is_err(),
            "a missing `rule` count must fail loud, not default to 0"
        );
    }

    #[test]
    fn a_negative_count_fails_to_deserialize() {
        let result: Result<ApiProfile, _> = serde_json::from_value(serde_json::json!({
            "PK": "pk1", "name": "X", "updated": 0,
            "profile": {
                "flt": {"count": 0}, "rule": {"count": -3}, "svc": {"count": 0},
                "grp": {"count": 0}, "opt": {"count": 0},
                "da": []
            }
        }));
        assert!(
            result.is_err(),
            "a negative count must fail loud, not print confidently"
        );
    }

    #[test]
    fn a_negative_updated_timestamp_is_a_shape_error() {
        let api: ApiProfile = serde_json::from_value(serde_json::json!({
            "PK": "pk1", "name": "X", "updated": -1,
            "profile": {
                "flt": {"count": 0}, "rule": {"count": 0}, "svc": {"count": 0},
                "grp": {"count": 0}, "opt": {"count": 0},
                "da": []
            }
        }))
        .expect("deserializes; from_api validates the range, not serde");
        let error = Profile::from_api(&api).expect_err("negative updated");
        assert!(error.message.contains("updated=-1"));
    }

    #[test]
    fn a_negative_disable_ttl_is_a_shape_error() {
        let api: ApiProfile = serde_json::from_value(serde_json::json!({
            "PK": "pk1", "name": "X", "updated": 0, "disable_ttl": -5,
            "profile": {
                "flt": {"count": 0}, "rule": {"count": 0}, "svc": {"count": 0},
                "grp": {"count": 0}, "opt": {"count": 0},
                "da": []
            }
        }))
        .expect("deserializes; from_api validates the range, not serde");
        let error = Profile::from_api(&api).expect_err("negative disable_ttl");
        assert!(error.message.contains("disable_ttl=-5"));
    }

    #[test]
    fn da_non_empty_array_fails_to_deserialize() {
        let result: Result<ApiProfile, _> = serde_json::from_value(serde_json::json!({
            "PK": "pk1", "name": "X", "updated": 0,
            "profile": {
                "flt": {"count": 0}, "rule": {"count": 0}, "svc": {"count": 0},
                "grp": {"count": 0}, "opt": {"count": 0},
                "da": [1]
            }
        }));
        assert!(
            result.is_err(),
            "a non-empty da array is not the []-when-unset shape"
        );
    }

    #[test]
    fn da_string_fails_to_deserialize() {
        let result: Result<ApiProfile, _> = serde_json::from_value(serde_json::json!({
            "PK": "pk1", "name": "X", "updated": 0,
            "profile": {
                "flt": {"count": 0}, "rule": {"count": 0}, "svc": {"count": 0},
                "grp": {"count": 0}, "opt": {"count": 0},
                "da": "bypass"
            }
        }));
        assert!(
            result.is_err(),
            "da must be an action object or []; a string is neither"
        );
    }
}
