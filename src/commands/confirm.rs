//! D8 tiered confirmation: prompt on a real TTY, fail loud (exit 7)
//! otherwise. `--yes` is honored only when the target profile is
//! **explicit** (`--profile` or `CONTROLD_PROFILE`) — an implicit
//! `default_profile` target is ambient state the caller may have forgotten,
//! so `--yes` is ignored for it rather than trusted (D8) — said via a stderr
//! notice before the prompt, or inside the exit-7 error when there is none.

use std::io::{IsTerminal, Write};

use crate::commands::scope::ProfileScope;
use crate::error::{Error, Exit};

/// `prompt` is the full question, without the `[y/N]` suffix (added here).
pub async fn confirm(prompt: &str, yes: bool, scope: &ProfileScope) -> Result<(), Error> {
    if yes && scope.explicit {
        return Ok(());
    }

    let interactive = std::io::stdin().is_terminal() && std::io::stderr().is_terminal();
    if interactive {
        // Reached only when `yes && scope.explicit` is false (the early
        // return above), so `yes` alone means "yes, but the implicit scope
        // ignores it". The non-interactive path says the same thing inside
        // its error instead — one explanation per stream, never both.
        if yes {
            eprintln!(
                "info: --yes is ignored: the target profile is implicit (config's \
                 default_profile); pass --profile or set CONTROLD_PROFILE to confirm \
                 non-interactively"
            );
        }
        eprint!("{prompt} [y/N] ");
        let _ = std::io::stderr().flush();
        // Off the runtime thread: a blocking read here would starve main's
        // select of its SIGINT branch and make Ctrl-C at the prompt appear
        // dead (same treatment as the stdin body read in `api`).
        let line = tokio::task::spawn_blocking(|| {
            let mut line = String::new();
            std::io::stdin().read_line(&mut line).map(|_| line)
        })
        .await
        .map_err(|e| Error::generic(format!("the confirmation reader task failed: {e}")))?
        .map_err(|e| Error::generic(format!("could not read the confirmation from stdin: {e}")))?;
        return if matches!(line.trim().to_ascii_lowercase().as_str(), "y" | "yes") {
            Ok(())
        } else {
            Err(Error::new(
                "confirmation.declined",
                "confirmation declined",
                Exit::ConfirmationRequired,
            ))
        };
    }

    if yes {
        return Err(Error::new(
            "confirmation.required",
            "the target profile is implicit (config's default_profile); --yes is ignored for \
             implicit targets",
            Exit::ConfirmationRequired,
        )
        .with_hint(
            "pass --profile <id|name> or set CONTROLD_PROFILE to confirm non-interactively",
        ));
    }
    Err(Error::new(
        "confirmation.required",
        format!("confirmation required: {prompt}"),
        Exit::ConfirmationRequired,
    )
    .with_hint("pass --yes to confirm non-interactively"))
}

#[cfg(test)]
mod tests {
    use super::*;

    // `cargo test` never runs attached to a TTY, so every case here exercises
    // the non-interactive branch — the interactive prompt path is covered by
    // manual testing only.

    fn scope(explicit: bool) -> ProfileScope {
        ProfileScope {
            id: "pk1".to_owned(),
            name: "Test".to_owned(),
            explicit,
        }
    }

    #[tokio::test]
    async fn yes_with_an_explicit_scope_needs_no_prompt() {
        confirm("delete it?", true, &scope(true))
            .await
            .expect("explicit --yes is honored");
    }

    #[tokio::test]
    async fn yes_with_an_implicit_scope_is_ignored() {
        let error = confirm("delete it?", true, &scope(false))
            .await
            .expect_err("yes ignored, non-interactive");
        assert_eq!(error.exit(), Exit::ConfirmationRequired);
        assert_eq!(error.code, "confirmation.required");
        assert!(error.message.contains("implicit"));
    }

    #[tokio::test]
    async fn no_yes_at_all_is_confirmation_required() {
        let error = confirm("delete it?", false, &scope(true))
            .await
            .expect_err("no --yes, non-interactive");
        assert_eq!(error.exit(), Exit::ConfirmationRequired);
        assert_eq!(error.code, "confirmation.required");
    }
}
