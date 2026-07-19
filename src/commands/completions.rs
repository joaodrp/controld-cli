//! `cdctl completions`: the shell completion script, generated from the
//! command tree. An artifact command — works with no token (D7), and its
//! output is the script itself, never JSON.

use crate::cli::Globals;
use crate::error::Error;

pub fn run(shell: clap_complete::Shell, globals: &Globals) -> Result<(), Error> {
    super::reject_explicit_json(globals, "completions", "a shell script")?;
    // Generated into a buffer first: `generate` reports write errors by
    // panicking, so writing straight to stdout could leak exit 101 — a failed
    // artifact write must be the checked exit 1 like every other data path.
    let mut script = Vec::new();
    clap_complete::generate(shell, &mut crate::cli::command(), "cdctl", &mut script);
    crate::output::print_raw(&script)
}
