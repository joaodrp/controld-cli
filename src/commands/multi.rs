//! Noun-agnostic aggregation of per-target multi-write outcomes into one D4
//! `write.partial_failure` error (commands.md#rule, decisions.md D4's
//! aggregation rule). Shared by `rule create/update/delete`; nothing here
//! names "rule" so a future multi-target noun (`access add/remove`) can
//! reuse it.

use crate::error::{Details, Error, Exit, TargetOutcome};

/// One target's outcome from a multi-target write, before aggregation. Holds
/// the full classified [`Error`] (never just its slug) so [`aggregate`] can
/// derive both the [`TargetOutcome`] row and the overall exit code from it —
/// `TargetOutcome` itself is serialization-only and hides retryability.
#[derive(Debug)]
pub enum TargetResult {
    Landed,
    /// Boxed: `Error` is large (a `Details::MultiTarget` variant nests two
    /// `Vec`s), and `Landed`/`Skipped` carry nothing — an unboxed `Error`
    /// would size every row to its largest variant.
    Failed(Box<Error>),
    /// Not attempted after an abort (`rule delete`'s stop-on-first-failure).
    Skipped,
}

/// Fold per-target outcomes into one D4 `details.multi_target` error.
/// `results` must be in the original target order — `targets` preserves it
/// verbatim. Callers build `retry_argv` themselves: only they know the verb
/// and flags a retry needs (commands.md#rule).
///
/// Panics if `results` carries no failure — aggregation exists to explain a
/// failure, and an all-landed batch has nothing to aggregate. A
/// `debug_assert!` catches the violation early in debug builds with a named
/// contract; release builds still panic, downstream, at the first `.expect`
/// below that assumes a non-empty failed slice.
pub fn aggregate(
    resource: &'static str,
    results: &[(String, TargetResult)],
    retry_argv: Vec<String>,
) -> Error {
    let landed = results
        .iter()
        .filter(|(_, r)| matches!(r, TargetResult::Landed))
        .count();
    let skipped = results
        .iter()
        .filter(|(_, r)| matches!(r, TargetResult::Skipped))
        .count();
    let failed: Vec<&Error> = results
        .iter()
        .filter_map(|(_, r)| match r {
            TargetResult::Failed(error) => Some(error.as_ref()),
            TargetResult::Landed | TargetResult::Skipped => None,
        })
        .collect();
    debug_assert!(
        !failed.is_empty(),
        "aggregate is only called when at least one target failed"
    );
    // The dangerous direction only: a landed hostname in `retry_argv` would
    // duplicate-POST it on retry and fail the whole chunk upstream. The
    // converse — every failed target present — is *not* asserted: a caller
    // with its own split (verify_create's absent-vs-mismatch) deliberately
    // excludes mismatched targets from `retry_argv`, since resending them
    // would duplicate-POST too.
    debug_assert!(
        results
            .iter()
            .filter(|(_, r)| matches!(r, TargetResult::Landed))
            .all(|(target, _)| !retry_argv.contains(target)),
        "a landed target must never appear in retry_argv"
    );

    let exit = aggregate_exit(&failed);
    // The max, not the first or the sum: D4 promises agents never re-derive
    // backoff, so the aggregate must wait out the longest-lived target.
    let retry_after = failed.iter().filter_map(|error| error.retry_after).max();
    // The first failure's own message, so human mode (counts only otherwise)
    // says *why* without a `--json` round trip; `collapse_to_single_line`
    // applies at render, so a multi-line upstream dump is still safe here.
    let (first_target, first_error) = results
        .iter()
        .find_map(|(target, result)| match result {
            TargetResult::Failed(error) => Some((target, error)),
            TargetResult::Landed | TargetResult::Skipped => None,
        })
        .expect(
            "aggregate requires at least one failed target; a debug_assert above enforces \
             this in debug builds, this expect enforces it in release",
        );
    let message = format!(
        "{} of {} {resource}s failed (first: {first_target}: {}); {landed} landed, {skipped} skipped",
        failed.len(),
        results.len(),
        first_error.message,
    );
    let targets = results
        .iter()
        .map(|(target, result)| match result {
            TargetResult::Landed => TargetOutcome::landed(target.clone()),
            TargetResult::Skipped => TargetOutcome::skipped(target.clone()),
            TargetResult::Failed(error) => TargetOutcome::failed(target.clone(), error),
        })
        .collect();

    // A blind re-run only makes sense when every failure is retryable
    // (exit 8) — a terminal aggregate (e.g. delete's 404) would fail
    // identically. Either way, human mode renders only `error:`/`hint:`
    // lines, so the hint must stand alone and name `--json` rather than
    // point at `retry_argv`, an envelope field human mode never prints.
    // `retry_argv`'s own coverage is the caller's promise, not this
    // function's — the two-kind split (failed-only vs failed-and-skipped)
    // holds for every caller here, but a caller with its own split
    // (verify_create's absent-vs-mismatch) must override via
    // `.with_hint(...)` afterwards.
    let hint = if exit == Exit::Retryable {
        if skipped > 0 {
            "re-run the retry_argv from the JSON error envelope (--json); it covers the failed \
             and skipped targets"
        } else {
            "re-run the retry_argv from the JSON error envelope (--json); it covers the failed \
             targets"
        }
    } else {
        "inspect details.targets in the JSON error envelope (--json); the failed targets are \
         not retryable as-is"
    };

    Error::new("write.partial_failure", message, exit)
        .with_details(Details::MultiTarget {
            targets,
            retry_argv,
        })
        .with_retry_after(retry_after)
        .with_hint(hint)
}

