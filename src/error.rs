//! The D4 error envelope, the D5 exit codes, and the D4b classification of
//! upstream responses. The envelope's JSON shape is public API: stable slugs,
//! verbatim upstream, stable key set — every member present, `null` when absent.

use serde::Serialize;

/// Hint for `auth.missing_token`/`auth.invalid_token` (D6).
const AUTH_HINT: &str = "Run `cdctl auth login --token-stdin`, or set CONTROLD_API_TOKEN.";

/// Exit codes are public API ([D5](../docs/decisions.md)): `8` is retryable,
/// everything else is terminal. Codes 9-19 are reserved, append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Success = 0,
    Generic = 1,
    Usage = 2,
    NotFound = 3,
    Auth = 4,
    Forbidden = 5,
    Conflict = 6,
    ConfirmationRequired = 7,
    Retryable = 8,
    Interrupt = 130,
}

/// The code list only — no leading "Exit codes:" line; callers supply their
/// own header (`cli::Cli`'s `after_long_help` and `commands::reference`'s
/// `## Exit codes` section both render this verbatim). One line per
/// [`Exit`] variant; `exit_codes_help_matches_every_exit_variant_exhaustively`
/// (below) is the drift guard: no `_` arm means a new `Exit` variant fails to
/// compile until it gets a match arm there; the guard's assert then fails
/// until this constant gains the matching line too.
pub const EXIT_CODES_HELP: &str = concat!(
    "  0    ok\n",
    "  1    generic error\n",
    "  2    usage error\n",
    "  3    not found\n",
    "  4    auth\n",
    "  5    forbidden / plan required\n",
    "  6    conflict\n",
    "  7    confirmation required\n",
    "  8    retryable (the only retryable code)\n",
    "  130  interrupted (SIGINT)\n",
    // 141 is not an `Exit` variant: `main`'s `reset_sigpipe` restores the
    // default handler and the kernel reports 128+SIGPIPE. It is still part of
    // the documented contract (design.md D5), so the advertised list carries
    // it; the drift guard pins it as the one non-variant line.
    "  141  broken pipe (SIGPIPE, Unix)",
);

impl From<Exit> for std::process::ExitCode {
    fn from(exit: Exit) -> Self {
        Self::from(exit as u8)
    }
}

/// The D4 error envelope; [`Error::to_json_document`] wraps it in the
/// `{"error": {...}}` stderr document. `retryable` and `exit` are private and
/// derived together at construction, so `exit 8 <=> retryable` cannot drift.
#[derive(Debug, Clone, Serialize, thiserror::Error)]
#[error("{message}")]
pub struct Error {
    pub code: String,
    pub message: String,
    pub upstream: Option<Upstream>,
    retryable: bool,
    pub retry_after: Option<u64>,
    pub details: Option<Details>,
    pub hint: Option<String>,
    #[serde(skip)]
    exit: Exit,
    /// Diagnostics surfaced only under `--debug` (e.g. a code/status mismatch).
    #[serde(skip)]
    pub debug_notes: Vec<String>,
}

/// The upstream error, verbatim. `None` members are serialized as `null` —
/// the 0-byte 500 yields `{"code": null, "http_status": 500, "message": null}`.
#[derive(Debug, Clone, Serialize)]
pub struct Upstream {
    pub code: Option<i64>,
    pub http_status: Option<u16>,
    pub message: Option<String>,
}

/// The `details` payload on aggregate errors (D4). `kind` discriminates; inside
/// each variant every key is always present.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Details {
    MultiTarget {
        targets: Vec<TargetOutcome>,
        /// Shell-neutral argv covering only the failed targets: resolved ids,
        /// never names or ambient defaults; deletes carry `--yes`.
        retry_argv: Vec<String>,
    },
    #[allow(
        dead_code,
        reason = "constructed from Phase 4 (rule import collisions); schema frozen here"
    )]
    Collisions { collisions: Vec<Collision> },
    #[allow(
        dead_code,
        reason = "constructed from Phase 4 (rule import unconvergeable states); schema frozen here"
    )]
    Unconvergeable { rules: Vec<UnconvergeableRule> },
}

