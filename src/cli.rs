//! Root parser: noun-verb tree plus the global flags (design.md).

use clap::{Args, Parser, Subcommand};

use crate::commands::{api::ApiArgs, auth::AuthCommand, config::ConfigCommand};
use crate::error::Error;
use crate::output::Mode;

#[derive(Debug, Parser)]
#[command(
    name = "cdctl",
    version,
    about = "Manage a Control D account over its REST API",
    long_about = "Manage a Control D account over its REST API.\n\n\
        cdctl is not ctrld: ctrld is Control D's DNS daemon and runs DNS on a \
        machine; cdctl manages the account behind it. They coexist."
)]
pub struct Cli {
    #[command(flatten)]
    pub globals: GlobalArgs,
    #[command(subcommand)]
    pub command: Command,
}

/// D6 forbids `--token` (argv is world-readable) and design.md forbids a
/// `-v` short flag (version/verbose ambiguity) — their absence is deliberate.
#[derive(Debug, Args)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI flags are boolean by nature"
)]
pub struct GlobalArgs {
    /// Profile to operate on (name or PK)
    #[arg(
        short = 'p',
        long,
        global = true,
        env = "CONTROLD_PROFILE",
        value_name = "PK|name"
    )]
    pub profile: Option<String>,

    /// Emit JSON (full document) instead of a table
    #[arg(long, global = true)]
    pub json: bool,

    /// Comma-separated JSON fields to keep; implies --json
    #[arg(long, global = true, value_delimiter = ',', value_name = "a,b")]
    pub fields: Option<Vec<String>>,

    /// Tables without borders, for awk/cut
    #[arg(long, global = true)]
    pub plain: bool,

    /// Skip confirmation prompts (ignored when the target profile is implicit)
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Disable automatic retries (GETs only ever retry)
    #[arg(long, global = true)]
    pub no_retry: bool,

    /// Per-request timeout in seconds
    #[arg(long, global = true, value_name = "SECS", value_parser = clap::value_parser!(u64).range(1..))]
    pub timeout: Option<u64>,

    /// Request/response trace on stderr; token always redacted
    #[arg(long, global = true)]
    pub debug: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Raw request against the API origin (escape hatch; GET by default)
    Api(ApiArgs),
    /// Authenticate cdctl with an API token
    #[command(subcommand)]
    Auth(AuthCommand),
    /// Read and write cdctl's own configuration
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Shell completion script (works without a token)
    Completions { shell: clap_complete::Shell },
    /// The full command surface as one Markdown document (works without a token)
    Reference,
}

/// Post-parse global state. `CONTROLD_OUTPUT=json` selects JSON mode but is
/// ambient — only an explicit `--json`/`--fields` conflicts with commands
/// whose stdout is never JSON (`completions`, `reference`, `api`).
#[derive(Debug)]
#[expect(
    dead_code,
    reason = "profile and plain are read from Phase 3 (rules & folders)"
)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI flags are boolean by nature"
)]
pub struct Globals {
    pub profile: Option<String>,
    pub mode: Mode,
    pub json_explicit: bool,
    pub fields: Option<Vec<String>>,
    pub plain: bool,
    pub yes: bool,
    pub no_retry: bool,
    pub timeout: Option<u64>,
    pub debug: bool,
}

impl Globals {
    /// A malformed `CONTROLD_OUTPUT` is an error, not silently human mode —
    /// an agent whose env said "give me JSON" must never get tables instead.
    pub fn resolve(args: GlobalArgs) -> Result<Self, Error> {
        let json_explicit = args.json || args.fields.is_some();
        let json_env = match crate::config::env_var("CONTROLD_OUTPUT")? {
            Some(value) if value == "json" => true,
            Some(other) => {
                return Err(Error::usage(format!(
                    "CONTROLD_OUTPUT must be \"json\", got \"{other}\""
                )));
            }
            None => false,
        };
        Ok(Self {
            profile: args.profile,
            mode: if json_explicit || json_env {
                Mode::Json
            } else {
                Mode::Human
            },
            json_explicit,
            fields: args.fields,
            plain: args.plain,
            yes: args.yes,
            no_retry: args.no_retry,
            timeout: args.timeout,
            debug: args.debug,
        })
    }

    pub fn json(&self) -> bool {
        self.mode == Mode::Json
    }
}

/// The built command tree, shared by `completions` and `reference`.
pub fn command() -> clap::Command {
    use clap::CommandFactory;
    let mut command = Cli::command();
    command.build();
    command
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_tree_is_valid() {
        // clap panics on conflicting definitions at first build.
        command().debug_assert();
    }

    /// A subcommand arg whose id matches a global flag's is silently captured
    /// by the global's value lookup — clap's own `debug_assert` does not catch
    /// it (an `api` arg with id `fields` would feed the global `--fields`).
    #[test]
    fn subcommand_args_never_reuse_global_ids() {
        fn walk(command: &clap::Command, global_ids: &[String]) {
            for sub in command.get_subcommands() {
                for arg in sub.get_arguments().filter(|a| !a.is_global_set()) {
                    assert!(
                        !global_ids.contains(&arg.get_id().to_string()),
                        "arg id {:?} in `{}` collides with a global flag; rename the field or set an explicit id",
                        arg.get_id(),
                        sub.get_name(),
                    );
                }
                walk(sub, global_ids);
            }
        }
        let root = command();
        let global_ids: Vec<String> = root
            .get_arguments()
            .filter(|a| a.is_global_set())
            .map(|a| a.get_id().to_string())
            .collect();
        assert!(!global_ids.is_empty(), "the global flags exist");
        walk(&root, &global_ids);
    }

    #[test]
    fn fields_implies_json() {
        let cli = Cli::parse_from(["cdctl", "auth", "status", "--fields", "email,region"]);
        let globals = Globals::resolve(cli.globals).expect("resolves");
        assert_eq!(globals.mode, Mode::Json);
        assert!(globals.json_explicit && globals.json());
        assert_eq!(
            globals.fields.as_deref(),
            Some(&["email".to_owned(), "region".to_owned()][..])
        );
    }

    #[test]
    fn no_token_flag_exists() {
        let result = Cli::try_parse_from(["cdctl", "auth", "login", "--token", "x"]);
        assert!(result.is_err(), "--token must not exist (D6)");
    }
}
