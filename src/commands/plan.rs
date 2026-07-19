//! Dry-run plan output (commands.md#dry-run): normalized intent, never wire
//! bytes — action names and booleans (D2/D10); encoding integers (`do`,
//! `status`) never appear here either (identity integers like a folder id
//! do). `requests` lists only the withheld mutations, in send order;
//! validation GETs run normally and are never listed.

use serde::Serialize;
use serde_json::Value;

use super::action_flags::ActionSpec;
use crate::cli::Globals;
use crate::error::Error;
use crate::model::action::Action;
use crate::output::{emit, print_doc, print_key_values};

/// The `-n, --dry-run` flag (the clig.dev standard flag), flattened into
/// typed remote-mutation commands (commands.md#dry-run).
#[derive(Debug, clap::Args)]
pub struct DryRun {
    /// Print the planned mutation without writing anything
    #[arg(short = 'n', long)]
    pub dry_run: bool,
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub requests: Vec<PlannedRequest>,
}

#[derive(Debug, Serialize)]
pub struct PlannedRequest {
    pub method: &'static str,
    pub path: String,
    /// Key order here is contractual (commands.md#dry-run) and rides on
    /// `serde_json`'s `preserve_order` feature (Cargo.toml) — that feature
    /// flag is load-bearing.
    pub intent: Value,
}

/// A create's intent: every key always present, `null` meaning "will not be
/// sent" (a create has nothing to preserve, so that's unambiguous here).
#[derive(Debug, Serialize)]
pub struct FolderCreateIntent {
    pub name: String,
    pub action: Option<Action>,
    pub via: Option<String>,
    pub enabled: bool,
}

/// `RuleUpdateChanges::folder_id`'s patch value: move to a folder, or back to
/// root. Its own type so `Some(None)` vs `None` — a transposition that
/// compiles either way — can't invert `--root` into a no-op or a
/// yank-everything-to-root; the read-back verification would then confirm
/// the transposed intent instead of the user's flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderPatch {
    Root,
    To(i64),
}

impl Serialize for FolderPatch {
    /// `Root` -> `null`, `To(id)` -> the id — the same present-but-null vs
    /// present-with-an-id shape the old `Option<Option<i64>>` produced.
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        match self {
            Self::Root => serializer.serialize_none(),
            Self::To(id) => serializer.serialize_i64(*id),
        }
    }
}

/// A merge-style update's patch: presence is meaningful, omitted fields are
/// preserved by the server's verified merge (write-verification.md).
#[derive(Debug, Serialize, Default)]
pub struct FolderUpdateChanges {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
}

#[derive(Debug, Serialize)]
pub struct FolderUpdateIntent {
    pub changes: FolderUpdateChanges,
}

/// A delete's resolved target plus what the deletion cascades to — a DELETE
/// sends no body, so this is what would be removed, normalized.
#[derive(Debug, Serialize)]
pub struct FolderDeleteIntent {
    pub id: i64,
    pub name: String,
    pub rules: u64,
}

/// `rule create`'s intent: every key always present, `null` meaning "will
/// not be sent" — `enabled: true` is the CLI-owned default (commands.md#rule).
#[derive(Debug, Serialize)]
pub struct RuleCreateIntent {
    pub hostnames: Vec<String>,
    pub action: Action,
    pub via: Option<String>,
    pub via6: Option<String>,
    pub enabled: bool,
    pub folder_id: Option<i64>,
}

/// `rule update`'s sparse patch — presence is meaningful, omitted fields are
/// preserved by `PUT /rules`'s verified merge. `folder_id` is `None` when
/// untouched (the key is skipped) and `Some(`[`FolderPatch`]`)` otherwise —
/// `via6: null` never appears: only the *clearing* spelling (`--via6=`) is
/// rejected before a patch is ever built, a concrete `--via6 <value>` still
/// flows through (commands.md#rule).
#[derive(Debug, Serialize, Default)]
pub struct RuleUpdateChanges {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<Action>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub via6: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder_id: Option<FolderPatch>,
}