/// One per-target row. Constructor-only: `Failed` always carries its slug and
/// retryability, `Landed`/`Skipped` never do — a correlation D4 promises but
/// the struct alone cannot express.
#[derive(Debug, Clone, Serialize)]
pub struct TargetOutcome {
    target: String,
    outcome: Outcome,
    /// The stable CLI slug — never the numeric upstream code.
    code: Option<String>,
    retryable: Option<bool>,
    upstream: Option<Upstream>,
}

impl TargetOutcome {
    pub fn landed(target: impl Into<String>) -> Self {
        Self::terminal(target, Outcome::Landed)
    }

    pub fn skipped(target: impl Into<String>) -> Self {
        Self::terminal(target, Outcome::Skipped)
    }

    /// From a classified [`Error`], so the slug is structurally the CLI's,
    /// never upstream's number.
    pub fn failed(target: impl Into<String>, error: &Error) -> Self {
        Self {
            target: target.into(),
            outcome: Outcome::Failed,
            code: Some(error.code.clone()),
            retryable: Some(error.retryable),
            upstream: error.upstream.clone(),
        }
    }

    fn terminal(target: impl Into<String>, outcome: Outcome) -> Self {
        Self {
            target: target.into(),
            outcome,
            code: None,
            retryable: None,
            upstream: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Outcome {
    Landed,
    Failed,
    /// Not attempted after an abort.
    Skipped,
}

#[derive(Debug, Clone, Serialize)]
pub struct Collision {
    pub hostname: String,
    pub folder_id: i64,
    pub folder: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct UnconvergeableRule {
    pub hostname: String,
    /// Stable slug, e.g. `via6-clear`.
    pub reason: String,
    pub current_via6: Option<String>,
    pub desired_via6: Option<String>,
}

impl Error {
    pub fn new(code: impl Into<String>, message: impl Into<String>, exit: Exit) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
            upstream: None,
            retryable: exit == Exit::Retryable,
            retry_after: None,
            details: None,
            hint: None,
            exit,
            debug_notes: Vec::new(),
        }
    }

    pub fn usage(message: impl Into<String>) -> Self {
        Self::new("usage.invalid", message, Exit::Usage)
    }

    pub fn generic(message: impl Into<String>) -> Self {
        Self::new("request.invalid", message, Exit::Generic)
    }

    pub fn config_invalid(message: impl Into<String>) -> Self {
        Self::new("config.invalid", message, Exit::Generic)
    }

    /// A failed stdout write or flush: exit 0 must mean the data on stdout
    /// is complete, so this is fatal wherever it surfaces.
    pub fn stdout_write_failed(err: &std::io::Error) -> Self {
        Self::new(
            "output.write_failed",
            format!("could not write to stdout: {err}"),
            Exit::Generic,
        )
    }

    pub fn auth_missing_token() -> Self {
        Self::new(
            "auth.missing_token",
            "not authenticated: no API token found",
            Exit::Auth,
        )
        .with_hint(AUTH_HINT)
    }

    /// SIGINT. Rendered silently; the exit code is the contract.
    pub fn interrupted() -> Self {
        Self::new("interrupted", "interrupted", Exit::Interrupt)
    }

    pub fn exit(&self) -> Exit {
        self.exit
    }

    pub fn retryable(&self) -> bool {
        self.retryable
    }

    #[must_use]
    pub fn with_hint(mut self, hint: impl Into<String>) -> Self {
        self.hint = Some(hint.into());
        self
    }

    #[must_use]
    pub fn with_upstream(mut self, upstream: Upstream) -> Self {
        self.upstream = Some(upstream);
        self
    }

    #[must_use]
    pub fn with_retry_after(mut self, retry_after: Option<u64>) -> Self {
        self.retry_after = retry_after;
        self
    }

    #[must_use]
    pub fn with_debug_note(mut self, note: impl Into<String>) -> Self {
        self.debug_notes.push(note.into());
        self
    }

    #[must_use]
    pub fn with_details(mut self, details: Details) -> Self {
        self.details = Some(details);
        self
    }

    /// The complete JSON error document — `{"error": {...}}`, pretty-printed,
    /// trailing newline. JSON mode emits exactly one of these on stderr.
    pub fn to_json_document(&self) -> String {
        #[derive(Serialize)]
        struct Document<'a> {
            error: &'a Error,
        }
        let mut doc = serde_json::to_string_pretty(&Document { error: self })
            .expect("the error envelope contains no non-serializable values");
        doc.push('\n');
        doc
    }

    /// Render to stderr. Human mode: one sanitized `error:` line plus `hint:`;
    /// JSON mode: exactly one envelope document. `--debug` prepends the notes
    /// in both modes and, in human mode, adds the upstream message
    /// JSON-escaped — visible, never raw bytes.
    pub fn render_to_stderr(&self, json: bool, debug: bool) {
        if self.exit == Exit::Interrupt {
            return;
        }
        if debug {
            for note in &self.debug_notes {
                eprintln!("debug: {}", collapse_to_single_line(note));
            }
        }
        if json {
            eprint!("{}", self.to_json_document());
            return;
        }
        eprintln!("error: {}", collapse_to_single_line(&self.message));
        // Nested (not a let-chain): let-chains stabilized after the 1.85 MSRV.
        if debug {
            if let Some(upstream_message) = self.upstream.as_ref().and_then(|u| u.message.as_ref())
            {
                // JSON-escaping keeps it verbatim yet never executable.
                let escaped = serde_json::to_string(upstream_message)
                    .expect("strings are always JSON-serializable");
                eprintln!("debug: upstream.message = {escaped}");
            }
        }
        if let Some(hint) = &self.hint {
            eprintln!("hint: {}", collapse_to_single_line(hint));
        }
    }
}

/// One clean terminal line from untrusted text (D4): CR/LF/TAB become spaces,
/// every other control (C0, DEL, C1 — ANSI included) is stripped, whitespace
/// runs collapse. Verbatim text belongs in JSON `upstream.message`, not here.
pub fn collapse_to_single_line(text: &str) -> String {
    let stripped: String = text
        .chars()
        .filter_map(|c| match c {
            '\r' | '\n' | '\t' => Some(' '),
            c if c.is_control() => None,
            c => Some(c),
        })
        .collect();
    let mut line = String::with_capacity(stripped.len());
    for word in stripped.split_whitespace() {
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    line
}

/// Classify an upstream error per D4b: the 3-digit prefix of `error.code`
/// carries the weight; the HTTP status is the fallback when no code exists.
/// `resource` is the noun of the command that ran (`"profile"`, `"rule"`) —
/// the resource half of a slug never comes from the message.
pub fn classify_response(
    http_status: u16,
    upstream_code: Option<i64>,
    upstream_message: Option<&str>,
    resource: &str,
    retry_after: Option<u64>,
) -> Error {
    let upstream = Upstream {
        code: upstream_code,
        http_status: Some(http_status),
        message: upstream_message.map(str::to_owned),
    };

    let code_prefix = upstream_code.and_then(status_prefix_of_code);
    let effective_status = code_prefix.unwrap_or(http_status);

    let mut error = classify_effective(effective_status, upstream_code, upstream_message, resource);
    if let Some(prefix) = code_prefix {
        if prefix != http_status {
            // D4b: trust `error.code`; surface the mismatch under --debug.
            error = error.with_debug_note(format!(
                "error.code prefix {prefix} disagrees with HTTP status {http_status}; trusting error.code"
            ));
        }
    }
    error.with_upstream(upstream).with_retry_after(retry_after)
}

/// Classify a response whose body is empty or not JSON. A non-2xx classifies
/// on its HTTP status with a fully-`null`-membered upstream; a 2xx is
/// `upstream.error`, exit 8 — success cannot be confirmed.
pub fn classify_unparseable(http_status: u16, resource: &str, retry_after: Option<u64>) -> Error {
    let upstream = Upstream {
        code: None,
        http_status: Some(http_status),
        message: None,
    };
    let error = if (200..300).contains(&http_status) {
        Error::new(
            "upstream.error",
            format!(
                "the API returned HTTP {http_status} with an unreadable body; success cannot be confirmed"
            ),
            Exit::Retryable,
        )
    } else {
        classify_effective(http_status, None, None, resource)
    };
    error.with_upstream(upstream).with_retry_after(retry_after)
}

/// A response whose shape defies what an operation verified: success cannot
/// be confirmed, so it is always exit 8 (error-codes.md, defensive parsing).
pub(crate) fn upstream_shape(detail: impl std::fmt::Display) -> Error {
    Error::new(
        "upstream.error",
        format!("unexpected API response shape: {detail}; success cannot be confirmed"),
        Exit::Retryable,
    )
}

/// A `success: true` body riding a non-2xx status (a middlebox replaying a
/// cached success, never observed live): success cannot be confirmed.
pub fn classify_unconfirmed_success(http_status: u16, retry_after: Option<u64>) -> Error {
    Error::new(
        "upstream.error",
        format!(
            "the API returned HTTP {http_status} with a success body; success cannot be confirmed"
        ),
        Exit::Retryable,
    )
    .with_upstream(Upstream {
        code: None,
        http_status: Some(http_status),
        message: None,
    })
    .with_retry_after(retry_after)
}

/// `--debug` evidence for a body that defied envelope parsing — an HTML block
/// page or truncated document is diagnosable only from the body itself.
pub fn unparseable_body_note(body: &[u8], parse_err: &serde_json::Error) -> String {
    const SNIPPET_LEN: usize = 200;
    if body.is_empty() {
        return format!("empty body did not parse as an envelope: {parse_err}");
    }
    let snippet = collapse_to_single_line(&String::from_utf8_lossy(
        &body[..SNIPPET_LEN.min(body.len())],
    ));
    format!(
        "body ({} bytes) did not parse as an envelope: {parse_err}; starts: {snippet:?}",
        body.len()
    )
}

/// A reqwest transport failure — no response to classify.
pub fn classify_transport(err: &reqwest::Error) -> Error {
    let message = if err.is_timeout() {
        "request timed out".to_owned()
    } else if err.is_connect() {
        "could not connect to the API".to_owned()
    } else {
        format!("network error: {err}")
    };
    Error::new("network.error", message, Exit::Retryable)
}

/// The prefix rule: `40401 -> 404`. An implausible prefix is discarded —
/// classification falls back to the real HTTP status.
fn status_prefix_of_code(code: i64) -> Option<u16> {
    let prefix = u16::try_from(code / 100).ok()?;
    (100..=599).contains(&prefix).then_some(prefix)
}

fn classify_effective(
    status: u16,
    upstream_code: Option<i64>,
    upstream_message: Option<&str>,
    resource: &str,
) -> Error {
    // The 400 trap: auth runs before routing, so a missing/invalid token is
    // 400/40001, never 401.
    if upstream_code == Some(40001) {
        return if upstream_message.is_some_and(|m| m.contains("No session token")) {
            Error::auth_missing_token()
        } else {
            Error::new(
                "auth.invalid_token",
                upstream_message.unwrap_or("invalid API token"),
                Exit::Auth,
            )
            .with_hint(AUTH_HINT)
        };
    }

    let message = |what: &str| {
        upstream_message.map_or_else(|| format!("{what} (HTTP {status})"), str::to_owned)
    };
    match status {
        400 => {
            // Best-effort refinement (error-codes.md): upstream rewording
            // degrades a conflict to request.invalid — still terminal.
            if upstream_message.is_some_and(|m| m.contains("already exists")) {
                Error::new(
                    format!("{resource}.conflict"),
                    upstream_message.unwrap_or_default(),
                    Exit::Conflict,
                )
            } else {
                Error::new(
                    "request.invalid",
                    message("the API rejected the request"),
                    Exit::Generic,
                )
            }
        }
        401 => Error::new("auth.denied", message("authentication failed"), Exit::Auth),
        402 => Error::new(
            "plan.upgrade_required",
            message("this action needs a higher plan"),
            Exit::Forbidden,
        ),
        403 => Error::new(
            "permission.denied",
            message("permission denied"),
            Exit::Forbidden,
        ),
        404 => Error::new(
            format!("{resource}.not_found"),
            message("not found"),
            Exit::NotFound,
        ),
        429 => Error::new(
            "ratelimit.exceeded",
            message("rate limited"),
            Exit::Retryable,
        ),
        500..=599 => Error::new("upstream.error", message("upstream error"), Exit::Retryable),
        405..=428 | 430..=499 => Error::new(
            "request.invalid",
            message("the API rejected the request"),
            Exit::Generic,
        ),
        // 1xx/3xx/2xx-with-error: anomalous, success unconfirmable.
        _ => Error::new(
            "upstream.error",
            message("unexpected response"),
            Exit::Retryable,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The drift guard `EXIT_CODES_HELP`'s doc comment promises: no `_` arm
    /// means a new `Exit` variant fails to compile until it gets a match arm
    /// here; the assert below then fails until `EXIT_CODES_HELP` gains the
    /// matching line. Each expected line is derived from the variant's own
    /// discriminant (`variant as u8`), not a second hand-typed copy of the
    /// numbers — a mismatched number, not just a missing description, now
    /// fails the assert too.
    #[test]
    fn exit_codes_help_matches_every_exit_variant_exhaustively() {
        fn description(exit: Exit) -> &'static str {
            match exit {
                Exit::Success => "ok",
                Exit::Generic => "generic error",
                Exit::Usage => "usage error",
                Exit::NotFound => "not found",
                Exit::Auth => "auth",
                Exit::Forbidden => "forbidden / plan required",
                Exit::Conflict => "conflict",
                Exit::ConfirmationRequired => "confirmation required",
                Exit::Retryable => "retryable (the only retryable code)",
                Exit::Interrupt => "interrupted (SIGINT)",
            }
        }
        let variants = [
            Exit::Success,
            Exit::Generic,
            Exit::Usage,
            Exit::NotFound,
            Exit::Auth,
            Exit::Forbidden,
            Exit::Conflict,
            Exit::ConfirmationRequired,
            Exit::Retryable,
            Exit::Interrupt,
        ];
        let rebuilt = variants
            .iter()
            .map(|&exit| format!("  {:<4} {}", exit as u8, description(exit)))
            .collect::<Vec<_>>()
            .join("\n")
            // 141 is kernel-reported (128+SIGPIPE), never an `Exit` variant —
            // the one advertised line that cannot be derived from the enum.
            + "\n  141  broken pipe (SIGPIPE, Unix)";
        assert_eq!(rebuilt, EXIT_CODES_HELP);
        assert_eq!(
            EXIT_CODES_HELP.lines().count(),
            variants.len() + 1,
            "a forgotten array entry above must not silently pass with fewer lines than variants"
        );
    }

    #[track_caller]
    fn assert_classified(
        http_status: u16,
        code: Option<i64>,
        message: Option<&str>,
        resource: &str,
        want_slug: &str,
        want_exit: Exit,
    ) {
        let error = classify_response(http_status, code, message, resource, None);
        assert_eq!(error.code, want_slug, "slug for {code:?} {message:?}");
        assert_eq!(error.exit(), want_exit, "exit for {code:?} {message:?}");
        assert_eq!(error.retryable(), want_exit == Exit::Retryable);
    }

    /// Every observed row of reference/error-codes.md.
    #[test]
    #[expect(clippy::too_many_lines, reason = "one row per observed error code")]
    fn observed_error_code_table_classifies() {
        let rows: &[(i64, &str, &str, &str, Exit)] = &[
            (
                40001,
                "No session token provided",
                "auth",
                "auth.missing_token",
                Exit::Auth,
            ),
            (
                40001,
                "Invalid session, please login again.",
                "auth",
                "auth.invalid_token",
                Exit::Auth,
            ),
            (
                40002,
                "config is required",
                "rule",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40002,
                "profile_id is required",
                "rule",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "Invalid import file",
                "rule",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "hostnames must be an array",
                "rule",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "Invalid rule action was provided",
                "rule",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "do must be one of Array\n(\n    [0] => 0\n)\n",
                "rule",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "Custom Rule already exists: d.example.com",
                "rule",
                "rule.conflict",
                Exit::Conflict,
            ),
            (
                40003,
                "A device with this name already exists",
                "device",
                "device.conflict",
                Exit::Conflict,
            ),
            (
                40003,
                "Name must be a minimum of 3 characters",
                "profile",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "Failed to create or modify custom rule(s)",
                "rule",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "Invalid filter name 'foo' at index 0",
                "filter",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "Invalid service was provided",
                "service",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "Via_v6 must be a minimum of 1 characters",
                "rule",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "Invalid service rule action was provided",
                "service",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40003,
                "You have reached the maximum number of custom rules",
                "rule",
                "request.invalid",
                Exit::Generic,
            ),
            (
                40201,
                "You need the Full Control plan to perform this action.",
                "rule",
                "plan.upgrade_required",
                Exit::Forbidden,
            ),
            (
                40401,
                "No such group exists.",
                "folder",
                "folder.not_found",
                Exit::NotFound,
            ),
            (
                40401,
                "Invalid category",
                "category",
                "category.not_found",
                Exit::NotFound,
            ),
            (
                40401,
                "You have no organizations associated with your account",
                "org",
                "org.not_found",
                Exit::NotFound,
            ),
        ];
        for (code, message, resource, slug, exit) in rows {
            // The API sends 400 for the 40x anomalies and the real status
            // otherwise; the prefix must win regardless, so classify with the
            // matching status here and prove mismatch-tolerance separately.
            let http = u16::try_from(code / 100).expect("test codes have valid prefixes");
            assert_classified(http, Some(*code), Some(message), resource, slug, *exit);
        }
    }

    #[test]
    fn code_prefix_wins_over_http_status() {
        // 402 body delivered with (hypothetically) a 400 transport status:
        // trust the code, note the mismatch for --debug.
        let error = classify_response(
            400,
            Some(40201),
            Some("You need the Full Control plan to perform this action."),
            "rule",
            None,
        );
        assert_eq!(error.code, "plan.upgrade_required");
        assert_eq!(error.exit(), Exit::Forbidden);
        assert_eq!(error.debug_notes.len(), 1, "mismatch must be noted");
        let upstream = error.upstream.expect("classified errors carry upstream");
        assert_eq!(
            upstream.http_status,
            Some(400),
            "upstream keeps the real HTTP status"
        );
    }

    /// success:false with no error code: classify on the HTTP status alone.
    #[test]
    fn codeless_errors_classify_on_the_http_status() {
        assert_classified(
            404,
            None,
            None,
            "profile",
            "profile.not_found",
            Exit::NotFound,
        );
        assert_classified(
            429,
            None,
            None,
            "rule",
            "ratelimit.exceeded",
            Exit::Retryable,
        );
        assert_classified(500, None, None, "rule", "upstream.error", Exit::Retryable);
        assert_classified(
            403,
            None,
            None,
            "rule",
            "permission.denied",
            Exit::Forbidden,
        );
        assert_classified(401, None, None, "rule", "auth.denied", Exit::Auth);
    }

    #[test]
    fn unparseable_bodies_classify_with_a_synthesized_upstream() {
        // Non-2xx: classify on the status, upstream fully null-membered.
        let e = classify_unparseable(500, "rule", None);
        assert_eq!(
            (e.code.as_str(), e.exit()),
            ("upstream.error", Exit::Retryable)
        );
        let upstream = e.upstream.expect("synthesized upstream");
        assert_eq!(upstream.code, None);
        assert_eq!(upstream.http_status, Some(500));
        assert_eq!(upstream.message, None);

        // 2xx: success cannot be confirmed, so still exit 8.
        let e = classify_unparseable(200, "rule", None);
        assert_eq!(
            (e.code.as_str(), e.exit()),
            ("upstream.error", Exit::Retryable)
        );
    }

    #[test]
    fn an_implausible_code_prefix_falls_back_to_the_http_status() {
        assert_classified(404, Some(7), None, "rule", "rule.not_found", Exit::NotFound);
    }

    #[test]
    fn retry_after_is_carried() {
        let error = classify_response(429, None, None, "rule", Some(17));
        assert_eq!(error.retry_after, Some(17));
        assert!(error.retryable());
    }

    #[test]
    fn client_side_errors_carry_null_upstream() {
        let error = Error::usage("--via requires --action spoof or --action redirect");
        assert!(error.upstream.is_none());
        let json = error.to_json_document();
        let value: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(value["error"]["upstream"], serde_json::Value::Null);
        // Stable key set: every D4 member present even when null.
        let keys: Vec<&String> = value["error"].as_object().expect("object").keys().collect();
        assert_eq!(
            keys,
            [
                "code",
                "message",
                "upstream",
                "retryable",
                "retry_after",
                "details",
                "hint"
            ]
        );
    }

    #[test]
    fn collapse_flattens_the_multiline_php_dump() {
        let php = "do must be one of Array\n(\n    [0] => 0\n    [1] => 1\n)\n ";
        assert_eq!(
            collapse_to_single_line(php),
            "do must be one of Array ( [0] => 0 [1] => 1 )"
        );
    }

    #[test]
    fn collapse_joins_crlf_lines() {
        assert_eq!(
            collapse_to_single_line("line one\r\nline two"),
            "line one line two"
        );
    }

    #[test]
    fn collapse_strips_ansi_escapes() {
        let ansi = "bad \u{1b}[31mred\u{1b}[0m input";
        assert_eq!(collapse_to_single_line(ansi), "bad [31mred[0m input");
        assert!(!collapse_to_single_line(ansi).contains('\u{1b}'));
    }

    #[test]
    fn multi_target_details_serialize_to_the_d4_shape() {
        // Through the constructors: a Failed row derives its members from an
        // already-classified Error, so the slug is the CLI's by construction.
        let failure = classify_response(500, Some(50001), Some("boom"), "rule", None);
        let multi = Details::MultiTarget {
            targets: vec![
                TargetOutcome::landed("a.com"),
                TargetOutcome::failed("b.com", &failure),
                TargetOutcome::skipped("c.com"),
            ],
            retry_argv: vec![
                "cdctl".into(),
                "rule".into(),
                "create".into(),
                "b.com".into(),
            ],
        };
        let value = serde_json::to_value(&multi).expect("serializable");
        assert_eq!(value["kind"], "multi_target");
        assert_eq!(value["targets"][0]["outcome"], "landed");
        assert_eq!(value["targets"][0]["code"], serde_json::Value::Null);
        assert_eq!(value["targets"][1]["code"], "upstream.error");
        assert_eq!(value["targets"][1]["retryable"], true);
        assert_eq!(value["targets"][1]["upstream"]["code"], 50001);
        assert_eq!(value["targets"][2]["outcome"], "skipped");
        assert_eq!(value["retry_argv"][0], "cdctl");
        // Every key present on every row, whatever the outcome (stable key set).
        for row in value["targets"].as_array().expect("array") {
            let keys: Vec<&String> = row.as_object().expect("object").keys().collect();
            assert_eq!(keys, ["target", "outcome", "code", "retryable", "upstream"]);
        }
    }

    #[test]
    fn collision_details_serialize_to_the_d4_shape() {
        let collisions = Details::Collisions {
            collisions: vec![Collision {
                hostname: "x.example".into(),
                folder_id: 3,
                folder: "Work".into(),
            }],
        };
        let value = serde_json::to_value(&collisions).expect("serializable");
        assert_eq!(value["kind"], "collisions");
        assert!(
            value.get("retry_argv").is_none(),
            "collisions carry no retry_argv"
        );
    }

    #[test]
    fn unconvergeable_details_serialize_to_the_d4_shape() {
        let unconvergeable = Details::Unconvergeable {
            rules: vec![UnconvergeableRule {
                hostname: "v6.example".into(),
                reason: "via6-clear".into(),
                current_via6: Some("2001:db8::1".into()),
                desired_via6: None,
            }],
        };
        let value = serde_json::to_value(&unconvergeable).expect("serializable");
        assert_eq!(value["kind"], "unconvergeable");
        assert_eq!(value["rules"][0]["desired_via6"], serde_json::Value::Null);
    }
}
