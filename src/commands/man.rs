//! Hidden `cdctl man --out-dir DIR`: roff man pages for the whole command
//! tree, generated from the same clap metadata as `--help`/`reference`
//! (module laid out like `completions` per D18). Packaging-only — not part
//! of the public command surface, so it stays hidden from `--help`.

use std::fs;
use std::path::Path;

use crate::cli::Globals;
use crate::error::Error;

pub fn run(out_dir: &Path, globals: &Globals) -> Result<(), Error> {
    super::reject_explicit_json(globals, "man", "roff pages")?;
    fs::create_dir_all(out_dir)
        .map_err(|e| Error::generic(format!("could not create {}: {e}", out_dir.display())))?;
    // Unbuilt, unlike `crate::cli::command()`: `generate_to` disables the
    // auto-inserted "help" subcommand via `disable_help_subcommand` before
    // its own `build()` call, which only takes effect pre-build — a
    // pre-built tree has already baked the help subcommand in.
    clap_mangen::generate_to(
        <crate::cli::Cli as clap::CommandFactory>::command(),
        out_dir,
    )
    .map_err(|e| {
        Error::generic(format!(
            "could not write man pages to {}: {e} (pages already written there may be a partial set)",
            out_dir.display()
        ))
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::output::Mode;

    fn globals() -> Globals {
        Globals {
            profile: None,
            mode: Mode::Human,
            json_explicit: false,
            fields: None,
            plain: false,
            yes: false,
            no_retry: false,
            timeout: None,
            debug: false,
            quiet: false,
        }
    }

    /// Every visible command path, as `clap_mangen::generate_to` names its
    /// files: hyphen-joined display names (`cdctl-rule-create`), hidden
    /// commands (including `man` itself) excluded. Built the same way
    /// `generate_to` builds its own copy (`disable_help_subcommand` set
    /// *before* the first `build()`) — unlike `crate::cli::command()`, which
    /// pre-builds for `completions`/`reference` and so already carries the
    /// auto-inserted "help" subcommand `disable_help_subcommand` can no
    /// longer retract.
    fn expected_pages() -> BTreeSet<String> {
        fn walk(command: &clap::Command, out: &mut BTreeSet<String>) {
            out.insert(
                command
                    .get_display_name()
                    .unwrap_or_else(|| command.get_name())
                    .to_owned(),
            );
            for sub in command.get_subcommands().filter(|c| !c.is_hide_set()) {
                walk(sub, out);
            }
        }
        let mut root =
            <crate::cli::Cli as clap::CommandFactory>::command().disable_help_subcommand(true);
        root.build();
        let mut out = BTreeSet::new();
        walk(&root, &mut out);
        out
    }

    #[test]
    fn generates_one_page_per_visible_command() {
        let dir = tempfile::tempdir().expect("tempdir");
        run(dir.path(), &globals()).expect("generates");

        let found: BTreeSet<String> = fs::read_dir(dir.path())
            .expect("read_dir")
            .map(|entry| {
                entry
                    .expect("entry")
                    .file_name()
                    .to_string_lossy()
                    .trim_end_matches(".1")
                    .to_owned()
            })
            .collect();

        let expected = expected_pages();
        assert_eq!(found, expected);
        for name in ["cdctl", "cdctl-rule", "cdctl-rule-create"] {
            assert!(
                expected.contains(name),
                "expected page set sanity-checks against {name}"
            );
        }
        assert!(
            !found.contains("cdctl-man"),
            "man is hidden; must not get its own page"
        );
        assert!(
            !found.contains("cdctl-help"),
            "generate_to disables the help subcommand"
        );
    }

    #[test]
    fn every_page_is_a_nonempty_roff_document() {
        let dir = tempfile::tempdir().expect("tempdir");
        run(dir.path(), &globals()).expect("generates");

        let mut pages = 0;
        for entry in fs::read_dir(dir.path()).expect("read_dir") {
            let path = entry.expect("entry").path();
            let content = fs::read_to_string(&path).expect("utf8 roff");
            assert!(!content.trim().is_empty(), "{} is empty", path.display());
            // `roff`'s writer emits a short groff string-alias preamble
            // (`.ie \n(.g .ds Aq ...`) before the `.TH` title macro.
            assert!(
                content.lines().take(5).any(|l| l.starts_with(".TH")),
                "{} has no leading .TH title macro",
                path.display()
            );
            pages += 1;
        }
        assert!(pages > 0, "an empty dir would pass the loop vacuously");
    }
}
