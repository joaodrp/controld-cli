//! `cdctl auth`: tokens are dashboard-issued and arrive on stdin only —
//! argv is world-readable (D6).

use clap::{Args, Subcommand};
use secrecy::SecretString;
use serde::Serialize;

use crate::cli::Globals;
use crate::config::Store;
use crate::error::Error;
use crate::output::{self, emit, print_key_values};

#[derive(Debug, Subcommand)]
pub enum AuthCommand {
    /// Store an API token read from stdin
    Login(LoginArgs),
    /// Whether a token is configured, where from, and who it authenticates
    Status,
    /// Remove the stored token for the active context
    Logout,
}

#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Read the token from stdin
    #[arg(long)]
    pub token_stdin: bool,
}

pub async fn run(command: AuthCommand, globals: &Globals) -> Result<(), Error> {
    match command {
        AuthCommand::Login(args) => login(&args, globals).await,
        AuthCommand::Status => status(globals).await,
        AuthCommand::Logout => logout(globals),
    }
}

async fn login(args: &LoginArgs, globals: &Globals) -> Result<(), Error> {
    super::reject_explicit_json(globals, "auth login", "nothing on stdout")?;
    if !args.token_stdin {
        return Err(
            Error::usage("a token is only accepted on stdin; pass --token-stdin").with_hint(
                "Example: op read 'op://vault/Control D/token' | cdctl auth login --token-stdin",
            ),
        );
    }

    let raw_bytes = super::read_stdin("token").await?;
    // Not `read_stdin`'s job: an invalid-UTF-8 token is still the same
    // environmental, argv-innocent failure ("stdin carried no token" and the
    // control-character rejection below stay usage errors; only I/O and
    // encoding failures take this mapping) — the shared helper only knows
    // bytes, so the conversion (and its exit-1 mapping) lives at this call
    // site instead.
    let raw = String::from_utf8(raw_bytes)
        .map_err(|e| Error::generic(format!("could not read the token from stdin: {e}")))?;
    let token = raw.trim();
    if token.is_empty() {
        return Err(Error::usage("stdin carried no token"));
    }
    if token.chars().any(char::is_control) || token.contains(' ') {
        return Err(Error::usage(
            "the token contains whitespace or control characters",
        ));
    }

    let store = Store::discover()?;
    let mut config = super::load_config(&store)?;
    let context = config.current_context_name().to_owned();
    config.active_context_mut().token = Some(SecretString::from(token.to_owned()));
    store.save(&config)?;
    eprintln!(
        "info: token stored for context \"{context}\" in {}",
        store.path().display()
    );
    Ok(())
}

#[derive(Serialize)]
struct AuthStatus {
    authenticated: bool,
    email: Option<String>,
    region: Option<String>,
    token_source: String,
}

impl AuthStatus {
    /// The struct's serialized top-level key set, in order — `auth status`'s
    /// upfront `--fields` check (`output::validate_fields`) validates
    /// against exactly this, before any request.
    /// `auth_status_fields_matches_the_serialized_key_set` (below) is the
    /// drift guard: it fails the moment a field is added, renamed, or
    /// removed here without a matching edit to this list.
    const FIELDS: &'static [&'static str] = &["authenticated", "email", "region", "token_source"];
}

async fn status(globals: &Globals) -> Result<(), Error> {
    output::validate_fields(globals.fields.as_deref(), AuthStatus::FIELDS)?;
    let (client, source, _config) = super::authenticated_client(globals)?;
    let user = client.get("/users", "account").await?.flat()?;

    let field = |key: &str| user.get(key).and_then(|v| v.as_str()).map(str::to_owned);
    let auth_status = AuthStatus {
        authenticated: true,
        email: field("email"),
        region: field("stats_endpoint"),
        token_source: source.to_string(),
    };

    emit(
        globals.mode,
        globals.fields.as_deref(),
        &auth_status,
        || {
            print_key_values(&[
                ("authenticated", auth_status.authenticated.to_string()),
                ("email", auth_status.email.clone().unwrap_or_default()),
                ("region", auth_status.region.clone().unwrap_or_default()),
                ("token_source", auth_status.token_source.clone()),
            ]);
        },
    )
}

fn logout(globals: &Globals) -> Result<(), Error> {
    super::reject_explicit_json(globals, "auth logout", "nothing on stdout")?;
    let store = Store::discover()?;
    let mut config = super::load_config(&store)?;
    let context = config.current_context_name().to_owned();

    let stored = config.active_context().is_some_and(|c| c.token.is_some());
    if !stored {
        eprintln!("info: no token stored for context \"{context}\"");
        return Ok(());
    }
    config.active_context_mut().token = None;
    store.save(&config)?;
    eprintln!("info: token removed for context \"{context}\"");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drift guard for [`AuthStatus::FIELDS`]: a field added, renamed, or
    /// removed on the struct without a matching edit to `FIELDS` fails here.
    #[test]
    fn auth_status_fields_matches_the_serialized_key_set() {
        let status = AuthStatus {
            authenticated: true,
            email: Some("user@example.com".into()),
            region: Some("europe".into()),
            token_source: "config".into(),
        };
        let value = serde_json::to_value(&status).expect("serializes");
        let keys: Vec<&str> = value
            .as_object()
            .expect("object")
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, AuthStatus::FIELDS);
    }
}
