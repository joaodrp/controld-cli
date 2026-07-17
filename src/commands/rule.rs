//! `cdctl rule` — a profile's DNS rules (commands.md#rule). Hostnames are
//! profile-unique and canonicalized client-side (lowercased, one trailing
//! dot stripped, deduplicated) before any request — a duplicate inside a
//! `POST` chunk fails the whole chunk upstream
//! (commands.md#rule-import-semantics), so `cdctl` never sends one.
//! `create`/`update` are read back and verified against the desired state
//! before anything is printed: the write response is a hostname-less
//! one-entry summary that cannot reveal a dropped hostname (the
//! ~1001-form-var silent truncation, write-verification.md). The read-back
//! is always profile-wide — the root listing unioned with one `GET` per
//! folder ([`fetch_all_rules`]) — never just the root: the root listing
//! alone omits every foldered rule outright (read-verification.md "Listing
//! root rules"), which would otherwise misreport a landed-in-a-folder write
//! as dropped. A *retryable*
//! `client.write` failure (timeout, 5xx, 429) does not propagate blind
//! either: it resolves through that same read-back — full convergence is
//! still success (with an info line explaining why), an all-absent read-back
//! returns the original error verbatim (the retry exit 8 invites is now
//! proven safe), and anything else attributes the still-missing targets to
//! the write error itself (`resolve_ambiguous_write`). `delete` gets per-request
//! outcomes instead — one `DELETE` per hostname, no read-back (D11): a
//! retryable `DELETE` failure is genuinely ambiguous (it may have landed),
//! so the aggregate hint leads with re-fetching state instead of advertising
//! a blind `retry_argv` replay that could 404 on an already-gone rule.
//! [`super::multi::aggregate`] turns any gap or failure into one D4
//! `write.partial_failure` error.

use clap::Subcommand;
use futures_util::future::try_join_all;
use reqwest::Method;

use super::action_flags::{self, ActionFlags, ActionSpec, Caps};
use super::confirm::confirm;
use super::multi::{self, TargetResult};
use super::plan::{
    self, DryRun, FolderPatch, Plan, PlannedRequest, RuleCreateIntent, RuleDeleteIntent,
    RuleUpdateChanges, RuleUpdateIntent,
};
use super::scope::ProfileScope;
use crate::api::client::{Client, encode_path_segment};
use crate::cli::Globals;
use crate::error::{Error, Exit};
use crate::model::folder::ApiFolder;
use crate::model::rule::{ApiRule, Rule};
use crate::output::{emit, escape_controls, render_table, validate_fields};

/// The import chunk size, comfortably inside the server's silent
/// ~1001-form-var ceiling (D11, write-verification.md) — the cap binds
/// `create`/`update` (and the still-unimplemented `rule import`); `delete`
/// is uncapped by design, since a `DELETE` sends no form variables.
const MAX_HOSTNAMES: usize = 500;

/// Segment-less rules path (the documented `folder_id=0` 404s,
/// read-verification.md section 3). As a `GET` this lists root rules only —
/// it silently omits every rule that lives in a folder (read-verification.md
/// "Listing root rules"), so [`fetch_all_rules`] is the profile-wide union
/// callers need. As a `POST`/`PUT` it addresses hostnames profile-wide: a
/// write is never scoped to root, and moving a rule into a folder is the
/// `group` form field, not a path segment.
fn rules_path(profile_id: &str) -> String {
    format!("/profiles/{profile_id}/rules")
}

/// One folder's rules listing.
fn folder_rules_path(profile_id: &str, folder_id: i64) -> String {
    format!("/profiles/{profile_id}/rules/{folder_id}")
}

/// One rule's path. The sole caller of `encode_path_segment` for rule
/// hostnames, so a site that forgets to encode can't reintroduce the
/// `*`-in-path hazard (write-verification.md).
fn rule_path(profile_id: &str, hostname: &str) -> String {
    format!(
        "/profiles/{profile_id}/rules/{}",
        encode_path_segment(hostname)
    )
}

/// Shared by list/create/update: a given `--folder` is control-character
/// hardened before anything else runs.
fn validate_folder_selector(selector: Option<&String>) -> Result<(), Error> {
    if let Some(selector) = selector {
        super::validate::reject_control_chars(selector, "the folder selector")?;
    }
    Ok(())
}

#[derive(Debug, Subcommand)]
pub enum RuleCommand {
    /// List a profile's rules
    List {
        /// Folder id or name (omitted lists the whole profile)
        #[arg(long, value_name = "id|name")]
        folder: Option<String>,
    },
    /// Create rules
    Create {
        /// Hostnames to create (wildcards like *.example.com are legal)
        #[arg(required = true, value_name = "HOSTNAME")]
        hostnames: Vec<String>,
        #[command(flatten)]
        action: ActionFlags,
        /// Folder id or name to create the rules in
        #[arg(long, value_name = "id|name")]
        folder: Option<String>,
        #[command(flatten)]
        dry_run: DryRun,
    },
    /// Update rules (sends only the flags given — `PUT /rules` merges)
    Update {
        /// Hostnames to update
        #[arg(required = true, value_name = "HOSTNAME")]
        hostnames: Vec<String>,
        #[command(flatten)]
        action: ActionFlags,
        /// Move to this folder (id or name)
        #[arg(long, value_name = "id|name", conflicts_with = "root")]
        folder: Option<String>,
        /// Move back to the root, out of any folder
        #[arg(long)]
        root: bool,
        #[command(flatten)]
        dry_run: DryRun,
    },
    /// Delete rules
    Delete {
        /// Hostnames to delete
        #[arg(required = true, value_name = "HOSTNAME")]
        hostnames: Vec<String>,
        #[command(flatten)]
        dry_run: DryRun,
    },
}

pub async fn run(command: RuleCommand, globals: &Globals) -> Result<(), Error> {
    match command {
        RuleCommand::List { folder } => list(folder, globals).await,
        RuleCommand::Create {
            hostnames,
            action,
            folder,
            dry_run,
        } => create(&hostnames, &action, folder, dry_run.dry_run, globals).await,
        RuleCommand::Update {
            hostnames,
            action,
            folder,
            root,
            dry_run,
        } => update(&hostnames, &action, folder, root, dry_run.dry_run, globals).await,
        RuleCommand::Delete { hostnames, dry_run } => {
            delete(&hostnames, dry_run.dry_run, globals).await
        }
    }
}