/// D4's aggregation rule: exit 8 iff every failed target is retryable; else
/// the one shared exit if every failure (terminal or not) agrees, else
/// generic 1 — a single retryable failure alongside a terminal one is
/// already a disagreement, so it falls to 1 too, not the terminal code.
fn aggregate_exit(failed: &[&Error]) -> Exit {
    let mut exits = failed.iter().map(|error| error.exit());
    let first = exits.next().expect(
        "aggregate requires at least one failed target; a debug_assert above enforces \
             this in debug builds, this expect enforces it in release",
    );
    if exits.all(|exit| exit == first) {
        first
    } else {
        Exit::Generic
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Exit;

    fn failed(exit: Exit) -> TargetResult {
        TargetResult::Failed(Box::new(Error::new("x.test", "boom", exit)))
    }

    fn failed_with_retry_after(exit: Exit, retry_after: Option<u64>) -> TargetResult {
        TargetResult::Failed(Box::new(
            Error::new("x.test", "boom", exit).with_retry_after(retry_after),
        ))
    }

    /// The dangerous direction: a landed hostname wrongly present in
    /// `retry_argv` would duplicate-POST it on retry and fail the whole
    /// chunk. Debug-only (`debug_assert!`), so this test only runs where
    /// `cfg(debug_assertions)` holds — release builds accept the caller's
    /// `retry_argv` uninspected, same as every other `debug_assert!` here.
    #[test]
    #[should_panic(expected = "landed")]
    #[cfg(debug_assertions)]
    fn a_landed_target_in_retry_argv_trips_the_debug_assert() {
        aggregate(
            "rule",
            &[
                ("a.com".into(), TargetResult::Landed),
                ("b.com".into(), failed(Exit::Retryable)),
            ],
            vec!["a.com".into()],
        );
    }

    #[test]
    fn aggregate_carries_the_max_retry_after_across_failed_targets() {
        let error = aggregate(
            "rule",
            &[
                (
                    "a.com".into(),
                    failed_with_retry_after(Exit::Retryable, Some(7)),
                ),
                (
                    "b.com".into(),
                    failed_with_retry_after(Exit::Retryable, Some(17)),
                ),
            ],
            vec![],
        );
        assert_eq!(error.retry_after, Some(17));
    }

    #[test]
    fn aggregate_retry_after_is_null_when_no_failed_target_carries_one() {
        let error = aggregate(
            "rule",
            &[
                (
                    "a.com".into(),
                    failed_with_retry_after(Exit::Retryable, None),
                ),
                (
                    "b.com".into(),
                    failed_with_retry_after(Exit::Retryable, None),
                ),
            ],
            vec![],
        );
        assert_eq!(error.retry_after, None);
    }

    #[test]
    fn all_retryable_failures_yield_exit_8() {
        let error = aggregate(
            "rule",
            &[
                ("a.com".into(), TargetResult::Landed),
                ("b.com".into(), failed(Exit::Retryable)),
                ("c.com".into(), failed(Exit::Retryable)),
            ],
            vec!["cdctl".into()],
        );
        assert_eq!(error.exit(), Exit::Retryable);
        assert!(error.retryable());
        assert_eq!(error.code, "write.partial_failure");
    }

    #[test]
    fn a_shared_terminal_exit_wins_when_none_are_retryable() {
        let error = aggregate(
            "rule",
            &[
                ("a.com".into(), failed(Exit::NotFound)),
                ("b.com".into(), failed(Exit::NotFound)),
            ],
            vec![],
        );
        assert_eq!(error.exit(), Exit::NotFound);
    }

    #[test]
    fn mixed_terminal_and_retryable_failures_yield_exit_1() {
        let error = aggregate(
            "rule",
            &[
                ("a.com".into(), failed(Exit::Retryable)),
                ("b.com".into(), failed(Exit::NotFound)),
            ],
            vec![],
        );
        assert_eq!(error.exit(), Exit::Generic);
    }

    #[test]
    fn mixed_terminal_exits_yield_exit_1() {
        let error = aggregate(
            "rule",
            &[
                ("a.com".into(), failed(Exit::NotFound)),
                ("b.com".into(), failed(Exit::Conflict)),
            ],
            vec![],
        );
        assert_eq!(error.exit(), Exit::Generic);
    }

    #[test]
    fn landed_and_skipped_rows_carry_null_code_and_retryable() {
        let error = aggregate(
            "rule",
            &[
                ("a.com".into(), TargetResult::Landed),
                ("b.com".into(), failed(Exit::NotFound)),
                ("c.com".into(), TargetResult::Skipped),
            ],
            vec!["cdctl".into(), "rule".into()],
        );
        let value = serde_json::to_value(&error).expect("serializable");
        let targets = value["details"]["targets"].as_array().expect("array");
        assert_eq!(targets[0]["outcome"], "landed");
        assert_eq!(targets[0]["code"], serde_json::Value::Null);
        assert_eq!(targets[0]["retryable"], serde_json::Value::Null);
        assert_eq!(targets[2]["outcome"], "skipped");
        assert_eq!(targets[2]["code"], serde_json::Value::Null);
        assert_eq!(value["details"]["retry_argv"][0], "cdctl");
    }

    #[test]
    fn message_names_the_first_failures_target_and_cause() {
        let error = aggregate(
            "rule",
            &[
                ("a.com".into(), TargetResult::Landed),
                ("b.com".into(), failed(Exit::Retryable)),
                ("c.com".into(), TargetResult::Skipped),
            ],
            vec![],
        );
        assert_eq!(
            error.message,
            "1 of 3 rules failed (first: b.com: boom); 1 landed, 1 skipped"
        );
    }

    #[test]
    fn retryable_aggregate_hint_names_the_json_envelope_and_covers_skipped_too() {
        let error = aggregate(
            "rule",
            &[
                ("a.com".into(), failed(Exit::Retryable)),
                ("b.com".into(), TargetResult::Skipped),
            ],
            vec![],
        );
        assert_eq!(error.exit(), Exit::Retryable);
        let hint = error.hint.expect("hint present");
        assert!(hint.contains("--json"), "got: {hint}");
        assert!(hint.contains("failed and skipped"), "got: {hint}");
    }

    #[test]
    fn retryable_aggregate_hint_covers_only_failed_when_none_skipped() {
        let error = aggregate("rule", &[("a.com".into(), failed(Exit::Retryable))], vec![]);
        let hint = error.hint.expect("hint present");
        assert!(!hint.contains("skipped"), "got: {hint}");
    }

    #[test]
    fn terminal_aggregate_hint_never_advises_a_blind_rerun() {
        let error = aggregate("rule", &[("a.com".into(), failed(Exit::NotFound))], vec![]);
        assert_eq!(error.exit(), Exit::NotFound);
        let hint = error.hint.expect("hint present");
        assert!(!hint.contains("re-run"), "got: {hint}");
        assert!(hint.contains("not retryable"), "got: {hint}");
    }
}
