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
        "device",
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

/// The README's resource/action matrix is hand-pasted too. For each row,
/// assert a ticked cell means the resource really has that verb and a blank
/// one means it does not — so a verb landing (or leaving) in a later version
/// fails CI until the matrix is corrected.
#[test]
fn readme_action_matrix_matches_the_verbs() {
    const ACTIONS: [&str; 5] = ["list", "get", "create", "update", "delete"];
    let dir = tempdir();

    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("README.md is readable");
    assert!(
        readme.contains("| Resource | list | get | create | update | delete |"),
        "the matrix header changed shape; the column order this test assumes no longer holds"
    );

    let commands = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("docs/commands.md"),
    )
    .expect("docs/commands.md is readable");

    for (resource, anchor, heading) in [
        ("profile", "docs/commands.md#profile", "## profile"),
        ("rule", "docs/commands.md#rule", "## rule"),
        (
            "folder",
            "docs/commands.md#folder-api-groups",
            "## folder (API: \"groups\")",
        ),
        (
            "device",
            "docs/commands.md#device-api-endpoints",
            "## device (API: \"endpoints\")",
        ),
    ] {
        let verbs: Vec<String> = help_for(dir.path(), &[resource])
            .lines()
            .skip_while(|line| *line != "Commands:")
            .skip(1)
            .take_while(|line| !line.trim().is_empty())
            .filter_map(|line| line.split_whitespace().next())
            .filter(|verb| *verb != "help")
            .map(str::to_owned)
            .collect();

        // The row links the resource name; assert both the link target and
        // the heading it resolves to actually exist.
        assert!(
            readme.contains(&format!("| [{resource}]({anchor}) |")),
            "the matrix row for {resource} lost its {anchor} link"
        );
        assert!(
            commands.contains(heading),
            "{anchor} points at a heading commands.md no longer has: {heading:?}"
        );

        let row = readme
            .lines()
            .find(|line| line.starts_with(&format!("| [{resource}]")))
            .unwrap_or_else(|| panic!("no matrix row for {resource}"));
        // Cells between the pipes, dropping the leading empty split and the
        // resource-name cell — what remains lines up with ACTIONS.
        let cells: Vec<&str> = row.split('|').skip(2).map(str::trim).collect();

        for (action, cell) in ACTIONS.iter().zip(&cells) {
            let ticked = cell.contains('✅');
            let supported = verbs.iter().any(|v| v == action);
            assert_eq!(
                ticked,
                supported,
                "matrix disagrees with `cdctl {resource} --help`: {action} \
                 is {} in the CLI but {} in the README",
                if supported { "present" } else { "absent" },
                if ticked { "ticked" } else { "blank" },
            );
        }
    }
}

/// First cell of every row under `header`, backticks stripped. The tables in
/// the README end at the first line that is not a row, so a section that
/// grows past its table does not leak into the result.
fn table_keys(readme: &str, header: &str) -> Vec<String> {
    let mut lines = readme.lines().skip_while(|line| *line != header);
    assert!(
        lines.next().is_some(),
        "the README has no table with the header {header:?}"
    );
    lines
        .skip(1) // the `| --- | --- |` separator
        .take_while(|line| line.starts_with('|'))
        .filter_map(|line| line.split('|').nth(1))
        .map(|cell| cell.trim().trim_matches('`').to_owned())
        .collect()
}

/// The README's Configuration tables are hand-pasted like the matrix above.
/// Pin each to what the binary really accepts — `ENV_HELP` for the variables,
/// the `config set` value enum for the keys — so a setting added, renamed, or
/// dropped fails CI until the README catches up.
#[test]
fn readme_configuration_tables_match_the_real_settings() {
    let dir = tempdir();
    let readme =
        std::fs::read_to_string(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("README.md"))
            .expect("README.md is readable");

    let documented: Vec<String> = help_for(dir.path(), &[])
        .lines()
        .skip_while(|line| *line != "Environment:")
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .filter_map(|line| line.split_whitespace().next())
        .map(str::to_owned)
        .collect();
    assert!(
        !documented.is_empty(),
        "the root long help lost its Environment section"
    );
    assert_eq!(
        table_keys(&readme, "| Variable | Meaning |"),
        documented,
        "the README variable table disagrees with `cdctl --help`"
    );

    // Long help renders each variant as `- name` or `- name: <doc comment>`.
    let keys: Vec<String> = help_for(dir.path(), &["config", "set"])
        .lines()
        .skip_while(|line| line.trim() != "Possible values:")
        .skip(1)
        .map(str::trim)
        .take_while(|line| line.starts_with("- "))
        .filter_map(|line| line.trim_start_matches("- ").split(':').next())
        .map(str::to_owned)
        .collect();
    assert!(
        !keys.is_empty(),
        "`cdctl config set --help` lost its possible-values list"
    );
    // The table documents the file, so it carries one key `config set` does
    // not accept: `token`. That omission is D6 — `config set token <value>`
    // would put the token in argv — so `auth login` writes it instead.
    let mut expected = keys;
    expected.push("token".to_owned());
    expected.sort();
    let mut listed = table_keys(&readme, "| Key | Meaning | Notes |");
    listed.sort();
    assert_eq!(
        listed, expected,
        "the README key table disagrees with `cdctl config set --help` plus `token`"
    );
}

#[test]
fn reference_snapshot() {
    let dir = tempdir();
    let assert = cdctl(dir.path()).arg("reference").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    insta::assert_snapshot!("reference_document", stdout);
}
