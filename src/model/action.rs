//! The API's action encoding: `do` 0-3 plus `status`/`via`/`via_v6`
//! siblings. Integers never leak into output or input (D2/D10) — they map to
//! names here and nowhere else.

use serde::{Deserialize, Serialize};

use crate::error::Error;

/// A rule action by name. What a *missing* `do` means is the caller's call:
/// on a rule it is an error (never a default — a silent BLOCK is the worst
/// failure a DNS tool can have), on a folder it is the action-less state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Block,
    Bypass,
    Spoof,
    Redirect,
}

/// Serializes as its name — the same string `name()` returns, never the wire
/// `do` integer (D2/D10).
impl Serialize for Action {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

impl Action {
    pub fn from_do(value: i64) -> Result<Self, Error> {
        match value {
            0 => Ok(Self::Block),
            1 => Ok(Self::Bypass),
            2 => Ok(Self::Spoof),
            3 => Ok(Self::Redirect),
            other => Err(crate::error::upstream_shape(format_args!(
                "unknown action do={other}"
            ))),
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Block => "block",
            Self::Bypass => "bypass",
            Self::Spoof => "spoof",
            Self::Redirect => "redirect",
        }
    }

    /// The wire `do` integer, mirror of [`Action::from_do`]. Sole legal
    /// caller: the form encoding in `commands::action_flags` — the one point
    /// where names become wire ints (D2/D10 keep them out of output and
    /// argv).
    pub fn to_do(self) -> i64 {
        match self {
            Self::Block => 0,
            Self::Bypass => 1,
            Self::Spoof => 2,
            Self::Redirect => 3,
        }
    }
}

impl std::fmt::Display for Action {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.name())
    }
}

/// The wire `action` object as rules, folders, and the default rule carry it.
#[derive(Debug, Deserialize)]
#[cfg_attr(test, serde(deny_unknown_fields))]
pub struct ApiAction {
    #[serde(rename = "do", default)]
    pub r#do: Option<i64>,
    #[serde(default)]
    pub status: Option<i64>,
    #[serde(default)]
    pub via: Option<String>,
    #[serde(default)]
    #[allow(dead_code, reason = "read from the rule commands (next PR)")]
    pub via_v6: Option<String>,
}

impl ApiAction {
    /// Maps the wire `do`, if present. `Ok(None)` means absent — the caller
    /// decides whether that is an error (a rule) or the action-less state (a
    /// folder); a present-but-unknown value is always a shape error.
    pub fn action(&self) -> Result<Option<Action>, Error> {
        self.r#do.map(Action::from_do).transpose()
    }

    /// `status` defaults to enabled when absent — unlike `do`, absence is not
    /// dangerous: the server itself stores an omitted `status` as 1
    /// (write-verification.md), so absence reads as enabled.
    pub fn enabled(&self) -> Result<bool, Error> {
        match self.status {
            None | Some(1) => Ok(true),
            Some(0) => Ok(false),
            Some(other) => Err(crate::error::upstream_shape(format_args!(
                "unknown status={other}"
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Exit;

    #[test]
    fn each_do_integer_maps_to_its_name() {
        for (value, name) in [(0, "block"), (1, "bypass"), (2, "spoof"), (3, "redirect")] {
            assert_eq!(Action::from_do(value).expect("known value").name(), name);
        }
    }

    #[test]
    fn to_do_round_trips_with_from_do() {
        for (value, action) in [
            (0, Action::Block),
            (1, Action::Bypass),
            (2, Action::Spoof),
            (3, Action::Redirect),
        ] {
            assert_eq!(action.to_do(), value);
            assert_eq!(Action::from_do(value).expect("known value"), action);
        }
    }

    #[test]
    fn unknown_do_is_a_shape_error_never_a_default() {
        let error = Action::from_do(9).expect_err("unknown action");
        assert_eq!(error.exit(), Exit::Retryable);
        assert!(error.message.contains("do=9"));
    }

    #[test]
    fn an_unknown_status_is_a_shape_error() {
        let action = ApiAction {
            r#do: Some(1),
            status: Some(2),
            via: None,
            via_v6: None,
        };
        let error = action.enabled().expect_err("unknown status");
        assert_eq!(error.exit(), Exit::Retryable);
        assert!(error.message.contains("status=2"));
    }
}
