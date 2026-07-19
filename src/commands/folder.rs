//! `cdctl folder` — a profile's rule folders (API: "groups",
//! commands.md#folder-api-groups). Folder names are **not unique**; the
//! `{folder}` path segment is always the integer PK — a name in the path
//! 404s upstream, so name resolution stays strictly client-side
//! (`commands::scope`, shared with rule commands).

use clap::Subcommand;
use reqwest::Method;

use super::action_flags::{ActionFlags, Caps};
use super::confirm::confirm;
use super::plan::{
    self, DryRun, FolderCreateIntent, FolderDeleteIntent, FolderUpdateChanges, FolderUpdateIntent,
};
use crate::cli::Globals;
use crate::error::Error;
use crate::model::folder::{ApiFolder, Folder};
use crate::output::{self, emit, escape_controls, print_doc, print_key_values, render_table};

#[derive(Debug, Subcommand)]
pub enum FolderCommand {
    /// List a profile's folders
    List,
    /// Create a folder (omitting --action makes an action-less folder)
    Create {
        /// Folder name
        name: String,
        #[command(flatten)]
        action: ActionFlags,
        #[command(flatten)]
        dry_run: DryRun,
    },
    /// Rename a folder or change its action (sends only the flags given)
    Update {
        /// Folder id or name (case-insensitive, an ambiguous name is an
        /// error)
        selector: String,
        /// New name
        #[arg(long)]
        name: Option<String>,
        #[command(flatten)]
        action: ActionFlags,
        #[command(flatten)]
        dry_run: DryRun,
    },
    /// Delete a folder and its contained rules
    Delete {
        /// Folder id or name (case-insensitive, an ambiguous name is an
        /// error)
        selector: String,
        #[command(flatten)]
        dry_run: DryRun,
    },
}

pub async fn run(command: FolderCommand, globals: &Globals) -> Result<(), Error> {
    match command {
        FolderCommand::List => list(globals).await,
        FolderCommand::Create {
            name,
            action,
            dry_run,
        } => create(&name, &action, dry_run.dry_run, globals).await,
        FolderCommand::Update {
            selector,
            name,
            action,
            dry_run,
        } => update(&selector, name, &action, dry_run.dry_run, globals).await,
        FolderCommand::Delete { selector, dry_run } => {
            delete(&selector, dry_run.dry_run, globals).await
        }
    }
}

async fn list(globals: &Globals) -> Result<(), Error> {
    output::validate_fields(globals.fields.as_deref(), Folder::FIELDS)?;
    let (client, _source, config) = super::authenticated_client(globals)?;
    let scope = super::scope::resolve_profile(&client, globals, config.default_profile()).await?;

    let api_folders = super::scope::fetch_folders(&client, &scope.id).await?;
    let folders = api_folders
        .iter()
        .map(Folder::from_api)
        .collect::<Result<Vec<_>, _>>()?;

    emit(globals.mode, globals.fields.as_deref(), &folders, || {
        let rows = folders
            .iter()
            .map(|folder| {
                vec![
                    folder.name.clone(),
                    folder.id.to_string(),
                    folder.rules.to_string(),
                    folder
                        .action
                        .map_or_else(|| "-".to_owned(), |a| a.to_string()),
                    folder.enabled.to_string(),
                ]
            })
            .collect();
        print_doc(
            &render_table(
                &["NAME", "ID", "RULES", "ACTION", "ENABLED"],
                rows,
                globals.plain,
            )
            .to_string(),
        )
    })
}

async fn create(
    name: &str,
    flags: &ActionFlags,
    dry_run: bool,
    globals: &Globals,
) -> Result<(), Error> {
    super::validate::reject_control_chars(name, "the folder name")?;
    super::preflight_fields(globals, Folder::FIELDS, dry_run)?;
    let (spec, client, _config, scope) = super::validated_client(
        flags,
        Caps {
            via6_allowed: false,
            action_required: false,
            default_enabled: true,
        },
        globals,
    )
    .await?;

    let path = super::scope::groups_path(&scope.id);
    if dry_run {
        let intent = FolderCreateIntent {
            name: name.to_owned(),
            action: spec.action,
            via: spec.via.clone(),
            enabled: spec.enabled.expect("create caps default enabled"),
        };
        return plan::print_one(globals, "POST", path, &intent);
    }

    let mut form: Vec<(&str, String)> = vec![("name", name.to_owned())];
    form.extend(spec.form_pairs());

    let envelope = client.write(Method::POST, &path, &form, "folder").await?;
    let folder = single_folder_from(envelope, "create")
        .map_err(|e| super::landed_write_unverified(e, "folder"))?;
    print_folder(globals, &folder)
}

