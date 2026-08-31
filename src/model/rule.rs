//! `/profiles/{id}/rules` wire shape and the normalized rule (D2,
//! commands.md#rule). Unlike a folder, a rule's `action.do` is never
//! optional: a missing `do` is a hard error, never coerced to BLOCK
//! (read-verification.md, write-verification.md).

use serde::{Deserialize, Serialize};

use super::action::{Action, ApiAction};
use super::folder::ApiFolder;
use crate::error::Error;

/// One entry of `body.rules[]`. `group: 0` is the "no folder" sentinel, not a
/// listable folder id (read-verification.md).
#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiRule {
    #[serde(rename = "PK")]
    pub pk: String,
    pub order: i64,
    pub group: i64,
    pub action: ApiAction,
    /// Top-level sibling of `action`, absent when unset (probed 2026-08-31,
    /// write-verification.md).
    pub comment: Option<String>,
}

/// The normalized rule — the documented JSON schema, stable key set.
#[derive(Debug, Clone, Serialize)]
pub struct Rule {
    pub hostname: String,
    pub action: Action,
    pub via: Option<String>,
    pub via6: Option<String>,
    pub enabled: bool,
    /// The resolved display name, informational only — folder names are not
    /// unique, so nothing may key off it (commands.md#rule).
    pub folder: Option<String>,
    pub folder_id: Option<i64>,
    pub order: i64,
    pub comment: Option<String>,
}

impl Rule {
    /// The struct's serialized top-level key set, in order — the mutating
    /// `rule create`/`update` handlers' upfront `--fields` check
    /// (`output::validate_fields`) validates against exactly this, before
    /// anything is written. `rule_fields_matches_the_serialized_key_set`
    /// (below) is the drift guard: it fails the moment a field is added,
    /// renamed, or removed here without a matching edit to this list.
    pub const FIELDS: &'static [&'static str] = &[
        "hostname",
        "action",
        "via",
        "via6",
        "enabled",
        "folder",
        "folder_id",
        "order",
        "comment",
    ];

    /// `folders` supplies the display name for `folder_id`; a `group` with
    /// no matching folder (deleted concurrently) yields `folder: null`, not
    /// an error.
    pub fn from_api(api: &ApiRule, folders: &[ApiFolder]) -> Result<Self, Error> {
        // A missing `do` is an error, never a default — the worst possible
        // failure for a DNS tool. A folder may legally be action-less; a
        // rule never is.
        let Some(action) = api.action.action()? else {
            return Err(crate::error::upstream_shape(format_args!(
                "rule {:?}: missing action.do (a rule always carries an action)",
                api.pk
            )));
        };
        let enabled = api.action.enabled()?;
        let folder_id = (api.group != 0).then_some(api.group);
        let folder = folder_id.and_then(|id| {
            folders
                .iter()
                .find(|folder| folder.pk == id)
                .map(|folder| folder.name.clone())
        });
        Ok(Self {
            hostname: api.pk.clone(),
            action,
            via: api.action.via.clone(),
            via6: api.action.via_v6.clone(),
            enabled,
            folder,
            folder_id,
            order: api.order,
            // Reads back absent when unset (probed); an empty string still
            // normalizes to null so consumers see one shape.
            comment: api.comment.clone().filter(|comment| !comment.is_empty()),
        })
    }
}