impl RuleUpdateChanges {
    /// Built from a validated [`ActionSpec`] plus the resolved folder
    /// change — `rule update` and `verify_create`'s converge-with-`update`
    /// retry both need exactly this patch.
    pub fn from_spec(spec: &ActionSpec, folder_id: Option<FolderPatch>) -> Self {
        Self {
            action: spec.action,
            via: spec.via.clone(),
            via6: spec.via6.clone(),
            enabled: spec.enabled,
            folder_id,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct RuleUpdateIntent {
    pub hostnames: Vec<String>,
    pub changes: RuleUpdateChanges,
}

/// `rule delete` plans **one request per hostname** (commands.md#rule); each
/// hostname gets its own [`PlannedRequest`] carrying this intent.
#[derive(Debug, Serialize)]
pub struct RuleDeleteIntent {
    pub hostname: String,
}

/// Print the plan and let the caller `return print(...)` — dry runs always
/// exit 0 (a `--fields` typo is already caught upfront, before the plan is
/// even built, and `--fields` together with `--dry-run` is rejected outright
/// as a conflict — `commands::preflight_fields` — so `globals.fields`
/// is always `None` by the time a handler reaches here), and confirmation is
/// skipped (nothing mutates).
pub fn print(globals: &Globals, plan: &Plan) -> Result<(), Error> {
    emit(globals.mode, globals.fields.as_deref(), plan, || {
        for request in &plan.requests {
            print_doc(&format!("would send: {} {}", request.method, request.path))?;
            render_intent(&request.intent)?;
        }
        Ok(())
    })
}

/// [`print`] for the common case of a single planned request.
pub fn print_one(
    globals: &Globals,
    method: &'static str,
    path: String,
    intent: &impl Serialize,
) -> Result<(), Error> {
    print(
        globals,
        &Plan {
            requests: vec![PlannedRequest {
                method,
                path,
                intent: serde_json::to_value(intent).expect("intent serializes"),
            }],
        },
    )
}

fn render_intent(intent: &Value) -> Result<(), Error> {
    let Value::Object(map) = intent else {
        // Unreachable today (every intent type is an object), but a dry-run
        // renderer for DNS mutations must never print nothing for a payload
        // the user never saw.
        return print_doc(&render_value(intent));
    };
    let pairs: Vec<(&str, String)> = map
        .iter()
        .map(|(key, value)| (key.as_str(), render_value(value)))
        .collect();
    print_key_values(&pairs)
}

fn render_value(value: &Value) -> String {
    match value {
        Value::Null => "null".to_owned(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .map(render_value)
            .collect::<Vec<_>>()
            .join(", "),
        Value::Object(map) => map
            .iter()
            .map(|(key, value)| format!("{key}: {}", render_value(value)))
            .collect::<Vec<_>>()
            .join(", "),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folder_create_intent_always_carries_all_four_keys() {
        let intent = FolderCreateIntent {
            name: "Ads".into(),
            action: None,
            via: None,
            enabled: true,
        };
        let value = serde_json::to_value(&intent).expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({"name": "Ads", "action": null, "via": null, "enabled": true})
        );
    }

    #[test]
    fn folder_update_changes_are_sparse() {
        let intent = FolderUpdateIntent {
            changes: FolderUpdateChanges {
                action: Some(Action::Block),
                ..Default::default()
            },
        };
        let value = serde_json::to_value(&intent).expect("serializes");
        assert_eq!(value, serde_json::json!({"changes": {"action": "block"}}));
    }

    #[test]
    fn folder_delete_intent_matches_the_documented_keys() {
        let intent = FolderDeleteIntent {
            id: 2,
            name: "Ads".into(),
            rules: 4,
        };
        let value = serde_json::to_value(&intent).expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({"id": 2, "name": "Ads", "rules": 4})
        );
    }

    #[test]
    fn rule_create_intent_always_carries_every_key() {
        let intent = RuleCreateIntent {
            hostnames: vec!["a.com".into(), "b.com".into()],
            action: Action::Block,
            via: None,
            via6: None,
            enabled: true,
            folder_id: None,
        };
        let value = serde_json::to_value(&intent).expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({
                "hostnames": ["a.com", "b.com"],
                "action": "block",
                "via": null,
                "via6": null,
                "enabled": true,
                "folder_id": null
            })
        );
    }

    #[test]
    fn rule_update_changes_are_sparse() {
        let intent = RuleUpdateIntent {
            hostnames: vec!["x.com".into()],
            changes: RuleUpdateChanges {
                enabled: Some(false),
                ..Default::default()
            },
        };
        let value = serde_json::to_value(&intent).expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({"hostnames": ["x.com"], "changes": {"enabled": false}})
        );
    }

    #[test]
    fn rule_update_root_encodes_folder_id_as_present_but_null() {
        let intent = RuleUpdateIntent {
            hostnames: vec!["x.com".into()],
            changes: RuleUpdateChanges {
                folder_id: Some(FolderPatch::Root),
                ..Default::default()
            },
        };
        let value = serde_json::to_value(&intent).expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({"hostnames": ["x.com"], "changes": {"folder_id": null}})
        );

        // Untouched: the key disappears entirely, distinct from `--root`'s null.
        let untouched = RuleUpdateIntent {
            hostnames: vec!["x.com".into()],
            changes: RuleUpdateChanges::default(),
        };
        let value = serde_json::to_value(&untouched).expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({"hostnames": ["x.com"], "changes": {}})
        );
    }

    #[test]
    fn rule_delete_intent_matches_the_documented_keys() {
        let intent = RuleDeleteIntent {
            hostname: "a.com".into(),
        };
        let value = serde_json::to_value(&intent).expect("serializes");
        assert_eq!(value, serde_json::json!({"hostname": "a.com"}));
    }
}