async fn update(
    selector: &str,
    name: Option<String>,
    flags: &ActionFlags,
    dry_run: bool,
    globals: &Globals,
) -> Result<(), Error> {
    super::validate::reject_control_chars(selector, "the folder selector")?;
    if let Some(new_name) = &name {
        super::validate::reject_control_chars(new_name, "the new folder name")?;
    }
    if name.is_none() && flags.is_empty() {
        return Err(Error::usage(
            "folder update needs at least one change: --name or an action flag",
        ));
    }
    super::preflight_fields(globals, Folder::FIELDS, dry_run)?;
    let (spec, client, _config, scope) = super::validated_client(
        flags,
        Caps {
            via6_allowed: false,
            action_required: false,
            default_enabled: false,
        },
        globals,
    )
    .await?;
    let api_folders = super::scope::fetch_folders(&client, &scope.id).await?;
    let folder_id = super::scope::find_folder(&api_folders, selector)?.pk;

    let path = super::scope::group_path(&scope.id, folder_id);
    if dry_run {
        let changes = FolderUpdateChanges {
            name: name.clone(),
            action: spec.action,
            via: spec.via.clone(),
            enabled: spec.enabled,
        };
        return plan::print_one(globals, "PUT", path, &FolderUpdateIntent { changes });
    }

    let mut form: Vec<(&str, String)> = Vec::new();
    if let Some(new_name) = &name {
        form.push(("name", new_name.clone()));
    }
    form.extend(spec.form_pairs());

    let envelope = client.write(Method::PUT, &path, &form, "folder").await?;
    let folder = single_folder_from(envelope, "update")
        .map_err(|e| super::landed_write_unverified(e, "folder"))?;
    print_folder(globals, &folder)
}

async fn delete(selector: &str, dry_run: bool, globals: &Globals) -> Result<(), Error> {
    super::validate::reject_control_chars(selector, "the folder selector")?;
    // Before the confirmation prompt too: a delete emits no row, but a
    // typo'd `--fields` must fail before anything is deleted.
    super::preflight_fields(globals, Folder::FIELDS, dry_run)?;
    let (client, _source, config) = super::authenticated_client(globals)?;
    let scope = super::scope::resolve_profile(&client, globals, config.default_profile()).await?;

    let api_folders = super::scope::fetch_folders(&client, &scope.id).await?;
    let folder = Folder::from_api(super::scope::find_folder(&api_folders, selector)?)?;

    let path = super::scope::group_path(&scope.id, folder.id);
    if dry_run {
        let intent = FolderDeleteIntent {
            id: folder.id,
            name: folder.name.clone(),
            rules: folder.rules,
        };
        return plan::print_one(globals, "DELETE", path, &intent);
    }

    let prompt = format!(
        "delete folder \"{}\" (id {}) and its {} contained rules from profile \"{}\"?",
        escape_controls(&folder.name),
        folder.id,
        folder.rules,
        escape_controls(&scope.name),
    );
    confirm(&prompt, globals.yes, &scope).await?;

    client.write(Method::DELETE, &path, &[], "folder").await?;
    eprintln!(
        "info: deleted folder \"{}\" (id {})",
        escape_controls(&folder.name),
        folder.id
    );
    Ok(())
}

/// `folder create`/`update` both print source **R**: the full folder object
/// arrives in `groups[]` (exactly one element) even though the response
/// reuses the list envelope key.
fn single_folder_from(
    envelope: crate::api::envelope::Envelope,
    verb: &str,
) -> Result<Folder, Error> {
    let folders: Vec<ApiFolder> = envelope.keyed_as("groups")?;
    match folders.as_slice() {
        [folder] => Folder::from_api(folder),
        other => Err(crate::error::upstream_shape(format_args!(
            "expected one folder in the {verb} response, got {}",
            other.len()
        ))),
    }
}

fn print_folder(globals: &Globals, folder: &Folder) -> Result<(), Error> {
    emit(globals.mode, globals.fields.as_deref(), folder, || {
        print_key_values(&[
            ("id", folder.id.to_string()),
            ("name", folder.name.clone()),
            (
                "action",
                folder
                    .action
                    .map_or_else(|| "-".to_owned(), |a| a.to_string()),
            ),
            ("via", folder.via.clone().unwrap_or_else(|| "-".to_owned())),
            ("enabled", folder.enabled.to_string()),
            ("rules", folder.rules.to_string()),
        ])
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::envelope::Envelope;
    use crate::error::Exit;

    fn envelope(groups: &serde_json::Value) -> Envelope {
        Envelope {
            success: Some(true),
            body: Some(serde_json::json!({"groups": groups})),
            error: None,
            message: None,
        }
    }

    #[test]
    fn single_folder_from_requires_exactly_one_element() {
        let error =
            single_folder_from(envelope(&serde_json::json!([])), "create").expect_err("empty");
        assert_eq!(error.exit(), Exit::Retryable);

        let two = envelope(&serde_json::json!([
            {"PK": 1, "group": "A", "action": null, "count": 0},
            {"PK": 2, "group": "B", "action": null, "count": 0},
        ]));
        let error = single_folder_from(two, "create").expect_err("multiple is a shape error");
        assert_eq!(error.exit(), Exit::Retryable);

        let one = envelope(&serde_json::json!([
            {"PK": 1, "group": "A", "action": null, "count": 0},
        ]));
        let folder = single_folder_from(one, "create").expect("exactly one succeeds");
        assert_eq!(folder.id, 1);
    }
}
