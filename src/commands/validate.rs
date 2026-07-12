//! Input hardening (commands.md#input-hardening): reject hostile argument
//! values with exit 2 **before any request is built**. `cdctl api` is exempt
//! (gated by D9 instead).

use crate::error::Error;

/// Every argument value: no control characters (C1 included — same stance as
/// the output escaping).
pub fn reject_control_chars(value: &str, what: &str) -> Result<(), Error> {
    if value.chars().any(char::is_control) {
        return Err(Error::usage(format!(
            "{what} {value:?} contains a control character"
        )));
    }
    Ok(())
}

/// Values bound into URL path segments additionally reject `?`/`#`/`%` —
/// an embedded query or fragment start, or pre-encoded input; values are
/// taken literally, never pre-encoded. `*` stays legal (rule grammar; it is
/// percent-encoded at the HTTP layer).
pub fn validate_path_bound(value: &str, what: &str) -> Result<(), Error> {
    reject_control_chars(value, what)?;
    if let Some(offender) = value.chars().find(|c| matches!(c, '?' | '#' | '%')) {
        return Err(Error::usage(format!(
            "{what} {value:?} contains {offender:?}; values are taken literally, never pre-encoded"
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::Exit;

    #[test]
    fn control_characters_are_rejected_everywhere() {
        for hostile in ["a\u{7}b", "a\nb", "a\u{7f}b", "a\u{9b}b"] {
            let error = reject_control_chars(hostile, "the name").expect_err("rejected");
            assert_eq!(error.exit(), Exit::Usage);
        }
        reject_control_chars("plain.example.com", "the hostname").expect("legal");
    }

    #[test]
    fn path_bound_values_reject_url_metacharacters() {
        for hostile in ["a?b=c", "a#frag", "a%2Fb"] {
            let error = validate_path_bound(hostile, "the hostname").expect_err("rejected");
            assert_eq!(error.exit(), Exit::Usage);
        }
    }

    #[test]
    fn wildcards_stay_legal_in_path_bound_values() {
        validate_path_bound("*.example.com", "the hostname").expect("rule grammar");
    }
}
