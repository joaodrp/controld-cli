//! Output contract (D2, D3, D4): stdout carries only data; JSON is always
//! pretty-printed, 2-space indent, trailing newline, identical piped or not.

pub mod time;

use std::borrow::Cow;

use serde::Serialize;

use crate::error::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Human,
    Json,
}

/// The one emitter of advisory `info:` stderr lines, so the prefix and the
/// `--quiet` gate live in a single place. Callers with a `Globals` in hand
/// use [`Globals::info`](crate::cli::Globals::info); this free function
/// serves the one caller below `Globals` (the API client).
/// A failed stderr write is ignored — advisories must never fail a command,
/// and `eprintln!` would panic instead.
pub fn info(quiet: bool, message: impl std::fmt::Display) {
    use std::io::Write;
    if !quiet {
        let _ = writeln!(std::io::stderr().lock(), "info: {message}");
    }
}

/// Checked data write, `println!` semantics: `doc` plus a trailing newline.
/// A write failure is `Error::stdout_write_failed` (exit 1), never a
/// `println!` panic leaking the undocumented 101 (D5). EPIPE never reaches
/// here on Unix — SIGPIPE keeps its default disposition, so a closed pipe
/// kills the process with 141 before the write returns.
pub fn print_doc(doc: impl std::fmt::Display) -> Result<(), Error> {
    use std::io::Write;
    writeln!(std::io::stdout().lock(), "{doc}").map_err(|e| Error::stdout_write_failed(&e))
}

/// Verbatim variant of [`print_doc`] for artifacts that own their trailing
/// bytes (`reference`, `completions`, the `api` passthrough) — no added
/// newline, binary-safe.
pub fn print_raw(bytes: &[u8]) -> Result<(), Error> {
    use std::io::Write;
    std::io::stdout()
        .lock()
        .write_all(bytes)
        .map_err(|e| Error::stdout_write_failed(&e))
}

pub fn print_json<T: Serialize>(value: &T) -> Result<(), Error> {
    let doc = serde_json::to_string_pretty(value).expect("output types serialize");
    print_doc(&doc)
}

/// The one data-output entry point: JSON mode always honors `--fields`,
/// human mode runs the caller's renderer. Handlers never branch on [`Mode`]
/// or project fields themselves, so `--fields` stays uniform.
///
/// Matching is shallow, not "anywhere in the document": a top-level key on
/// the document itself, or on each element when the document is an array
/// ([`project_tracking`]'s single-level key match). A requested field that
/// matches none of those is a usage error, not a silent no-op: a typo'd
/// field must not become silent data loss with exit 0. A field present on
/// *some* but not *every* array element still projects cleanly (each object
/// keeps only the fields it has), since that's the ordinary shape of
/// heterogeneous API data, not a typo. An empty top-level array is the one
/// exception: there are no elements to disagree with the request, so the
/// check is vacuous and `[]` prints — an empty `rule list`/`folder list`
/// must not fail a request that would succeed non-empty.
pub fn emit<T: Serialize>(
    mode: Mode,
    fields: Option<&[String]>,
    value: &T,
    human: impl FnOnce() -> Result<(), Error>,
) -> Result<(), Error> {
    match mode {
        Mode::Json => {
            let mut doc = serde_json::to_value(value).expect("output types serialize");
            if let Some(fields) = fields {
                // Vacuous on an empty top-level array: `project_tracking`
                // never visits an element to (mis)match against, so there is
                // nothing for a requested field to legitimately disagree
                // with — failing here would turn an empty `rule list`/
                // `folder list` into a false "no such field", precisely on
                // the input a sweep across every profile is most likely to
                // hit.
                let is_vacuous_empty =
                    matches!(&doc, serde_json::Value::Array(items) if items.is_empty());
                let mut matched = std::collections::HashSet::new();
                doc = project_tracking(doc, fields, &mut matched);
                if !is_vacuous_empty {
                    if let Some(field) = fields.iter().find(|f| !matched.contains(f.as_str())) {
                        return Err(Error::usage(format!("--fields: no such field \"{field}\"")));
                    }
                }
            }
            print_json(&doc)?;
        }
        Mode::Human => human()?,
    }
    Ok(())
}

/// The mutating handlers' upfront `--fields` check (rule/folder
/// `create`/`update`): run before the write, unlike [`emit`]'s check, which
/// only fires on the post-write read-back — a typo there would perform the
/// mutation, then exit 2 with nothing printed (misreading as "nothing
/// happened, retry", which duplicate-POSTs a create). `allowed` is the
/// handler's row type's canonical top-level field list (its `FIELDS`
/// const); matching is exactly [`project_tracking`]'s single-level key
/// match, so this never rejects a field `emit` would actually project. Not a
/// replacement for `emit`'s check, which stays the general mechanism for
/// read paths and the backstop everywhere else.
pub(crate) fn validate_fields(requested: Option<&[String]>, allowed: &[&str]) -> Result<(), Error> {
    let Some(requested) = requested else {
        return Ok(());
    };
    if let Some(field) = requested.iter().find(|f| !allowed.contains(&f.as_str())) {
        return Err(Error::usage(format!("--fields: no such field \"{field}\"")));
    }
    Ok(())
}