async fn list(folder_selector: Option<String>, globals: &Globals) -> Result<(), Error> {
    validate_folder_selector(folder_selector.as_ref())?;
    // Upfront, before any request: `rule list`'s row shape is known
    // (`Rule::FIELDS`), so a typo'd `--fields` is a usage error even when
    // the eventual result is empty — `emit`'s own check alone would be
    // vacuous on an empty profile and let the typo through with exit 0.
    validate_fields(globals.fields.as_deref(), Rule::FIELDS)?;
    let (client, _source, config) = super::authenticated_client(globals)?;
    let scope = super::scope::resolve_profile(&client, globals, config.default_profile()).await?;

    // `api_folders` names every rule's FOLDER column/field either way
    // (commands.md#rule) and, with no `--folder` given, also drives
    // `fetch_all_rules`'s per-folder GETs — the root listing alone omits
    // every foldered rule (read-verification.md "Listing root rules").
    let api_folders = super::scope::fetch_folders(&client, &scope.id).await?;
    let api_rules: Vec<ApiRule> = if let Some(selector) = &folder_selector {
        let folder = super::scope::find_folder(&api_folders, selector)?;
        let path = folder_rules_path(&scope.id, folder.pk);
        client.get(&path, "rule").await?.keyed_as("rules")?
    } else {
        fetch_all_rules(&client, &scope.id, &api_folders).await?
    };

    let mut rules: Vec<Rule> = api_rules
        .iter()
        .map(|api_rule| Rule::from_api(api_rule, &api_folders))
        .collect::<Result<_, _>>()?;
    rules.sort_by_key(|rule| rule.order);

    print_rules(globals, &rules)
}

async fn create(
    raw_hostnames: &[String],
    flags: &ActionFlags,
    folder_selector: Option<String>,
    dry_run: bool,
    globals: &Globals,
) -> Result<(), Error> {
    validate_folder_selector(folder_selector.as_ref())?;
    let hostnames = canonicalize_hostnames(raw_hostnames, super::validate::reject_control_chars)?;
    check_hostname_cap(&hostnames)?;
    // Before anything is written or planned, live or dry-run alike:
    // `--fields` always names fields of `Rule`, the command's data schema —
    // never the dry-run plan envelope's own keys (`method`/`path`/`intent`).
    // A dry run prints the plan, not rows, so the two flags conflict outright
    // (`reject_fields_with_dry_run`, checked once the field names themselves
    // are known-good).
    validate_fields(globals.fields.as_deref(), Rule::FIELDS)?;
    super::reject_fields_with_dry_run(dry_run, globals)?;
    let (spec, client, config) = super::validated_client(
        flags,
        Caps {
            via6_allowed: true,
            action_required: true,
            default_enabled: true,
        },
        globals,
    )
    .await?;

    let scope = super::scope::resolve_profile(&client, globals, config.default_profile()).await?;

    // Fetched once, unconditionally: the dry-run intent needs `--folder`
    // resolved to a pk, and the read-back needs every folder in the profile
    // regardless of `--folder` — the created rule could land in the wrong
    // folder (or root) rather than the desired one, and distinguishing that
    // `state_mismatch` from a plain `write_dropped` absence needs to see it
    // wherever it landed, not just the target folder (module doc).
    let api_folders = super::scope::fetch_folders(&client, &scope.id).await?;
    let folder_id = match &folder_selector {
        Some(selector) => Some(super::scope::find_folder(&api_folders, selector)?.pk),
        None => None,
    };

    let path = rules_path(&scope.id);
    if dry_run {
        let intent = RuleCreateIntent {
            hostnames: hostnames.clone(),
            action: spec.action.expect("create caps require an action"),
            via: spec.via.clone(),
            via6: spec.via6.clone(),
            enabled: spec.enabled.expect("create caps default enabled"),
            folder_id,
        };
        return plan::print_one(globals, "POST", path, &intent);
    }

    let mut form: Vec<(&str, String)> = spec.form_pairs();
    if let Some(id) = folder_id {
        form.push(("group", id.to_string()));
    }
    for hostname in &hostnames {
        form.push(("hostnames[]", hostname.clone()));
    }

    // The 2xx envelope's `body` is a hostname-less one-entry summary; only
    // its `success` is meaningful here (write-verification.md).
    match client.write(Method::POST, &path, &form, "rule").await {
        Ok(_envelope) => {
            verify_create(
                &client,
                &scope,
                &hostnames,
                &spec,
                folder_id,
                &api_folders,
                globals,
                None,
            )
            .await
        }
        // Terminal: the server rejected the write outright, so nothing landed.
        Err(e) if !e.retryable() => Err(e),
        // Retryable and therefore ambiguous (D12's "may still have landed"
        // hint) — resolve it with the same read-back the success path runs,
        // rather than guessing (module doc).
        Err(e) => {
            verify_create(
                &client,
                &scope,
                &hostnames,
                &spec,
                folder_id,
                &api_folders,
                globals,
                Some(&e),
            )
            .await
        }
    }
}

/// The mandatory read-back verification for `rule create`: re-fetch the
/// whole profile and assert every target landed with the intended action,
/// enabled state, `via`, `via6`, and folder before printing anything
/// (commands.md#rule). `write_error` is `Some` only when `client.write`
/// itself returned a *retryable* error — this same read-back then also
/// resolves whether that write landed (module doc,
/// [`resolve_ambiguous_write`]); `None` is the ordinary
/// read-back-after-a-successful-write path, unchanged.
#[expect(
    clippy::too_many_arguments,
    reason = "one write-error param past verify_update's 7; splitting the rest into a struct \
              would just move the coupling, not reduce it"
)]
async fn verify_create(
    client: &Client,
    scope: &ProfileScope,
    hostnames: &[String],
    spec: &ActionSpec,
    folder_id: Option<i64>,
    api_folders: &[ApiFolder],
    globals: &Globals,
    write_error: Option<&Error>,
) -> Result<(), Error> {
    let api_rules = read_back_for_verification(client, &scope.id, api_folders, write_error).await?;
    let mut reconciled = reconcile(
        hostnames,
        &api_rules,
        api_folders,
        |rule| create_matches(rule, spec, folder_id),
        Exit::Generic,
    )?;

    if reconciled.absent.is_empty() && reconciled.mismatched.is_empty() {
        print_rules(globals, &reconciled.landed)?;
        announce_if_ambiguous_write_landed(write_error);
        return Ok(());
    }
    if let Some(original) = resolve_ambiguous_write(&mut reconciled, write_error) {
        return Err(original);
    }
    let Reconciled {
        results,
        absent,
        mismatched,
        ..
    } = reconciled;

    // A live `via_v6` the desired spec omits can never be cleared by a
    // `rule update` retry — `PUT /rules` preserves an omitted `via_v6`
    // (write-verification.md "via_v6 cannot be cleared") — so a retry built
    // from `spec` would converge everything else and report the same
    // mismatch forever. Split those out before building the retry: they
    // never enter an update-verb `retry_argv`, only the hint below.
    let (unconvergeable, convergeable): (Vec<MismatchedTarget>, Vec<MismatchedTarget>) = mismatched
        .into_iter()
        .partition(|(_, live)| spec.via6.is_none() && live.via6.is_some());
    let unconvergeable: Vec<String> = unconvergeable.into_iter().map(|(h, _)| h).collect();
    let convergeable: Vec<String> = convergeable.into_iter().map(|(h, _)| h).collect();

    // Absent targets never landed, so resending them is safe; a convergeable
    // mismatch already landed, so retrying `create` would duplicate-POST it
    // — the remedy is `rule update`, named in the hint rather than
    // retry_argv when both kinds occur together (commands.md#rule). An
    // all-unconvergeable mismatch with nothing absent leaves no safe
    // update-verb retry at all — an empty `retry_argv` is correct; the hint
    // carries the whole remedy.
    let retry_argv = if !absent.is_empty() {
        create_retry_argv(&scope.id, spec, folder_id, &absent)
    } else if convergeable.is_empty() {
        Vec::new()
    } else {
        let folder_patch = match folder_id {
            Some(id) => FolderPatch::To(id),
            None => FolderPatch::Root,
        };
        let changes = RuleUpdateChanges::from_spec(spec, Some(folder_patch));
        update_retry_argv(&scope.id, &changes, &convergeable)
    };

    let mut error = multi::aggregate("rule", &results, retry_argv);
    if let Some(hint) = create_mismatch_hint(&absent, &convergeable, &unconvergeable) {
        error = error.with_hint(hint);
    }
    Err(error)
}

