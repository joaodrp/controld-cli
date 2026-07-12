//! `cdctl auth`: tokens are dashboard-issued and arrive on stdin only —
//! argv is world-readable (D6).

use std::io::Read;

use clap::{Args, Subcommand};
use secrecy::SecretString;
use serde::Serialize;

use crate::cli::Globals;
use crate::config::Store;
use crate::error::Error;
use crate::output::{emit, print_key_values};

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
        AuthCommand::Login(args) => login(&args),
        AuthCommand::Status => status(globals).await,
        AuthCommand::Logout => logout(),
    }
}

fn login(args: &LoginArgs) -> Result<(), Error> {
    if !args.token_stdin {
        return Err(
            Error::usage("a token is only accepted on stdin; pass --token-stdin").with_hint(
                "Example: op read 'op://vault/Control D/token' | cdctl auth login --token-stdin",
            ),
        );
    }

    let mut raw = String::new();
    std::io::stdin()
        .read_to_string(&mut raw)
        .map_err(|e| Error::usage(format!("could not read the token from stdin: {e}")))?;
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

async fn status(globals: &Globals) -> Result<(), Error> {
    let (client, source) = super::authenticated_client(globals)?;
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
    );
    Ok(())
}

fn logout() -> Result<(), Error> {
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
