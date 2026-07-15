//! Helpers shared by the integration-test crates. Each `tests/*.rs` file
//! compiles as its own crate, so shared code has to live in a subdirectory
//! module like this one (cargo does not build `tests/*/mod.rs` as a target).

use assert_cmd::Command;

/// A hermetic `cdctl`: empty environment (no COLUMNS/TERM to vary help wrap
/// width), config under a private tempdir.
pub fn cdctl(config_home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("cdctl").expect("binary builds");
    cmd.env_clear().env("XDG_CONFIG_HOME", config_home);
    cmd
}

pub fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}