/// `verify_create`'s remedy hint once `retry_argv` alone can't carry it:
/// `convergeable` mismatches already ride `retry_argv` as a `rule update`,
/// so they're named here only alongside an absent-target `retry_argv`
/// (create-verb, which says nothing about them). `unconvergeable` mismatches
/// — an unclearable via6 (write-verification.md) — never ride `retry_argv`
/// at all, so the hint spells their only real remedy: delete + recreate.
/// `None` when there's nothing beyond what `retry_argv` already covers (no
/// absent targets and every mismatch converges).
fn create_mismatch_hint(
    absent: &[String],
    convergeable: &[String],
    unconvergeable: &[String],
) -> Option<String> {
    if convergeable.is_empty() && unconvergeable.is_empty() {
        return None;
    }
    if absent.is_empty() {
        return if unconvergeable.is_empty() {
            None
        } else {
            Some(format!(
                "{} landed with a via6 that `rule update` can never clear \u{2014} delete and \
                 recreate with the desired via6 (the rules are unprotected between the two \
                 commands)",
                unconvergeable.join(", ")
            ))
        };
    }
    Some(match (convergeable.is_empty(), unconvergeable.is_empty()) {
        (false, true) => format!(
            "retry_argv covers only the missing hostnames; {} landed with the wrong state \
             \u{2014} converge with `cdctl rule update`",
            convergeable.join(", ")
        ),
        (true, false) => format!(
            "retry_argv covers only the missing hostnames; {} landed with a via6 that `rule \
             update` can never clear \u{2014} delete and recreate with the desired via6 (the \
             rules are unprotected between the two commands)",
            unconvergeable.join(", ")
        ),
        (false, false) => format!(
            "retry_argv covers only the missing hostnames; {} landed with the wrong state \
             (converge with `cdctl rule update`), {} with an unclearable via6 (delete and \
             recreate instead)",
            convergeable.join(", "),
            unconvergeable.join(", ")
        ),
        (true, true) => unreachable!("guarded above: at least one bucket is nonempty"),
    })
}

fn create_matches(rule: &Rule, spec: &ActionSpec, folder_id: Option<i64>) -> bool {
    Some(rule.action) == spec.action
        && rule.via.as_deref() == spec.via.as_deref()
        && rule.via6.as_deref() == spec.via6.as_deref()
        && Some(rule.enabled) == spec.enabled
        && rule.folder_id == folder_id
}

async fn update(
    raw_hostnames: &[String],
    flags: &ActionFlags,
    folder_selector: Option<String>,
    root: bool,
    dry_run: bool,
    globals: &Globals,
) -> Result<(), Error> {
    validate_folder_selector(folder_selector.as_ref())?;
    let hostnames = canonicalize_hostnames(raw_hostnames, super::validate::reject_control_chars)?;
    check_hostname_cap(&hostnames)?;
    if flags.is_empty() && folder_selector.is_none() && !root {
        return Err(Error::usage(
            "rule update needs at least one change: an action flag, --folder, or --root",
        ));
    }
    // Before anything is written or planned, live or dry-run alike; see
    // `create`'s matching comment.
    validate_fields(globals.fields.as_deref(), Rule::FIELDS)?;
    super::reject_fields_with_dry_run(dry_run, globals)?;
    let (spec, client, config) = super::validated_client(
        flags,
        Caps {
            via6_allowed: true,
            action_required: false,
            default_enabled: false,
        },
        globals,
    )
    .await?;

    let scope = super::scope::resolve_profile(&client, globals, config.default_profile()).await?;

    // Fetched once, unconditionally, like `create`: a plain update's targets
    // can live in any folder, and a root-only read-back would misreport a
    // converged update as dropped (module doc) — `--folder` also resolves
    // its pk from this list, and the read-back's `folder` display must stay
    // accurate profile-wide for a rule an untouched update (or `--root`)
    // leaves inside — or moves out of — an existing folder (commands.md#rule).
    let api_folders = super::scope::fetch_folders(&client, &scope.id).await?;
    let folder_change = if root {
        Some(FolderPatch::Root)
    } else if let Some(selector) = &folder_selector {
        Some(FolderPatch::To(
            super::scope::find_folder(&api_folders, selector)?.pk,
        ))
    } else {
        None
    };

    let changes = RuleUpdateChanges::from_spec(&spec, folder_change);

    let path = rules_path(&scope.id);
    if dry_run {
        return plan::print_one(
            globals,
            "PUT",
            path,
            &RuleUpdateIntent {
                hostnames: hostnames.clone(),
                changes,
            },
        );
    }

    let mut form: Vec<(&str, String)> = spec.form_pairs();
    match folder_change {
        Some(FolderPatch::To(id)) => form.push(("group", id.to_string())),
        Some(FolderPatch::Root) => form.push(("group", "0".to_owned())),
        None => {}
    }
    for hostname in &hostnames {
        form.push(("hostnames[]", hostname.clone()));
    }

    match client.write(Method::PUT, &path, &form, "rule").await {
        Ok(_envelope) => {
            verify_update(
                &client,
                &scope,
                &hostnames,
                &changes,
                &api_folders,
                globals,
                None,
            )
            .await
        }
        // Terminal: the server rejected the write outright, so nothing landed.
        Err(e) if !e.retryable() => Err(e),
        // Retryable and therefore ambiguous — same resolution as `create`
        // (module doc).
        Err(e) => {
            verify_update(
                &client,
                &scope,
                &hostnames,
                &changes,
                &api_folders,
                globals,
                Some(&e),
            )
            .await
        }
    }
}

