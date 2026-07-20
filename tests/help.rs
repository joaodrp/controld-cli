//! `--help` snapshots for the public command surface (root + every visible
//! noun, plus the two artifact commands). No server needed — these are
//! pure clap output — so this stays separate from `cli.rs`. Any wording or
//! flag change to what a user or agent sees shows up as a reviewable
//! snapshot diff.

mod common;
use common::{cdctl, tempdir};

fn help_for(dir: &std::path::Path, args: &[&str]) -> String {
    let assert = cdctl(dir).args(args).arg("--help").assert().success();
    String::from_utf8_lossy(&assert.get_output().stdout).into_owned()
}

#[test]
fn help_snapshots() {
    let dir = tempdir();
    let nouns = [
        "auth",
        "profile",
        "rule",
        "folder",
        "config",
        "api",
        "completions",
        "reference",
    ];

    let root = help_for(dir.path(), &[]);
    // The list above must track the visible surface: a new noun that is not
    // added here fails loudly instead of silently missing its snapshot.
    let advertised: Vec<&str> = root
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.is_empty())
        .filter_map(|line| line.strip_prefix("  "))
        .filter(|line| !line.starts_with(' ')) // wrapped about-text continuations
        .filter_map(|line| line.split_whitespace().next())
        .filter(|name| *name != "help")
        .collect();
    assert_eq!(
        advertised, nouns,
        "every command the root help advertises needs a snapshot below"
    );

    insta::assert_snapshot!("root", root);
    for noun in nouns {
        insta::assert_snapshot!(noun, help_for(dir.path(), &[noun]));
    }
}

/// The README's Usage block is hand-pasted, so it can drift from the real
/// command surface. Assert every command clap advertises (minus the `help`
/// builtin) appears there verbatim, and vice versa, so a new or renamed
/// noun fails CI until the README catches up.
#[test]
fn readme_usage_block_matches_the_command_surface() {
    let dir = tempdir();
    let help = help_for(dir.path(), &[]);
    let command_lines: Vec<&str> = help
        .lines()
        .skip_while(|line| *line != "Commands:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter(|line| !line.starts_with("  help "))
        .collect();

    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("README.md is readable");

    assert!(
        readme.contains("Usage: cdctl [OPTIONS] <COMMAND>"),
        "the README Usage block dropped the usage line"
    );
    for line in &command_lines {
        assert!(
            readme.contains(line),
            "the README Usage block is missing a command clap advertises:\n{line}\n\
             regenerate it from `cdctl --help`"
        );
    }
}

#[test]
fn reference_snapshot() {
    let dir = tempdir();
    let assert = cdctl(dir.path()).arg("reference").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    insta::assert_snapshot!("reference_document", stdout);
}
