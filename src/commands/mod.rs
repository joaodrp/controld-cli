//! One module per noun (D18); handlers live with their `Subcommand` enums.
//! The per-request ritual (config -> warnings -> token -> client) lives here,
//! once — handlers receive a ready [`Client`], never re-derive one.

pub mod action_flags;
pub mod api;
pub mod auth;
pub mod completions;
pub mod config;
pub mod confirm;
pub mod folder;
pub mod man;
pub mod multi;
pub mod plan;
pub mod profile;
pub mod reference;
pub mod rule;
pub mod scope;
pub mod validate;

use crate::api::client::{Client, ClientConfig};
use crate::cli::Globals;
use crate::config::{
    Config, ResolvedToken, Store, TOKEN_ENV_VAR, TokenSource, env_var, resolve_token,
};
use crate::error::{Error, Exit};
use crate::output;
use secrecy::SecretString;

/// Load a config and surface its warnings — the pair is never split, so the
/// world-readable-token warning cannot be silently dropped by a handler.
pub(crate) fn load_config(store: &Store) -> Result<Config, Error> {
    let loaded = store.load()?;
    for warning in &loaded.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(loaded.config)
}

/// The store discovered, its config loaded (warnings surfaced), and the
/// stored-or-env token resolved against it — once, so no caller loads the
/// config twice. Both `Client` constructors below share this.
fn config_and_token() -> Result<(Config, Option<ResolvedToken>), Error> {
    let store = Store::discover()?;
    let config = load_config(&store)?;
    let token = resolve_token(env_var(TOKEN_ENV_VAR)?, &config);
    Ok((config, token))
}

fn build_client(token: Option<SecretString>, globals: &Globals) -> Result<Client, Error> {
    Client::new(ClientConfig::from_env(
        token,
        globals.timeout,
        globals.no_retry,
        globals.debug,
    )?)
}

/// A [`Client`] that carries the token when one resolves and none otherwise,
/// plus the loaded config. For `cdctl api`: the spec's `security: []`
/// endpoints must stay reachable tokenless (D7), and only the server knows
/// which paths those are — a protected endpoint answers 400/`40001`, which
/// classifies to exit 4.
pub(crate) fn client_with_optional_token(
    globals: &Globals,
) -> Result<(Client, Option<TokenSource>, Config), Error> {
    let (config, resolved) = config_and_token()?;
    let (token, source) = match resolved {
        Some(ResolvedToken { token, source }) => (Some(token), Some(source)),
        None => (None, None),
    };
    Ok((build_client(token, globals)?, source, config))
}

/// An authenticated [`Client`] for the current invocation, where the token
/// came from, and the loaded config. Missing token is `auth.missing_token`
/// (lazy auth, D7), raised **before** the client builds — request-only
/// environment problems (a malformed `CONTROLD_API_URL`) must not outrank
/// the documented exit 4.
pub(crate) fn authenticated_client(
    globals: &Globals,
) -> Result<(Client, TokenSource, Config), Error> {
    let (config, resolved) = config_and_token()?;
    let ResolvedToken { token, source } = resolved.ok_or_else(Error::auth_missing_token)?;
    Ok((build_client(Some(token), globals)?, source, config))
}

/// The `validate` -> `authenticated_client` -> overlapped
/// via-check-and-profile-resolution choreography shared by `rule`/`folder`
/// `create`/`update`, in that fixed order: local usage errors must outrank
/// auth errors (`tests/cli.rs local_flag_errors_outrank_a_missing_token` pins
/// it), so `validate` runs first. The redirect `--via` proxy check
/// (`ActionSpec::check_redirect_via`) and `scope::resolve_profile` are two
/// independent `GET`s once the client exists, so they run overlapped via
/// `tokio::join!` rather than paying both round trips serially — `join!` over
/// `try_join!` on purpose: when both fail, the proxy verdict wins
/// deterministically (checked first below), so an exit code never flips
/// between identical invocations on a network race. Folded into one function
/// rather than left as two back-to-back calls: a caller that fetched the
/// client and skipped this would silently skip the mandatory via check.
pub(crate) async fn validated_client(
    flags: &action_flags::ActionFlags,
    caps: action_flags::Caps,
    globals: &Globals,
) -> Result<
    (
        action_flags::ActionSpec,
        Client,
        Config,
        scope::ProfileScope,
    ),
    Error,
> {
    let spec = action_flags::validate(flags, caps)?;
    let (client, _source, config) = authenticated_client(globals)?;
    let (via_result, scope_result) = tokio::join!(
        spec.check_redirect_via(&client),
        scope::resolve_profile(&client, globals, config.default_profile())
    );
    via_result?;
    let scope = scope_result?;
    Ok((spec, client, config, scope))
}

