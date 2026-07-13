//! Entry point: parse, dispatch, map [`error::Error`] to its D5 exit code.
//! The process boundary owns three things Rust gives nobody for free:
//! SIGINT -> 130, SIGPIPE -> the conventional 141 (never a panic), and a
//! checked stdout flush — exit 0 must mean the data on stdout is complete.

mod api;
mod cli;
mod commands;
mod config;
mod error;
mod model;
mod output;

use std::io::Write;
use std::process::ExitCode;

use clap::Parser;

use cli::{Cli, Command, Globals};
use error::{Error, Exit};

fn main() -> ExitCode {
    reset_sigpipe();
    run()
}

/// Rust ignores SIGPIPE, so `cdctl reference | head` would panic with exit
/// 101; the default disposition dies quietly with 141 instead (D5). Sockets
/// are unaffected: tokio sends with `MSG_NOSIGNAL`/`SO_NOSIGPIPE`.
fn reset_sigpipe() {
    #[cfg(unix)]
    // SAFETY: installing a default disposition before any other thread exists.
    unsafe {
        libc::signal(libc::SIGPIPE, libc::SIG_DFL);
    }
}

// One sequential request at a time — a worker pool would be pure startup cost.
#[tokio::main(flavor = "current_thread")]
async fn run() -> ExitCode {
    let cli = Cli::parse(); // usage errors exit 2 here, from clap
    let json_explicit = cli.globals.json || cli.globals.fields.is_some();
    let globals = match Globals::resolve(cli.globals) {
        Ok(globals) => globals,
        Err(err) => {
            err.render_to_stderr(json_explicit, false);
            return err.exit().into();
        }
    };

    let result = tokio::select! {
        biased;
        interrupt = wait_for_sigint() => Err(interrupt),
        result = dispatch(cli.command, &globals) => result,
    };

    // A lost buffered tail must never exit 0 — success asserts complete stdout.
    let result = match std::io::stdout().flush() {
        Ok(()) => result,
        Err(flush_err) => result.and(Err(Error::stdout_write_failed(&flush_err))),
    };

    match result {
        Ok(()) => Exit::Success.into(),
        Err(err) => {
            err.render_to_stderr(globals.json(), globals.debug);
            // SIGINT leaves the process directly: a normal return drops the
            // runtime, whose shutdown waits on blocking tasks — a Ctrl-C
            // during a stdin read would turn into a hang until EOF.
            if err.exit() == Exit::Interrupt {
                std::process::exit(i32::from(Exit::Interrupt as u8));
            }
            err.exit().into()
        }
    }
}

/// Resolves only on a real SIGINT. A failed handler registration must not
/// impersonate one (silent exit 130); it warns and lets the command run.
async fn wait_for_sigint() -> Error {
    match tokio::signal::ctrl_c().await {
        Ok(()) => Error::interrupted(),
        Err(e) => {
            eprintln!("warning: could not install the SIGINT handler: {e}");
            std::future::pending().await
        }
    }
}

async fn dispatch(command: Command, globals: &Globals) -> Result<(), Error> {
    match command {
        Command::Api(args) => commands::api::run(args, globals).await,
        Command::Auth(command) => commands::auth::run(command, globals).await,
        Command::Profile(command) => commands::profile::run(command, globals).await,
        Command::Folder(command) => commands::folder::run(command, globals).await,
        Command::Rule(command) => commands::rule::run(command, globals).await,
        Command::Config(command) => commands::config::run(command, globals),
        Command::Completions { shell } => commands::completions::run(shell, globals),
        Command::Reference => commands::reference::run(globals),
    }
}
