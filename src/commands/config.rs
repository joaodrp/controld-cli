//! `cdctl config`: the context-keyed file (D6/D15). `get`/`path` print raw
//! values; `list` annotates each value's source (env / config / default).

use clap::{Subcommand, ValueEnum};
use serde::Serialize;
use serde_json::json;

use crate::cli::Globals;
use crate::config::{Store, TOKEN_ENV_VAR, env_var, resolve_token};
use crate::error::Error;
use crate::output::{emit, print_key_values};

#[derive(Debug, Subcommand)]
pub enum ConfigCommand {
    /// Print one value (raw, not JSON)
    Get { key: Key },
    /// Set one value
    Set { key: Key, value: String },
    /// The active context, each value annotated with its source
    List,
    /// The config file path
    Path,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum Key {
    #[value(name = "current_context")]
    CurrentContext,
    /// Stored per context; `--profile`/`CONTROLD_PROFILE` override it
    #[value(name = "default_profile")]
    DefaultProfile,
}

pub fn run(command: ConfigCommand, globals: &Globals) -> Result<(), Error> {
    let store = Store::discover()?;
    match command {
        ConfigCommand::Get { key } => get(&store, key),
        ConfigCommand::Set { key, value } => set(&store, key, value),
        ConfigCommand::List => list(&store, globals),
        ConfigCommand::Path => {
            println!("{}", store.path().display());
            Ok(())
        }
    }
}

fn get(store: &Store, key: Key) -> Result<(), Error> {
    let config = super::load_config(store)?;
    match key {
        Key::CurrentContext => println!("{}", config.current_context_name()),
        Key::DefaultProfile => {
            // Unset prints nothing: raw values stay pipe-safe.
            if let Some(profile) = config.default_profile() {
                println!("{profile}");
            }
        }
    }
    Ok(())
}

fn set(store: &Store, key: Key, value: String) -> Result<(), Error> {
    super::validate::reject_control_chars(&value, "the value")?;
    let mut config = super::load_config(store)?;
    match key {
        Key::CurrentContext => {
            // Switch-then-login is the intended flow, but a typo'd name would
            // otherwise surface much later as a misleading auth.missing_token.
            if !config.contexts.contains_key(&value) {
                eprintln!("info: context \"{value}\" has no stored token yet");
            }
            config.current_context = Some(value);
        }
        Key::DefaultProfile => config.active_context_mut().default_profile = Some(value),
    }
    store.save(&config)
}

/// A value plus where it came from. Env beats config beats default.
#[derive(Serialize)]
struct Annotated {
    value: Option<String>,
    source: Option<&'static str>,
}

fn list(store: &Store, globals: &Globals) -> Result<(), Error> {
    let config = super::load_config(store)?;

    let current_context = Annotated {
        value: Some(config.current_context_name().to_owned()),
        source: Some(if config.current_context.is_some() {
            "config"
        } else {
            "default"
        }),
    };
    // The D6 precedence lives in resolve_token; this only renders its verdict.
    let token_source =
        resolve_token(env_var(TOKEN_ENV_VAR)?, &config).map(|resolved| resolved.source.to_string());
    let default_profile = if let Some(profile) = env_var("CONTROLD_PROFILE")? {
        Annotated {
            value: Some(profile),
            source: Some("env"),
        }
    } else {
        let value = config.default_profile().map(str::to_owned);
        Annotated {
            source: value.as_ref().map(|_| "config"),
            value,
        }
    };

    let document = json!({
        "context": current_context,
        "token": { "set": token_source.is_some(), "source": token_source },
        "default_profile": default_profile,
    });
    emit(globals.mode, globals.fields.as_deref(), &document, || {
        let show = |a: &Annotated| match (&a.value, a.source) {
            (Some(value), Some(source)) => format!("{value} ({source})"),
            _ => "(not set)".to_owned(),
        };
        print_key_values(&[
            ("context", show(&current_context)),
            (
                "token",
                token_source
                    .as_deref()
                    .map_or_else(|| "(not set)".to_owned(), |s| format!("(set, {s})")),
            ),
            ("default_profile", show(&default_profile)),
        ]);
    });
    Ok(())
}