/// Same read-back contract as [`verify_create`], but asserts only the
/// fields the merge actually sent — `PUT /rules` preserves the rest, so
/// checking unsent fields would race a concurrent editor
/// (write-verification.md). `write_error` carries the same ambiguous-write
/// meaning as `verify_create`'s.
async fn verify_update(
    client: &Client,
    scope: &ProfileScope,
    hostnames: &[String],
    changes: &RuleUpdateChanges,
    api_folders: &[ApiFolder],
    globals: &Globals,
    write_error: Option<&Error>,
) -> Result<(), Error> {
    let api_rules = read_back_for_verification(client, &scope.id, api_folders, write_error).await?;
    // A re-run of `rule update` converges either kind of gap (the merge is
    // idempotent), unlike create's mismatch, which would duplicate-POST —
    // both feed one retry_argv.
    let mut reconciled = reconcile(
        hostnames,
        &api_rules,
        api_folders,
        |rule| update_matches(rule, changes),
        Exit::Retryable,
    )?;

    if reconciled.absent.is_empty() && reconciled.mismatched.is_empty() {
        print_rules(globals, &reconciled.landed)?;
        announce_if_ambiguous_write_landed(write_error);
        return Ok(());
    }
    if let Some(original) = resolve_ambiguous_write(&mut reconciled, write_error) {
        return Err(original);
    }
    let results = reconciled.results;

    // Preserves the original per-target order — `absent` and `mismatched`
    // are separate buckets, but a combined retry_argv reads better in the
    // order the targets were given.
    let gaps: Vec<String> = results
        .iter()
        .filter(|(_, result)| !matches!(result, TargetResult::Landed))
        .map(|(target, _)| target.clone())
        .collect();
    let retry_argv = update_retry_argv(&scope.id, changes, &gaps);
    Err(multi::aggregate("rule", &results, retry_argv))
}

fn update_matches(rule: &Rule, changes: &RuleUpdateChanges) -> bool {
    changes.action.is_none_or(|action| rule.action == action)
        && changes
            .via
            .as_deref()
            .is_none_or(|via| rule.via.as_deref() == Some(via))
        && changes
            .via6
            .as_deref()
            .is_none_or(|via6| rule.via6.as_deref() == Some(via6))
        && changes
            .enabled
            .is_none_or(|enabled| rule.enabled == enabled)
        && changes.folder_id.is_none_or(|patch| match patch {
            FolderPatch::Root => rule.folder_id.is_none(),
            FolderPatch::To(id) => rule.folder_id == Some(id),
        })
}

async fn delete(raw_hostnames: &[String], dry_run: bool, globals: &Globals) -> Result<(), Error> {
    // Hostnames are percent-encoded into the DELETE path, so they need the
    // path-bound check (rejects `?`/`#`/`%` in addition to control
    // characters; `*` stays legal — commands.md#input-hardening).
    let hostnames = canonicalize_hostnames(raw_hostnames, super::validate::validate_path_bound)?;
    // Upfront, before any request and before the confirmation prompt: a
    // delete emits no row, but `--fields` still names a field of `Rule` (the
    // command's data schema) — a typo must fail before anything is deleted,
    // not slip through as an unchecked flag that does nothing. A dry-run
    // delete prints the plan instead, so the two flags still conflict
    // outright, same as `create`/`update`.
    validate_fields(globals.fields.as_deref(), Rule::FIELDS)?;
    super::reject_fields_with_dry_run(dry_run, globals)?;

    let (client, _source, config) = super::authenticated_client(globals)?;
    let scope = super::scope::resolve_profile(&client, globals, config.default_profile()).await?;

    if dry_run {
        let requests = hostnames
            .iter()
            .map(|hostname| PlannedRequest {
                method: "DELETE",
                path: rule_path(&scope.id, hostname),
                intent: serde_json::to_value(RuleDeleteIntent {
                    hostname: hostname.clone(),
                })
                .expect("intent serializes"),
            })
            .collect();
        return plan::print(globals, &Plan { requests });
    }

    let prompt = delete_prompt(&hostnames, &scope.name);
    confirm(&prompt, globals.yes, &scope).await?;

    let mut results: Vec<(String, TargetResult)> = Vec::with_capacity(hostnames.len());
    let mut aborted = false;
    for hostname in &hostnames {
        if aborted {
            results.push((hostname.clone(), TargetResult::Skipped));
            continue;
        }
        let path = rule_path(&scope.id, hostname);
        match client.write(Method::DELETE, &path, &[], "rule").await {
            Ok(_envelope) => results.push((hostname.clone(), TargetResult::Landed)),
            Err(error) => {
                results.push((hostname.clone(), TargetResult::Failed(Box::new(error))));
                aborted = true;
            }
        }
    }

    // Every target landed exactly when the loop never aborted (delete stops
    // on the first failure and skips the rest, so a partial batch always
    // trips `aborted`).
    if !aborted {
        eprintln!(
            "info: deleted {} rule{} from profile \"{}\"",
            hostnames.len(),
            if hostnames.len() == 1 { "" } else { "s" },
            escape_controls(&scope.name)
        );
        return Ok(());
    }

    // Failed and skipped targets both need re-attempting; landed ones must
    // never be resent (commands.md#rule).
    let retry_targets: Vec<String> = results
        .iter()
        .filter(|(_, result)| !matches!(result, TargetResult::Landed))
        .map(|(target, _)| target.clone())
        .collect();
    let mut retry_argv = retry_argv_base("delete", &scope.id);
    retry_argv.push("--yes".to_owned());
    retry_argv.extend(retry_targets);

    let mut error = multi::aggregate("rule", &results, retry_argv);
    // `delete` has no read-back (D11): a retryable failure is genuinely
    // ambiguous — the DELETE may have landed, and blindly re-running
    // retry_argv then 404s on a rule that is actually already gone. A
    // terminal-only failure (e.g. 404 itself) keeps the default hint —
    // there is no ambiguity to inspect for.
    if results
        .iter()
        .any(|(_, result)| matches!(result, TargetResult::Failed(e) if e.retryable()))
    {
        error = error.with_hint(
            "a failed delete may still have landed; re-fetch with `cdctl rule list` before \
             re-running the retry_argv from the JSON error envelope (--json)",
        );
    }
    Err(error)
}

/// Lists the hostnames when few enough to stay one line; otherwise the
/// count plus a short prefix. The inline-list/truncation policy is
/// CLI-local — commands.md specifies the confirmation tier, not this shape.
fn delete_prompt(hostnames: &[String], profile_name: &str) -> String {
    const SHOWN_INLINE: usize = 5;
    const HEAD: usize = 3;
    let count = hostnames.len();
    let plural = if count == 1 { "" } else { "s" };
    let listed = if count <= SHOWN_INLINE {
        hostnames
            .iter()
            .map(|hostname| escape_controls(hostname).into_owned())
            .collect::<Vec<_>>()
            .join(", ")
    } else {
        let head: Vec<String> = hostnames
            .iter()
            .take(HEAD)
            .map(|hostname| escape_controls(hostname).into_owned())
            .collect();
        format!("{}, +{} more", head.join(", "), count - HEAD)
    };
    format!(
        "delete {count} rule{plural} ({listed}) from profile \"{}\"?",
        escape_controls(profile_name)
    )
}

