//! `cdctl completions`: the shell completion script, generated from the
//! command tree. An artifact command — works with no token (D7), and its
//! output is the script itself, never JSON.

use std::io;

use crate::cli::Globals;
use crate::error::Error;

pub fn run(shell: clap_complete::Shell, globals: &Globals) -> Result<(), Error> {
    super::reject_explicit_json(globals, "completions", "a shell script")?;
    clap_complete::generate(
        shell,
        &mut crate::cli::command(),
        "cdctl",
        &mut io::stdout(),
    );
    Ok(())
}