/// `--fields a,b`: project each object down to the named keys, preserving
/// their order in the object. Arrays project element-wise; scalars pass
/// through. Records which requested fields matched anywhere — one traversal
/// serves both the projection and the typo warning.
fn project_tracking<'f>(
    value: serde_json::Value,
    fields: &'f [String],
    matched: &mut std::collections::HashSet<&'f str>,
) -> serde_json::Value {
    use serde_json::Value;
    match value {
        Value::Object(map) => Value::Object(
            map.into_iter()
                .filter(|(key, _)| match fields.iter().find(|field| *field == key) {
                    Some(field) => {
                        matched.insert(field.as_str());
                        true
                    }
                    None => false,
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| project_tracking(item, fields, matched))
                .collect(),
        ),
        other => other,
    }
}

/// Make server-supplied strings terminal-safe (the D4 rule applied to success
/// output): C0/DEL are caret-escaped (`\x1b` -> `^[`), C1 (U+0080-U+009F,
/// honored as 8-bit CSI by some terminals) is stripped like the error path
/// does. Used for table cells and human key/value output.
pub fn escape_controls(text: &str) -> Cow<'_, str> {
    if !text.chars().any(char::is_control) {
        return Cow::Borrowed(text);
    }
    let mut escaped = String::with_capacity(text.len() + 8);
    for c in text.chars() {
        if c.is_ascii_control() {
            escaped.push('^');
            escaped.push(((c as u8) ^ 0x40) as char);
        } else if !c.is_control() {
            escaped.push(c);
        }
    }
    Cow::Owned(escaped)
}

/// Human-mode key/value lines (e.g. `auth status`), aligned and escaped.
pub fn print_key_values(pairs: &[(&str, String)]) -> Result<(), Error> {
    use std::io::Write;
    let width = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    let mut out = std::io::stdout().lock();
    for (key, value) in pairs {
        writeln!(out, "{key:width$}  {}", escape_controls(value))
            .map_err(|e| Error::stdout_write_failed(&e))?;
    }
    Ok(())
}

/// Default human view. `plain` drops borders for `awk`/`cut` (D3: piping
/// never changes representation; `--plain` is an explicit rendering choice).
pub fn render_table(headers: &[&str], rows: Vec<Vec<String>>, plain: bool) -> comfy_table::Table {
    use comfy_table::presets;
    let mut table = comfy_table::Table::new();
    table.load_preset(if plain {
        presets::NOTHING
    } else {
        presets::UTF8_FULL_CONDENSED
    });
    table.set_header(headers.to_vec());
    for row in rows {
        table.add_row(row.into_iter().map(|cell| match escape_controls(&cell) {
            Cow::Borrowed(_) => cell,
            Cow::Owned(escaped) => escaped,
        }));
    }
    table
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn json_is_pretty_with_two_space_indent() {
        let doc = serde_json::to_string_pretty(&json!({"a": [1]})).expect("serializes");
        assert_eq!(doc, "{\n  \"a\": [\n    1\n  ]\n}");
    }

    #[test]
    fn validate_fields_accepts_known_fields_and_a_missing_request() {
        assert!(validate_fields(None, &["a", "b"]).is_ok());
        assert!(validate_fields(Some(&["a".to_owned()]), &["a", "b"]).is_ok());
    }

    #[test]
    fn validate_fields_rejects_an_unknown_field() {
        let error = validate_fields(Some(&["a".to_owned(), "nope".to_owned()]), &["a", "b"])
            .expect_err("nope is not allowed");
        assert!(
            error.message.contains("no such field \"nope\""),
            "got: {}",
            error.message
        );
    }

    #[test]
    fn fields_project_objects_and_arrays() {
        let value = json!([{"a": 1, "b": 2}, {"a": 3, "c": 4}]);
        let fields = vec!["a".to_owned(), "c".to_owned()];
        assert_eq!(
            project_tracking(value, &fields, &mut std::collections::HashSet::new()),
            json!([{"a": 1}, {"a": 3, "c": 4}])
        );
    }

    #[test]
    fn projection_tracks_which_fields_matched() {
        let mut matched = std::collections::HashSet::new();
        let fields = vec!["a".to_owned(), "c".to_owned(), "emial".to_owned()];
        project_tracking(json!([{"a": 1}, {"c": 4}]), &fields, &mut matched);
        assert!(matched.contains("a") && matched.contains("c"));
        assert!(!matched.contains("emial"), "a typo'd field never matches");
    }

    #[test]
    fn controls_are_caret_escaped_and_c1_is_stripped() {
        assert_eq!(escape_controls("plain"), "plain");
        assert_eq!(escape_controls("bad\x1b[31mname"), "bad^[[31mname");
        assert_eq!(escape_controls("a\x7fb"), "a^?b");
        assert_eq!(escape_controls("tab\there"), "tab^Ihere");
        // C1: U+009B is a single-char CSI on 8-bit-clean terminals.
        assert_eq!(escape_controls("evil\u{9b}31mname"), "evil31mname");
    }

    #[test]
    fn tables_escape_cells_and_plain_drops_borders() {
        let rows = vec![vec!["evil\x1b[2Jname".to_owned(), "1".to_owned()]];
        let bordered = render_table(&["NAME", "ID"], rows.clone(), false).to_string();
        assert!(
            bordered.contains("^[[2J"),
            "ANSI must be escaped: {bordered}"
        );
        assert!(!bordered.contains('\x1b'));

        let plain = render_table(&["NAME", "ID"], rows, true).to_string();
        assert!(!plain.contains('│'), "plain mode has no borders: {plain}");
    }
}
