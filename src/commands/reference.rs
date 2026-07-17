//! `cdctl reference`: the command surface as one pipeable Markdown document,
//! derived from the same clap metadata as `--help` (D3: no handwritten
//! parallel contract). Works without a token (D7).

use std::fmt::Write;

use crate::cli::Globals;
use crate::error::Error;

pub fn run(globals: &Globals) -> Result<(), Error> {
    super::reject_explicit_json(globals, "reference", "Markdown")?;
    let root = crate::cli::command();

    let mut doc = String::new();
    let _ = writeln!(doc, "# cdctl reference\n");
    if let Some(about) = root.get_long_about().or_else(|| root.get_about()) {
        let _ = writeln!(doc, "{about}\n");
    }
    write_global_flags(&mut doc, &root);
    for subcommand in visible_subcommands(&root) {
        write_command(
            &mut doc,
            subcommand,
            &format!("cdctl {}", subcommand.get_name()),
            2,
        );
    }
    write_exit_codes(&mut doc);
    print!("{doc}");
    Ok(())
}

fn write_exit_codes(doc: &mut String) {
    let _ = writeln!(doc, "## Exit codes\n");
    let _ = writeln!(doc, "```");
    let _ = writeln!(doc, "{}", crate::error::EXIT_CODES_HELP);
    let _ = writeln!(doc, "```");
}

fn write_global_flags(doc: &mut String, root: &clap::Command) {
    let _ = writeln!(doc, "## Global flags\n");
    for arg in root.get_arguments().filter(|a| a.is_global_set()) {
        write_arg(doc, arg);
    }
    let _ = writeln!(doc);
}

fn write_command(doc: &mut String, command: &clap::Command, path: &str, depth: usize) {
    let heading = "#".repeat(depth.min(6));
    let _ = writeln!(doc, "{heading} {path}\n");
    if let Some(about) = command.get_long_about().or_else(|| command.get_about()) {
        let _ = writeln!(doc, "{about}\n");
    }

    let own_args: Vec<&clap::Arg> = command
        .get_arguments()
        .filter(|a| !a.is_global_set() && a.get_id() != "help")
        .collect();
    if !own_args.is_empty() {
        for arg in own_args {
            write_arg(doc, arg);
        }
        let _ = writeln!(doc);
    }

    for subcommand in visible_subcommands(command) {
        write_command(
            doc,
            subcommand,
            &format!("{path} {}", subcommand.get_name()),
            depth + 1,
        );
    }
}

fn write_arg(doc: &mut String, arg: &clap::Arg) {
    let mut signature = String::new();
    match (arg.get_short(), arg.get_long()) {
        (Some(short), Some(long)) => {
            let _ = write!(signature, "-{short}, --{long}");
        }
        (Some(short), None) => {
            let _ = write!(signature, "-{short}");
        }
        (None, Some(long)) => {
            let _ = write!(signature, "--{long}");
        }
        (None, None) => {} // positional; value names below carry it
    }
    if arg.get_action().takes_values() {
        for name in arg.get_value_names().unwrap_or_default() {
            if !signature.is_empty() {
                signature.push(' ');
            }
            let _ = write!(signature, "<{name}>");
        }
    }

    let _ = write!(doc, "- `{signature}`");
    if let Some(help) = arg.get_help() {
        let _ = write!(doc, " — {help}");
    }
    if let Some(env) = arg.get_env().and_then(|e| e.to_str()) {
        let _ = write!(doc, " (env: `{env}`)");
    }
    let _ = writeln!(doc);
}

fn visible_subcommands(command: &clap::Command) -> impl Iterator<Item = &clap::Command> {
    command
        .get_subcommands()
        .filter(|c| !c.is_hide_set() && c.get_name() != "help")
}
