//! Dry-run plan output (commands.md#dry-run): normalized intent, never wire
//! bytes — action names and booleans (D2/D10); encoding integers (`do`,
//! `status`) never appear here either (identity integers like a folder id
//! do). `requests` lists only the withheld mutations, in send order;
//! validation GETs run normally and are never listed.

use serde::Serialize;
use serde_json::Value;

use crate::cli::Globals;
use crate::model::action::Action;
use crate::output::{emit, print_key_values};

/// The `-n, --dry-run` flag (the clig.dev standard flag), flattened into
/// typed remote-mutation commands (commands.md#dry-run).
#[derive(Debug, clap::Args)]
pub struct DryRun {
    /// Print the withheld mutation as a plan; write nothing
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

/// Print the plan and let the caller `return Ok(())` — dry runs always
/// exit 0, and confirmation is skipped (nothing mutates).
pub fn print(globals: &Globals, plan: &Plan) {
    emit(globals.mode, globals.fields.as_deref(), plan, || {
        for request in &plan.requests {
            println!("would send: {} {}", request.method, request.path);
            render_intent(&request.intent);
        }
    });
}

/// [`print`] for the common case of a single planned request.
pub fn print_one(globals: &Globals, method: &'static str, path: String, intent: &impl Serialize) {
    print(
        globals,
        &Plan {
            requests: vec![PlannedRequest {
                method,
                path,
                intent: serde_json::to_value(intent).expect("intent serializes"),
            }],
        },
    );
}

fn render_intent(intent: &Value) {
    let Value::Object(map) = intent else {
        // Unreachable today (every intent type is an object), but a dry-run
        // renderer for DNS mutations must never print nothing for a payload
        // the user never saw.
        println!("{}", render_value(intent));
        return;
    };
    let pairs: Vec<(&str, String)> = map
        .iter()
        .map(|(key, value)| (key.as_str(), render_value(value)))
        .collect();
    print_key_values(&pairs);
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
}