/// Shared by every test in this crate that needs `Vec<ApiRule>` from a
/// captured fixture.
#[cfg(test)]
pub(crate) fn rules_fixture(name: &str) -> Vec<ApiRule> {
    super::fixture_list(name, "rules")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Exit;
    use crate::model::folder::folders_fixture;

    /// Drift guard for [`Rule::FIELDS`]: a field added, renamed, or removed
    /// on the struct without a matching edit to `FIELDS` fails here.
    #[test]
    fn rule_fields_matches_the_serialized_key_set() {
        let rule = Rule {
            hostname: "a.example.com".into(),
            action: Action::Block,
            via: None,
            via6: None,
            enabled: true,
            folder: None,
            folder_id: None,
            order: 1,
            comment: None,
        };
        let value = serde_json::to_value(&rule).expect("serializes");
        let keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, Rule::FIELDS);
    }

    #[test]
    fn group_zero_normalizes_to_no_folder() {
        let api: ApiRule = serde_json::from_value(serde_json::json!({
            "PK": "a.example.com", "order": 1, "group": 0,
            "action": {"do": 0, "status": 1}
        }))
        .expect("deserializes");
        let rule = Rule::from_api(&api, &[]).expect("normalizes");
        assert_eq!(rule.folder_id, None);
        assert_eq!(rule.folder, None);
    }

    #[test]
    fn a_missing_do_is_an_error_never_a_default() {
        let api: ApiRule = serde_json::from_value(serde_json::json!({
            "PK": "a.example.com", "order": 1, "group": 0,
            "action": {"status": 1}
        }))
        .expect("deserializes; from_api validates the value, not serde");
        let error = Rule::from_api(&api, &[]).expect_err("missing do");
        assert_eq!(error.exit(), Exit::Retryable);
        assert!(error.message.contains("missing action.do"));
    }

    #[test]
    fn via_and_via6_pass_through_for_a_spoof_rule() {
        let api: ApiRule = serde_json::from_value(serde_json::json!({
            "PK": "spoof.example.com", "order": 2, "group": 0,
            "action": {"do": 2, "status": 1, "via": "192.0.2.10", "via_v6": "2001:db8::1"}
        }))
        .expect("deserializes");
        let rule = Rule::from_api(&api, &[]).expect("normalizes");
        assert_eq!(rule.action, Action::Spoof);
        assert_eq!(rule.via.as_deref(), Some("192.0.2.10"));
        assert_eq!(rule.via6.as_deref(), Some("2001:db8::1"));
    }

    #[test]
    fn a_folder_id_resolves_its_display_name() {
        let folders = folders_fixture("p_groups_full.json");
        let api: ApiRule = serde_json::from_value(serde_json::json!({
            "PK": "a.example.com", "order": 1, "group": 2,
            "action": {"do": 1, "status": 1}
        }))
        .expect("deserializes");
        let rule = Rule::from_api(&api, &folders).expect("normalizes");
        assert_eq!(rule.folder_id, Some(2));
        assert_eq!(rule.folder.as_deref(), Some("Spoofed"));
    }

    #[test]
    fn an_unknown_folder_id_yields_a_null_folder_not_an_error() {
        let api: ApiRule = serde_json::from_value(serde_json::json!({
            "PK": "a.example.com", "order": 1, "group": 999,
            "action": {"do": 1, "status": 1}
        }))
        .expect("deserializes");
        let rule = Rule::from_api(&api, &[]).expect("deleted-concurrently folder is not fatal");
        assert_eq!(rule.folder_id, Some(999));
        assert_eq!(rule.folder, None);
    }

    #[test]
    fn a_disabled_rule_normalizes() {
        let api: ApiRule = serde_json::from_value(serde_json::json!({
            "PK": "a.example.com", "order": 1, "group": 0,
            "action": {"do": 2, "status": 0, "via": "192.0.2.10"}
        }))
        .expect("deserializes");
        let rule = Rule::from_api(&api, &[]).expect("normalizes");
        assert!(!rule.enabled);
    }

    #[test]
    fn a_comment_passes_through_and_an_empty_one_normalizes_to_null() {
        let with = |comment: &str| {
            serde_json::from_value::<ApiRule>(serde_json::json!({
                "PK": "a.example.com", "order": 1, "group": 0,
                "action": {"do": 0, "status": 1}, "comment": comment
            }))
            .expect("deserializes")
        };
        let rule = Rule::from_api(&with("probe comment"), &[]).expect("normalizes");
        assert_eq!(rule.comment.as_deref(), Some("probe comment"));
        let rule = Rule::from_api(&with(""), &[]).expect("normalizes");
        assert_eq!(rule.comment, None);
    }

    /// Deserialization only — no sorting happens here (`Rule::from_api` and
    /// `rule list`'s `sort_by_key` are what actually order by `order`; this
    /// fixture is deliberately *not* fully order-sorted, so a caller that
    /// assumed this list came out ordered would be wrong).
    #[test]
    fn the_captured_list_deserializes_strictly() {
        let rules = rules_fixture("rules_nofolder.json");
        assert_eq!(rules.len(), 8);
        assert_eq!(rules[0].pk, "host1.example.com");
    }
}
