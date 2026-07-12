//! One module per noun (D18); handlers live with their `Subcommand` enums.
//! The per-request ritual (config -> warnings -> token -> client) lives here,
//! once — handlers receive a ready [`Client`], never re-derive one.

pub mod api;
pub mod auth;
pub mod completions;
pub mod config;
pub mod reference;

use crate::api::client::{Client, ClientConfig};
use crate::cli::Globals;
use crate::config::{
    Config, ResolvedToken, Store, TOKEN_ENV_VAR, TokenSource, env_var, resolve_token,
};
use crate::error::Error;

/// Load a config and surface its warnings — the pair is never split, so the
/// world-readable-token warning cannot be silently dropped by a handler.
pub(crate) fn load_config(store: &Store) -> Result<Config, Error> {
    let loaded = store.load()?;
    for warning in &loaded.warnings {
        eprintln!("warning: {warning}");
    }
    Ok(loaded.config)
}

/// An authenticated [`Client`] for the current invocation, plus where the
/// token came from. Missing token is `auth.missing_token` (lazy auth, D7).
pub(crate) fn authenticated_client(globals: &Globals) -> Result<(Client, TokenSource), Error> {
    let store = Store::discover()?;
    let config = load_config(&store)?;
    let ResolvedToken { token, source } =
        resolve_token(env_var(TOKEN_ENV_VAR)?, &config).ok_or_else(Error::auth_missing_token)?;
    let client = Client::new(ClientConfig::from_env(
        Some(token),
        globals.timeout,
        globals.no_retry,
        globals.debug,
    )?)?;
    Ok((client, source))
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