/// Every rule in `profile_id`: the root listing (no folder segment) unioned
/// with one `GET` per entry in `folders`, run concurrently. The root listing
/// alone includes only unfoldered rules — a rule inside a folder is visible
/// solely through that folder's own listing (read-verification.md "Listing
/// root rules") — so a caller that skips a folder here silently drops every
/// rule inside it. `folders` is the caller's, not fetched here: a caller
/// that already has it for another reason (folder resolution, the FOLDER
/// column) pays no second `/groups` GET.
async fn fetch_all_rules(
    client: &Client,
    profile_id: &str,
    folders: &[ApiFolder],
) -> Result<Vec<ApiRule>, Error> {
    let root_path = rules_path(profile_id);
    let folder_paths: Vec<String> = folders
        .iter()
        .map(|folder| folder_rules_path(profile_id, folder.pk))
        .collect();
    let (root_envelope, folder_rule_lists) = tokio::try_join!(
        client.get(&root_path, "rule"),
        try_join_all(
            folder_paths
                .iter()
                .map(|path| fetch_folder_rules(client, path))
        ),
    )?;

    let mut rules: Vec<ApiRule> = root_envelope.keyed_as("rules")?;
    for folder_rules in folder_rule_lists {
        rules.extend(folder_rules);
    }
    Ok(rules)
}

/// One folder's rules, tolerating exactly the not-found classification: the
/// folder ids driving this GET come from a `/groups` fetch moments earlier,
/// so a 404 here can only mean the folder was deleted in the race between
/// the two calls — its rules died with it, so an empty contribution is the
/// true state (read-verification.md "Listing root rules"). Every other
/// error (500, auth, transport) still propagates through the caller's
/// `try_join_all` — treating those as empty too would make `rule list` print
/// silently-partial data, the exact class of bug this module exists to
/// prevent. The root GET ([`fetch_all_rules`]) keeps full propagation: a
/// root 404 means the profile itself is gone, a real error.
async fn fetch_folder_rules(client: &Client, path: &str) -> Result<Vec<ApiRule>, Error> {
    match client.get(path, "rule").await {
        Ok(envelope) => envelope.keyed_as("rules"),
        Err(e) if e.exit() == Exit::NotFound => Ok(Vec::new()),
        Err(e) => Err(e),
    }
}

/// The re-fetch every read-back verification shares: profile-wide (module
/// doc — [`fetch_all_rules`]), not just the root listing. A *retryable*
/// failure here means the preceding write's success cannot be confirmed,
/// but it already landed, so `landed_write_unverified` remaps it to
/// `write.unverified`. A terminal failure (e.g. the profile itself is gone)
/// passes through unremapped, by design (PR 5 reviewed intent): it is
/// already a real, actionable error, and folding it into `write.unverified`
/// would bury its own hint (auth's `cdctl auth login`, say) under a generic
/// one.
async fn read_back(
    client: &Client,
    profile_id: &str,
    folders: &[ApiFolder],
) -> Result<Vec<ApiRule>, Error> {
    fetch_all_rules(client, profile_id, folders)
        .await
        .map_err(|e| super::landed_write_unverified(e, "rule"))
}

/// [`read_back`], but for the branch that also resolves a retryable
/// `client.write` error (`write_error`, `verify_create`/`verify_update`'s
/// case (d)): the ambiguity is unresolvable when the read-back itself fails,
/// so it still surfaces as `write.unverified` — never the original
/// retryable error, which would invite an unsafe duplicate-POST replay —
/// but the write error's message rides along as a `--debug` note, since
/// `write.unverified`'s own message otherwise names only the read-back
/// failure. The stock `write.unverified` hint claims the API acknowledged
/// the write — untrue on this path, where the write itself errored — so it
/// is reworded to state the uncertainty; a terminal read-back failure (e.g.
/// auth) passes through `read_back` unremapped and keeps its own hint.
async fn read_back_for_verification(
    client: &Client,
    profile_id: &str,
    folders: &[ApiFolder],
    write_error: Option<&Error>,
) -> Result<Vec<ApiRule>, Error> {
    read_back(client, profile_id, folders)
        .await
        .map_err(|error| match write_error {
            Some(write_error) => {
                let error = if error.code == "write.unverified" {
                    error.with_hint(
                        "the write may have landed; re-fetch with `cdctl rule list` instead of \
                         retrying",
                    )
                } else {
                    error
                };
                error.with_debug_note(format!(
                    "the read-back could not resolve the preceding write error: {}",
                    write_error.message
                ))
            }
            None => error,
        })
}

/// Case (b)/(c) of the ambiguous-write reconciliation (module doc), shared
/// by `verify_create`/`verify_update`. Called only once the caller has
/// already ruled out full convergence (case (a)). `write_error` being `None`
/// (the ordinary post-success read-back) is a no-op. Otherwise:
/// - every target absent (case (b)): the write demonstrably did not apply —
///   `Some` carries the original error back to the caller, to return
///   verbatim, unaggregated. Absence of *every* target is the requirement,
///   not an empty `landed`: a present-but-mismatched target already exists
///   upstream, so the replay that a pass-through's exit 8 invites would
///   duplicate-POST it.
/// - anything landed or mismatched (case (c)): `None`, but every
///   still-absent target's [`TargetResult`] is swapped in place for the
///   original write error — these targets verifiably never landed, and the
///   write error is why. Mismatched targets are untouched: they already
///   carry the caller's per-verb `state_mismatch` verdict, which the write
///   error cannot improve on.
fn resolve_ambiguous_write(
    reconciled: &mut Reconciled,
    write_error: Option<&Error>,
) -> Option<Error> {
    let write_error = write_error?;
    if reconciled.landed.is_empty() && reconciled.mismatched.is_empty() {
        return Some(write_error.clone());
    }
    let absent: std::collections::HashSet<&str> =
        reconciled.absent.iter().map(String::as_str).collect();
    for (target, result) in &mut reconciled.results {
        if absent.contains(target.as_str()) {
            *result = TargetResult::Failed(Box::new(write_error.clone()));
        }
    }
    None
}

/// The case (a) info line (module doc): printed only when the read-back
/// resolved a retryable write error, never on the ordinary success path.
fn announce_if_ambiguous_write_landed(write_error: Option<&Error>) {
    if write_error.is_some() {
        eprintln!(
            "info: the write reported an error but the read-back confirms every target landed"
        );
    }
}

/// A target absent from the read-back never landed — resending it is safe.
fn write_dropped(hostname: &str) -> Error {
    Error::new(
        "rule.write_dropped",
        format!("{hostname:?} did not land in the read-back verification"),
        Exit::Retryable,
    )
}

/// A target present but not in the desired state. `exit` differs by verb:
/// terminal for `create` (a retry would duplicate-POST it), retryable for
/// `update` (the merge is idempotent, so a re-run converges it).
fn state_mismatch(hostname: &str, exit: Exit) -> Error {
    Error::new(
        "rule.state_mismatch",
        format!("{hostname:?} landed but not in the desired state"),
        exit,
    )
}

/// A mismatched target paired with its read-back `Rule`, so `verify_create`
/// can tell a convergeable mismatch from an unclearable via6
/// (write-verification.md "`via_v6` cannot be cleared") before building a
/// retry.
type MismatchedTarget = (String, Rule);

