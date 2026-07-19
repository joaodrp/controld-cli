//! Root parser: noun-verb tree plus the global flags (design.md).

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

use crate::commands::{
    api::ApiArgs, auth::AuthCommand, config::ConfigCommand, folder::FolderCommand,
    profile::ProfileCommand, rule::RuleCommand,
};
use crate::error::Error;
use crate::output::Mode;

const ROOT_EXAMPLES: &str = "Examples:
  cdctl auth login --token-stdin < token.txt
  cdctl profile list
  cdctl rule create ads.example.com --action block --profile Home
  cdctl rule list --json";

#[derive(Debug, Parser)]
#[command(
    name = "cdctl",
    version,
    about = "Manage a Control D account over its REST API",
    after_help = ROOT_EXAMPLES,
    after_long_help = format!(
        "{ROOT_EXAMPLES}\n\nExit codes:\n{}",
        crate::error::EXIT_CODES_HELP
    )
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
#[command(next_help_heading = "Global options")]
#[expect(
    clippy::struct_excessive_bools,
    reason = "CLI flags are boolean by nature"
)]
pub struct GlobalArgs {
    /// Profile to operate on (name or PK) [env: CONTROLD_PROFILE]
    #[expect(
        clippy::doc_markdown,
        reason = "doc comments are clap help text; backticks would render literally"
    )]
    #[arg(short = 'p', long, global = true, value_name = "PK|name")]
    pub profile: Option<String>,

    /// Emit JSON (full document) instead of a table
    #[arg(long, global = true)]
    pub json: bool,

    /// Comma-separated JSON fields to keep (implies --json)
    #[arg(long, global = true, value_delimiter = ',', value_name = "a,b")]
    pub fields: Option<Vec<String>>,

    /// Tables without borders, for awk/cut
    #[arg(long, global = true)]
    pub plain: bool,

    /// Skip confirmation prompts (ignored when the target profile is implicit)
    #[arg(short = 'y', long, global = true)]
    pub yes: bool,

    /// Disable automatic retries
    #[arg(long, global = true)]
    pub no_retry: bool,

    /// Per-request timeout in seconds
    #[arg(long, global = true, value_name = "SECS", value_parser = parse_timeout_secs)]
    pub timeout: Option<u64>,

    /// Request/response trace on stderr (token always redacted)
    #[arg(long, global = true)]
    pub debug: bool,

    /// Suppress info lines on stderr (warnings and errors still print)
    #[arg(short = 'q', long, global = true)]
    pub quiet: bool,
}

impl GlobalArgs {
    /// Whether the invocation asked for JSON explicitly (`--json`/`--fields`),
    /// as opposed to via ambient `CONTROLD_OUTPUT=json`. Needed both before
    /// [`Globals::resolve`] runs (`main`'s early-error rendering, which has
    /// only the raw parsed args) and inside it — one derivation, not two.
    pub fn json_explicit(&self) -> bool {
        self.json || self.fields.is_some()
    }
}

/// `--timeout`'s value parser: a plain message on `0` instead of clap's
/// default `1..18446744073709551615` range dump. Clap-level: exit 2, fires
/// before any client or config exists.
fn parse_timeout_secs(raw: &str) -> Result<u64, String> {
    match raw.parse::<u64>() {
        Ok(0) => Err("must be at least 1".to_owned()),
        Ok(value) => Ok(value),
        Err(err) => Err(err.to_string()),
    }
}

// Declaration order is display order: the everyday commands lead, the
// escape hatch and artifact commands trail.
#[derive(Debug, Subcommand)]
pub enum Command {
    /// Authenticate cdctl with an API token
    #[command(subcommand)]
    Auth(AuthCommand),
    /// Inspect the account's profiles
    #[command(subcommand)]
    Profile(ProfileCommand),
    /// Manage a profile's DNS rules
    #[command(subcommand)]
    Rule(RuleCommand),
    /// Manage a profile's rule folders (API: groups)
    #[command(subcommand)]
    Folder(FolderCommand),
    /// Read and write cdctl's own configuration
    #[command(subcommand)]
    Config(ConfigCommand),
    /// Raw request against the API origin (escape hatch, GET by default)
    Api(ApiArgs),
    /// Shell completion script (works without a token)
    #[command(after_long_help = "Install:\n  \
        bash  cdctl completions bash > ~/.local/share/bash-completion/completions/cdctl\n  \
        zsh   cdctl completions zsh > ~/.zfunc/_cdctl   (add fpath+=~/.zfunc before compinit)\n  \
        fish  cdctl completions fish > ~/.config/fish/completions/cdctl.fish\n\n\
        Create the target directory first if missing (mkdir -p). Open a new \
        shell afterwards. Regenerate after upgrading cdctl.")]
    Completions { shell: clap_complete::Shell },
    /// The full command surface as one Markdown document (works without a token)
    Reference,
    /// Roff man pages for the whole command tree (packaging only, laid out like `completions`)
    #[command(hide = true)]
    Man {
        #[arg(long, value_name = "DIR")]
        out_dir: PathBuf,
    },
}

/// Post-parse global state. `CONTROLD_OUTPUT=json` selects JSON mode but is
/// ambient — only an explicit `--json`/`--fields` conflicts with commands
/// whose stdout is never JSON (`completions`, `reference`, `api`).
#[derive(Debug)]
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
    pub quiet: bool,
}

impl Globals {
    /// A malformed `CONTROLD_OUTPUT` is an error, not silently human mode —
    /// an agent whose env said "give me JSON" must never get tables instead.
    pub fn resolve(args: GlobalArgs) -> Result<Self, Error> {
        let json_explicit = args.json_explicit();
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
            // The flag wins over the environment — clap's own precedence,
            // kept even though `--profile` carries no clap-level `env` (D8's
            // fallback is resolved here instead). Routing `CONTROLD_PROFILE`
            // through `config::env_var` — the one policy every other
            // `CONTROLD_*` read already uses — means an empty value falls
            // back like any other unset variable; a non-UTF-8 value errors
            // loudly instead (`env_var`'s own contract), with no separate
            // filter to keep in sync.
            profile: match args.profile {
                Some(profile) => Some(profile),
                None => crate::config::env_var("CONTROLD_PROFILE")?,
            },
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
            quiet: args.quiet,
        })
    }

    pub fn json(&self) -> bool {
        self.mode == Mode::Json
    }

    /// An advisory `info:` line on stderr, dropped under `--quiet`.
    /// Warnings and errors never route through here — they always print.
    pub fn info(&self, message: impl std::fmt::Display) {
        crate::output::info(self.quiet, message);
    }
}

/// The built command tree, shared by `completions` and `reference`.
///
/// `man` deliberately does NOT use this: `.build()` bakes in the auto-inserted
/// `help` subcommand, which `clap_mangen` must see absent or it emits stray
/// `cdctl-*-help.1` pages (see `commands/man.rs`).
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