/// Commands whose stdout is never JSON (`completions`, `reference`, `api`):
/// an explicit `--json`/`--fields` is a usage error; ambient
/// `CONTROLD_OUTPUT` never shapes their stdout (it still shapes error
/// rendering).
pub(crate) fn reject_explicit_json(
    globals: &Globals,
    command: &str,
    artifact: &str,
) -> Result<(), Error> {
    if globals.json_explicit {
        return Err(Error::usage(format!(
            "`cdctl {command}` emits {artifact}, not JSON; drop --json/--fields"
        )));
    }
    Ok(())
}

/// The `--fields` preflight for every *mutating* handler (rule/folder
/// `create`/`update`/`delete`): field-name validation first — `--fields`
/// always names a field of the command's own data schema, so a typo'd field
/// must be a usage error even when the eventual result would be empty
/// (`output::emit`'s own check alone would be vacuous then, and would let the
/// typo through with exit 0) — then the dry-run conflict, so a genuine typo
/// (`--fields bogus`) is reported as that typo, never masked by it. `--fields`
/// conflicts with `--dry-run`: a dry run prints the request plan, not data
/// rows, so projecting row fields over it is meaningless, and the plan
/// envelope's own keys (`method`/`path`/`intent`) are not the row-schema
/// namespace `--fields` names either way. Read-only handlers (`list`, `get`,
/// `status`, `config list`) have no `--dry-run` flag to conflict with, so they
/// call `output::validate_fields` directly instead of this.
pub(crate) fn preflight_fields(
    globals: &Globals,
    fields: &[&str],
    dry_run: bool,
) -> Result<(), Error> {
    output::validate_fields(globals.fields.as_deref(), fields)?;
    if dry_run && globals.fields.is_some() {
        return Err(Error::usage(
            "--fields selects data-row fields, but a dry run prints the request plan; drop \
             --fields or run without --dry-run",
        ));
    }
    Ok(())
}

/// A landed write whose read-back failed: terminal exit 1, never 8 — the
/// write's success was already confirmed, so the retryable contract would
/// invite an exit-code-driven agent to replay it (duplicating the
/// create/update). Remaps only a *retryable* source error: a terminal one
/// (auth, forbidden, `not_found`...) already carries a better hint (e.g. the
/// auth login hint) and can't trigger a replay either, so it passes through
/// unchanged. `noun` names the resource to re-fetch instead (`folder`,
/// `rule`), rendered as `cdctl {noun} list`.
pub(crate) fn landed_write_unverified(error: Error, noun: &'static str) -> Error {
    if !error.retryable() {
        return error;
    }
    let mut remapped = Error::new("write.unverified", error.message, Exit::Generic)
        .with_retry_after(error.retry_after)
        .with_hint(format!(
            // The write itself only *acknowledged* the request — the ack is
            // a hostname-less summary, so it cannot reveal a dropped
            // hostname; only a fresh list confirms what landed.
            "the API acknowledged the write; re-fetch with `cdctl {noun} list` instead of retrying"
        ));
    if let Some(upstream) = error.upstream {
        remapped = remapped.with_upstream(upstream);
    }
    for note in error.debug_notes {
        remapped = remapped.with_debug_note(note);
    }
    remapped
}

/// Reads all of stdin to bytes, shared by `auth login --token-stdin` and
/// `cdctl api --input -`. `what` names the thing being read (`"token"`,
/// `"JSON body"`), for both the terminal notice below and the error mapping.
///
/// Off the runtime thread: a blocking read here would starve `main`'s select
/// of its SIGINT branch and make Ctrl-C appear dead until the read completes
/// — or, at a terminal, until Ctrl-D.
pub(crate) async fn read_stdin(what: &str) -> Result<Vec<u8>, Error> {
    use std::io::{IsTerminal, Read};

    if std::io::stdin().is_terminal() {
        eprintln!("info: reading the {what} from stdin; end with Ctrl-D");
    }
    tokio::task::spawn_blocking(|| {
        let mut raw = Vec::new();
        std::io::stdin().read_to_end(&mut raw).map(|_| raw)
    })
    .await
    .map_err(|e| Error::generic(format!("the stdin reader task failed: {e}")))?
    .map_err(|e| stdin_read_error(what, &e))
}

/// An I/O failure reading stdin is environmental, not a malformed invocation
/// (the caller's flags already parsed correctly) — exit 1 (`Error::generic`),
/// never the usage code (exit 2) scripts treat as "fix your argv". Separated
/// from [`read_stdin`] so the mapping stays unit-testable without driving
/// real stdin I/O.
fn stdin_read_error(what: &str, e: &std::io::Error) -> Error {
    Error::generic(format!("could not read the {what} from stdin: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Exit;

    /// A stdin I/O failure is environmental, not a malformed invocation —
    /// exit 1, never the usage code (exit 2) scripts treat as "fix your
    /// argv". Pins [`read_stdin`]'s mapping without driving real stdin I/O.
    #[test]
    fn stdin_read_failure_is_exit_1_not_usage() {
        let io_error = std::io::Error::other("device disconnected");
        let error = stdin_read_error("token", &io_error);
        assert_eq!(error.exit(), Exit::Generic);
        assert!(error.message.contains("device disconnected"));
    }
}
