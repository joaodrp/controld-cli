//! One module per noun (D18); handlers live with their `Subcommand` enums.
//! The per-request ritual (config -> warnings -> token -> client) lives here,
//! once — handlers receive a ready [`Client`], never re-derive one.

pub mod api;
pub mod auth;
pub mod completions;
pub mod config;
pub mod profile;
pub mod reference;
pub mod scope;
pub mod validate;

use crate::api::client::{Client, ClientConfig};
use crate::cli::Globals;
use crate::config::{
    Config, ResolvedToken, Store, TOKEN_ENV_VAR, TokenSource, env_var, resolve_token,
};
use crate::error::Error;
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

/// The stored-or-env token, if any, after config warnings surfaced.
fn optional_token() -> Result<Option<ResolvedToken>, Error> {
    let store = Store::discover()?;
    let config = load_config(&store)?;
    Ok(resolve_token(env_var(TOKEN_ENV_VAR)?, &config))
}

fn build_client(token: Option<SecretString>, globals: &Globals) -> Result<Client, Error> {
    Client::new(ClientConfig::from_env(
        token,
        globals.timeout,
        globals.no_retry,
        globals.debug,
    )?)
}

/// A [`Client`] that carries the token when one resolves and none otherwise.
/// For `cdctl api`: the spec's `security: []` endpoints must stay reachable
/// tokenless (D7), and only the server knows which paths those are — a
/// protected endpoint answers 400/`40001`, which classifies to exit 4.
pub(crate) fn client_with_optional_token(
    globals: &Globals,
) -> Result<(Client, Option<TokenSource>), Error> {
    let (token, source) = match optional_token()? {
        Some(ResolvedToken { token, source }) => (Some(token), Some(source)),
        None => (None, None),
    };
    Ok((build_client(token, globals)?, source))
}

/// An authenticated [`Client`] for the current invocation, plus where the
/// token came from. Missing token is `auth.missing_token` (lazy auth, D7),
/// raised **before** the client builds — request-only environment problems
/// (a malformed `CONTROLD_API_URL`) must not outrank the documented exit 4.
pub(crate) fn authenticated_client(globals: &Globals) -> Result<(Client, TokenSource), Error> {
    let ResolvedToken { token, source } =
        optional_token()?.ok_or_else(Error::auth_missing_token)?;
    Ok((build_client(Some(token), globals)?, source))
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