/// `reconcile`'s verdict on every target, in the original order.
struct Reconciled {
    results: Vec<(String, TargetResult)>,
    landed: Vec<Rule>,
    /// Never landed — resending is safe.
    absent: Vec<String>,
    /// Landed but not in the desired state; `mismatch_exit` set its
    /// [`TargetResult`]'s retryability.
    mismatched: Vec<MismatchedTarget>,
}

/// The read-back reconciliation shared by `verify_create` and
/// `verify_update`: match every target against the freshly re-fetched
/// `api_rules` — a `HashMap` built once, since a linear `find` per hostname
/// would be up to 500 targets x 10,000 rules today — and test it with
/// `matches`. Absent and present-but-wrong-state stay in separate buckets;
/// only the caller knows whether its verb can fold them into one retry set
/// (commands.md#rule). A read-back rule that fails to normalize is
/// `write.unverified`: the write already landed, so it is never a bare
/// classified error.
fn reconcile(
    hostnames: &[String],
    api_rules: &[ApiRule],
    folders: &[ApiFolder],
    matches: impl Fn(&Rule) -> bool,
    mismatch_exit: Exit,
) -> Result<Reconciled, Error> {
    let by_hostname: std::collections::HashMap<&str, &ApiRule> = api_rules
        .iter()
        .map(|api_rule| (api_rule.pk.as_str(), api_rule))
        .collect();

    let mut results = Vec::with_capacity(hostnames.len());
    let mut landed = Vec::new();
    let mut absent = Vec::new();
    let mut mismatched = Vec::new();

    for hostname in hostnames {
        match by_hostname.get(hostname.as_str()) {
            None => {
                results.push((
                    hostname.clone(),
                    TargetResult::Failed(Box::new(write_dropped(hostname))),
                ));
                absent.push(hostname.clone());
            }
            Some(api_rule) => {
                let rule = Rule::from_api(api_rule, folders)
                    .map_err(|e| super::landed_write_unverified(e, "rule"))?;
                if matches(&rule) {
                    results.push((hostname.clone(), TargetResult::Landed));
                    landed.push(rule);
                } else {
                    results.push((
                        hostname.clone(),
                        TargetResult::Failed(Box::new(state_mismatch(hostname, mismatch_exit))),
                    ));
                    mismatched.push((hostname.clone(), rule));
                }
            }
        }
    }

    Ok(Reconciled {
        results,
        landed,
        absent,
        mismatched,
    })
}

/// `cdctl rule <verb> --profile <id>`, the prefix every `retry_argv` this
/// module builds starts with.
fn retry_argv_base(verb: &str, profile_id: &str) -> Vec<String> {
    vec![
        "cdctl".to_owned(),
        "rule".to_owned(),
        verb.to_owned(),
        "--profile".to_owned(),
        profile_id.to_owned(),
    ]
}

fn create_retry_argv(
    profile_id: &str,
    spec: &ActionSpec,
    folder_id: Option<i64>,
    hostnames: &[String],
) -> Vec<String> {
    let mut argv = retry_argv_base("create", profile_id);
    argv.extend(spec.retry_flags());
    // `enabled: true` is create's CLI-owned default, so it is never
    // rendered (commands.md#rule); `update_retry_argv` renders `enabled`
    // explicitly since an update patch has no such default.
    if spec.enabled == Some(false) {
        argv.push("--disabled".to_owned());
    }
    if let Some(id) = folder_id {
        argv.push("--folder".to_owned());
        argv.push(id.to_string());
    }
    argv.extend(hostnames.iter().cloned());
    argv
}

fn update_retry_argv(
    profile_id: &str,
    changes: &RuleUpdateChanges,
    hostnames: &[String],
) -> Vec<String> {
    let mut argv = retry_argv_base("update", profile_id);
    argv.extend(action_flags::action_via_flags(
        changes.action,
        changes.via.as_deref(),
        changes.via6.as_deref(),
    ));
    match changes.enabled {
        Some(true) => argv.push("--enabled".to_owned()),
        Some(false) => argv.push("--disabled".to_owned()),
        None => {}
    }
    match changes.folder_id {
        Some(FolderPatch::To(id)) => {
            argv.push("--folder".to_owned());
            argv.push(id.to_string());
        }
        Some(FolderPatch::Root) => argv.push("--root".to_owned()),
        None => {}
    }
    argv.extend(hostnames.iter().cloned());
    argv
}

/// Lowercase, strip one trailing dot, then dedup preserving first
/// occurrence (commands.md#rule) — a duplicate inside a `POST` chunk
/// atomically fails the whole chunk upstream (commands.md#rule-import-semantics),
/// so `cdctl` never sends one. `validate_one` lets callers choose the
/// control-character-only check (create/update, form-bound) or the
/// path-bound check (delete, which percent-encodes the hostname into a URL).
fn canonicalize_hostnames(
    raw: &[String],
    validate_one: impl Fn(&str, &str) -> Result<(), Error>,
) -> Result<Vec<String>, Error> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::with_capacity(raw.len());
    for hostname in raw {
        validate_one(hostname, "the hostname")?;
        reject_non_ascii(hostname)?;
        let lower = hostname.to_ascii_lowercase();
        let canon = lower.strip_suffix('.').map(str::to_owned).unwrap_or(lower);
        if canon.is_empty() {
            return Err(Error::usage(format!(
                "hostname {hostname:?} is empty after canonicalization (lowercase, one \
                 trailing dot stripped)"
            )));
        }
        if seen.insert(canon.clone()) {
            out.push(canon);
        }
    }
    Ok(out)
}

/// Rejected until non-ASCII behavior is probed live (write-verification.md
/// has no such probe): the read-back reconciliation keys on the server's
/// `PK` verbatim vs `cdctl`'s ASCII-lowercased input, and a server-side
/// transform of a non-ASCII `PK` would classify the target absent ->
/// "resending is safe" -> exit 8 — an agent looping on that exit code would
/// duplicate-POST into live DNS on every pass. Cheap and conservative; lift
/// once the transform (if any) is known.
fn reject_non_ascii(hostname: &str) -> Result<(), Error> {
    if hostname.is_ascii() {
        return Ok(());
    }
    Err(
        Error::usage(format!("hostname {hostname:?} is not ASCII")).with_hint(
            "use its punycode (xn--) form; non-ASCII hostnames are rejected until the API's \
         handling of them is verified live",
        ),
    )
}

fn check_hostname_cap(hostnames: &[String]) -> Result<(), Error> {
    if hostnames.len() > MAX_HOSTNAMES {
        return Err(Error::usage(format!(
            "{} hostnames exceeds the {MAX_HOSTNAMES}-hostname limit per invocation",
            hostnames.len()
        ))
        .with_hint(format!(
            "pass at most {MAX_HOSTNAMES} hostnames per invocation; a chunked, quota-aware \
             `rule import` arrives in a later release"
        )));
    }
    Ok(())
}

