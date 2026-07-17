//! `/profiles/{id}/groups` wire shape and the normalized folder (D2,
//! commands.md#folder-api-groups). `id` is the one integer that *is* the
//! identity here, not an encoding (D10) — the `{folder}` path segment must be
//! this integer PK; the folder name 404s upstream (write-verification.md).

use serde::{Deserialize, Serialize};

use super::action::{Action, ApiAction};
use crate::error::Error;

/// One entry of `body.groups[]`. `action` may be `null`/absent-`do` — a
/// legal action-less folder, distinct from block (write-verification.md).
#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiFolder {
    #[serde(rename = "PK")]
    pub pk: i64,
    #[serde(rename = "group")]
    pub name: String,
    pub action: Option<ApiAction>,
    pub count: u64,
}

/// The normalized folder — the documented JSON schema, stable key set.
#[derive(Debug, Serialize)]
pub struct Folder {
    pub id: i64,
    pub name: String,
    pub action: Option<Action>,
    pub via: Option<String>,
    pub enabled: bool,
    pub rules: u64,
}

impl Folder {
    /// The struct's serialized top-level key set, in order — the mutating
    /// `folder create`/`update` handlers' upfront `--fields` check
    /// (`output::validate_fields`) validates against exactly this, before
    /// anything is written. `folder_fields_matches_the_serialized_key_set`
    /// (below) is the drift guard: it fails the moment a field is added,
    /// renamed, or removed here without a matching edit to this list.
    pub const FIELDS: &'static [&'static str] =
        &["id", "name", "action", "via", "enabled", "rules"];

    pub fn from_api(api: &ApiFolder) -> Result<Self, Error> {
        // No action object at all, or an action object with no `do`: both
        // are the action-less state, never coerced to block; absence of
        // `status` reads as enabled, same rationale as `ApiAction::enabled`.
        // A `via` riding an action-less object is a shape upstream cannot
        // legally produce (write-verification.md), so it surfaces as an
        // error rather than being silently dropped.
        let (action, via, enabled) = match &api.action {
            None => (None, None, true),
            Some(wire_action) => {
                let action = wire_action.action()?;
                let via = if action.is_some() {
                    wire_action.via.clone()
                } else if let Some(via) = &wire_action.via {
                    return Err(crate::error::upstream_shape(format_args!(
                        "folder {:?}: via {via:?} with no action",
                        api.name
                    )));
                } else {
                    None
                };
                (action, via, wire_action.enabled()?)
            }
        };
        Ok(Self {
            id: api.pk,
            name: api.name.clone(),
            action,
            via,
            enabled,
            rules: api.count,
        })
    }
}

/// Shared by every test in this crate that needs `Vec<ApiFolder>` from a
/// captured fixture (this module and `commands::scope`).
#[cfg(test)]
pub(crate) fn folders_fixture(name: &str) -> Vec<ApiFolder> {
    super::fixture_list(name, "groups")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Exit;

    /// Drift guard for [`Folder::FIELDS`]: a field added, renamed, or
    /// removed on the struct without a matching edit to `FIELDS` fails here.
    #[test]
    fn folder_fields_matches_the_serialized_key_set() {
        let folder = Folder {
            id: 1,
            name: "Ads".into(),
            action: None,
            via: None,
            enabled: true,
            rules: 0,
        };
        let value = serde_json::to_value(&folder).expect("serializes");
        let keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, Folder::FIELDS);
    }

    #[test]
    fn an_action_less_folder_normalizes_to_null_and_enabled() {
        let api: ApiFolder = serde_json::from_value(serde_json::json!({
            "PK": 1, "group": "Noaction", "action": {"status": 1}, "count": 3
        }))
        .expect("deserializes");
        let folder = Folder::from_api(&api).expect("normalizes");
        assert_eq!(folder.id, 1);
        assert_eq!(folder.action, None);
        assert_eq!(folder.via, None);
        assert!(folder.enabled);
        assert_eq!(folder.rules, 3);
    }

    #[test]
    fn a_missing_action_object_is_also_action_less_and_enabled() {
        let api: ApiFolder = serde_json::from_value(serde_json::json!({
            "PK": 1, "group": "Noaction", "action": null, "count": 0
        }))
        .expect("deserializes");
        let folder = Folder::from_api(&api).expect("normalizes");
        assert_eq!(folder.action, None);
        assert!(folder.enabled);
    }

    #[test]
    fn a_spoof_folder_carries_its_via() {
        let api: ApiFolder = serde_json::from_value(serde_json::json!({
            "PK": 2, "group": "Spoofed", "action": {"status": 1, "do": 2, "via": "192.0.2.53"}, "count": 5
        }))
        .expect("deserializes");
        let folder = Folder::from_api(&api).expect("normalizes");
        assert_eq!(folder.action, Some(Action::Spoof));
        assert_eq!(folder.via.as_deref(), Some("192.0.2.53"));
        assert!(folder.enabled);
    }

    #[test]
    fn a_disabled_block_folder_normalizes() {
        let api: ApiFolder = serde_json::from_value(serde_json::json!({
            "PK": 3, "group": "Blocked", "action": {"status": 0, "do": 0}, "count": 2
        }))
        .expect("deserializes");
        let folder = Folder::from_api(&api).expect("normalizes");
        assert_eq!(folder.action, Some(Action::Block));
        assert!(!folder.enabled);
    }

    #[test]
    fn a_via_with_no_action_is_a_shape_error() {
        let api: ApiFolder = serde_json::from_value(serde_json::json!({
            "PK": 6, "group": "Weird", "action": {"status": 1, "via": "192.0.2.53"}, "count": 0
        }))
        .expect("deserializes; from_api validates the value, not serde");
        let error = Folder::from_api(&api).expect_err("via with no do is a shape error");
        assert_eq!(error.exit(), Exit::Retryable);
    }

    #[test]
    fn an_unknown_do_is_a_shape_error() {
        let api: ApiFolder = serde_json::from_value(serde_json::json!({
            "PK": 4, "group": "Weird", "action": {"status": 1, "do": 9}, "count": 0
        }))
        .expect("deserializes; from_api validates the value, not serde");
        let error = Folder::from_api(&api).expect_err("unknown do");
        assert_eq!(error.exit(), Exit::Retryable);
    }

    #[test]
    fn the_captured_list_deserializes_strictly() {
        let folders = folders_fixture("p_groups_full.json");
        assert!(folders.len() >= 5);
    }
}