fn render_rules_table(rules: &[Rule], plain: bool) -> comfy_table::Table {
    let rows = rules
        .iter()
        .map(|rule| {
            vec![
                rule.hostname.clone(),
                rule.action.to_string(),
                rule.via.clone().unwrap_or_else(|| "-".to_owned()),
                rule.enabled.to_string(),
                rule.folder.clone().unwrap_or_else(|| "-".to_owned()),
            ]
        })
        .collect();
    render_table(
        &["HOSTNAME", "ACTION", "VIA", "ENABLED", "FOLDER"],
        rows,
        plain,
    )
}

fn print_rules(globals: &Globals, rules: &[Rule]) -> Result<(), Error> {
    emit(globals.mode, globals.fields.as_deref(), &rules, || {
        println!("{}", render_rules_table(rules, globals.plain));
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::action::Action;

    /// A minimal landed rule for tests that only need a `mismatched` entry
    /// to exist, not its content — `via6` is the one field the via6-clear
    /// tests vary.
    fn test_rule(hostname: &str, via6: Option<&str>) -> Rule {
        Rule {
            hostname: hostname.to_owned(),
            action: Action::Block,
            via: None,
            via6: via6.map(str::to_owned),
            enabled: true,
            folder: None,
            folder_id: None,
            order: 1,
        }
    }

    fn spoof_spec(via: &str, via6: Option<&str>, enabled: Option<bool>) -> ActionSpec {
        let flags = ActionFlags {
            action: Some(Action::Spoof),
            via: Some(via.to_owned()),
            via6: via6.map(str::to_owned),
            enabled: enabled == Some(true),
            disabled: enabled == Some(false),
        };
        action_flags::validate(
            &flags,
            Caps {
                via6_allowed: true,
                action_required: true,
                default_enabled: true,
            },
        )
        .expect("valid spoof spec")
    }

    #[test]
    fn canonicalization_lowercases_strips_one_trailing_dot_and_dedups() {
        let raw = vec![
            "A.Example.com.".to_owned(),
            "a.example.com".to_owned(),
            "B.example.com".to_owned(),
        ];
        let out = canonicalize_hostnames(&raw, super::super::validate::reject_control_chars)
            .expect("canonicalizes");
        assert_eq!(out, vec!["a.example.com", "b.example.com"]);
    }

    #[test]
    fn wildcards_survive_canonicalization() {
        let raw = vec!["*.Ads.example.com".to_owned()];
        let out = canonicalize_hostnames(&raw, super::super::validate::reject_control_chars)
            .expect("canonicalizes");
        assert_eq!(out, vec!["*.ads.example.com"]);
    }

    #[test]
    fn a_bare_dot_is_empty_after_canonicalization() {
        let raw = vec![".".to_owned()];
        let error = canonicalize_hostnames(&raw, super::super::validate::reject_control_chars)
            .expect_err("empty after stripping the trailing dot");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[test]
    fn non_ascii_hostnames_are_rejected_with_a_punycode_hint() {
        let raw = vec!["café.example.com".to_owned()];
        let error = canonicalize_hostnames(&raw, super::super::validate::reject_control_chars)
            .expect_err("non-ASCII hostnames are rejected");
        assert_eq!(error.exit(), Exit::Usage);
        let hint = error.hint.expect("hint present");
        assert!(hint.contains("xn--"), "got: {hint}");
    }

    #[test]
    fn delete_canonicalization_rejects_path_metacharacters() {
        let raw = vec!["a?b.example.com".to_owned()];
        let error = canonicalize_hostnames(&raw, super::super::validate::validate_path_bound)
            .expect_err("rejected");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[test]
    fn five_hundred_hostnames_is_the_boundary() {
        let ok: Vec<String> = (0..500).map(|i| format!("h{i}.example.com")).collect();
        check_hostname_cap(&ok).expect("500 is allowed");

        let too_many: Vec<String> = (0..501).map(|i| format!("h{i}.example.com")).collect();
        let error = check_hostname_cap(&too_many).expect_err("501 is rejected");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(error.hint.expect("hint present").contains("500"));
    }

    /// The cap runs on the canonicalized (post-dedup) list, not the raw
    /// argv — 501 raw hostnames that collapse to 500 distinct ones must
    /// succeed. Reordering `canonicalize_hostnames`/`check_hostname_cap`
    /// would silently break this.
    #[test]
    fn the_cap_runs_after_dedup_so_501_raw_hostnames_collapsing_to_500_succeed() {
        let mut raw: Vec<String> = (0..500).map(|i| format!("h{i}.example.com")).collect();
        raw.push("h0.example.com".to_owned());
        assert_eq!(raw.len(), 501);

        let canon = canonicalize_hostnames(&raw, super::super::validate::reject_control_chars)
            .expect("canonicalizes");
        assert_eq!(canon.len(), 500, "the duplicate collapsed");
        check_hostname_cap(&canon).expect("500 distinct hostnames is within the cap");
    }

    #[test]
    fn create_matches_checks_every_field() {
        let spec = spoof_spec("192.0.2.1", Some("2001:db8::1"), None);
        let rule = Rule {
            hostname: "a.example.com".into(),
            action: Action::Spoof,
            via: Some("192.0.2.1".into()),
            via6: Some("2001:db8::1".into()),
            enabled: true,
            folder: None,
            folder_id: Some(2),
            order: 1,
        };
        assert!(create_matches(&rule, &spec, Some(2)));
        assert!(!create_matches(&rule, &spec, None), "folder_id must match");
        assert!(!create_matches(&rule, &spec, Some(3)), "wrong folder");

        let wrong_via = Rule {
            via: Some("192.0.2.99".into()),
            ..rule.clone()
        };
        assert!(!create_matches(&wrong_via, &spec, Some(2)));
    }

    #[tokio::test]
    async fn update_matches_checks_only_sent_fields() {
        let changes = RuleUpdateChanges {
            enabled: Some(false),
            ..RuleUpdateChanges::default()
        };
        let rule = Rule {
            hostname: "a.example.com".into(),
            action: Action::Block,
            via: None,
            via6: None,
            enabled: false,
            folder: None,
            folder_id: Some(9),
            order: 1,
        };
        // `folder_id` was never sent: an unrelated live value must not fail
        // the check (write-verification.md's merge preserves it).
        assert!(update_matches(&rule, &changes));

        let still_enabled = Rule {
            enabled: true,
            ..rule.clone()
        };
        assert!(!update_matches(&still_enabled, &changes));
    }

    #[test]
    fn update_matches_root_requires_a_null_folder_id() {
        let changes = RuleUpdateChanges {
            folder_id: Some(FolderPatch::Root),
            ..RuleUpdateChanges::default()
        };
        let still_foldered = Rule {
            hostname: "a.example.com".into(),
            action: Action::Block,
            via: None,
            via6: None,
            enabled: true,
            folder: Some("Ads".into()),
            folder_id: Some(4),
            order: 1,
        };
        assert!(!update_matches(&still_foldered, &changes));
        let rooted = Rule {
            folder: None,
            folder_id: None,
            ..still_foldered.clone()
        };
        assert!(update_matches(&rooted, &changes));
    }

    #[test]
    fn resolve_ambiguous_write_is_a_no_op_without_a_write_error() {
        let mut reconciled = Reconciled {
            results: vec![("a.example.com".into(), TargetResult::Landed)],
            landed: vec![],
            absent: vec![],
            mismatched: vec![],
        };
        assert!(resolve_ambiguous_write(&mut reconciled, None).is_none());
    }

    #[test]
    fn resolve_ambiguous_write_returns_the_original_error_when_every_target_is_absent() {
        let write_error = Error::new("upstream.error", "boom", Exit::Retryable);
        let mut reconciled = Reconciled {
            results: vec![(
                "a.example.com".into(),
                TargetResult::Failed(Box::new(write_dropped("a.example.com"))),
            )],
            landed: vec![],
            absent: vec!["a.example.com".into()],
            mismatched: vec![],
        };
        let resolved = resolve_ambiguous_write(&mut reconciled, Some(&write_error))
            .expect("every target absent: the original error comes back");
        assert_eq!(resolved.code, "upstream.error");
    }

    /// A present-but-mismatched target is not safely replayable: passing the
    /// original retryable error through would invite a duplicate-POST against
    /// the existing hostname. Only an all-absent read-back passes it through.
    #[test]
    fn resolve_ambiguous_write_never_passes_through_when_a_target_landed_wrong() {
        let write_error = Error::new("upstream.error", "boom", Exit::Retryable);
        let mut reconciled = Reconciled {
            results: vec![(
                "a.example.com".into(),
                TargetResult::Failed(Box::new(state_mismatch("a.example.com", Exit::Generic))),
            )],
            landed: vec![],
            absent: vec![],
            mismatched: vec![("a.example.com".into(), test_rule("a.example.com", None))],
        };
        assert!(
            resolve_ambiguous_write(&mut reconciled, Some(&write_error)).is_none(),
            "a mismatch means the write may have applied: aggregate, never replay"
        );
        match &reconciled.results[0].1 {
            TargetResult::Failed(error) => assert_eq!(
                error.code, "rule.state_mismatch",
                "the mismatch verdict stays authoritative for its row"
            ),
            other => panic!("the mismatched row must stay Failed, got {other:?}"),
        }
    }

    /// The absent target's `rule.write_dropped` verdict is swapped for the
    /// write error itself; the mismatched target keeps its own
    /// `rule.state_mismatch` verdict — the write error explains an absence,
    /// not a wrong state.
    #[test]
    fn resolve_ambiguous_write_attributes_only_absent_targets_and_leaves_mismatches_alone() {
        let write_error = Error::new("ratelimit.exceeded", "rate limited", Exit::Retryable);
        let landed_rule = Rule {
            hostname: "a.example.com".into(),
            action: Action::Block,
            via: None,
            via6: None,
            enabled: true,
            folder: None,
            folder_id: None,
            order: 1,
        };
        let mut reconciled = Reconciled {
            results: vec![
                ("a.example.com".into(), TargetResult::Landed),
                (
                    "b.example.com".into(),
                    TargetResult::Failed(Box::new(write_dropped("b.example.com"))),
                ),
                (
                    "c.example.com".into(),
                    TargetResult::Failed(Box::new(state_mismatch("c.example.com", Exit::Generic))),
                ),
            ],
            landed: vec![landed_rule],
            absent: vec!["b.example.com".into()],
            mismatched: vec![("c.example.com".into(), test_rule("c.example.com", None))],
        };

        assert!(resolve_ambiguous_write(&mut reconciled, Some(&write_error)).is_none());

        let (_, b_result) = reconciled
            .results
            .iter()
            .find(|(target, _)| target == "b.example.com")
            .expect("b.example.com present");
        match b_result {
            TargetResult::Failed(error) => assert_eq!(error.code, "ratelimit.exceeded"),
            TargetResult::Landed | TargetResult::Skipped => {
                panic!("b.example.com must stay Failed")
            }
        }

        let (_, c_result) = reconciled
            .results
            .iter()
            .find(|(target, _)| target == "c.example.com")
            .expect("c.example.com present");
        match c_result {
            TargetResult::Failed(error) => assert_eq!(
                error.code, "rule.state_mismatch",
                "a mismatch keeps its own verdict, not the write error's"
            ),
            TargetResult::Landed | TargetResult::Skipped => {
                panic!("c.example.com must stay Failed")
            }
        }
    }

    #[test]
    fn create_retry_argv_uses_resolved_ids_and_omits_the_default_enabled() {
        let spec = spoof_spec("192.0.2.1", None, None);
        let argv = create_retry_argv("pk1", &spec, Some(7), &["a.example.com".to_owned()]);
        assert_eq!(
            argv,
            vec![
                "cdctl",
                "rule",
                "create",
                "--profile",
                "pk1",
                "--action",
                "spoof",
                "--via",
                "192.0.2.1",
                "--folder",
                "7",
                "a.example.com",
            ]
        );
    }

    #[test]
    fn create_retry_argv_renders_disabled_explicitly() {
        let spec = spoof_spec("192.0.2.1", None, Some(false));
        let argv = create_retry_argv("pk1", &spec, None, &["a.example.com".to_owned()]);
        assert!(argv.contains(&"--disabled".to_owned()));
        assert!(!argv.contains(&"--folder".to_owned()));
    }

    #[test]
    fn update_retry_argv_renders_root_and_explicit_enabled() {
        let changes = RuleUpdateChanges {
            enabled: Some(true),
            folder_id: Some(FolderPatch::Root),
            ..RuleUpdateChanges::default()
        };
        let argv = update_retry_argv("pk1", &changes, &["a.example.com".to_owned()]);
        assert_eq!(
            argv,
            vec![
                "cdctl",
                "rule",
                "update",
                "--profile",
                "pk1",
                "--enabled",
                "--root",
                "a.example.com",
            ]
        );
    }

    #[test]
    fn delete_prompt_lists_hostnames_when_few() {
        let prompt = delete_prompt(
            &[
                "a.com".to_owned(),
                "b.example".to_owned(),
                "*.c.net".to_owned(),
            ],
            "Home",
        );
        assert_eq!(
            prompt,
            "delete 3 rules (a.com, b.example, *.c.net) from profile \"Home\"?"
        );
    }

    #[test]
    fn delete_prompt_truncates_when_many() {
        let hostnames: Vec<String> = (0..8).map(|i| format!("h{i}.example.com")).collect();
        let prompt = delete_prompt(&hostnames, "Home");
        assert!(prompt.starts_with("delete 8 rules ("));
        assert!(prompt.contains("+5 more"));
        assert!(!prompt.contains("h7.example.com"));
    }

    #[test]
    fn delete_prompt_is_singular_for_one_hostname() {
        let prompt = delete_prompt(&["a.com".to_owned()], "Home");
        assert!(prompt.starts_with("delete 1 rule ("), "got: {prompt}");
    }
}
