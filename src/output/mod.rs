//! Output contract (D2, D3, D4): stdout carries only data; JSON is always
//! pretty-printed, 2-space indent, trailing newline, identical piped or not.

pub mod time;

use std::borrow::Cow;

use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Human,
    Json,
}

pub fn print_json<T: Serialize>(value: &T) {
    let doc = serde_json::to_string_pretty(value).expect("output types serialize");
    println!("{doc}");
}

/// The one data-output entry point: JSON mode always honors `--fields`,
/// human mode runs the caller's renderer. Handlers never branch on [`Mode`]
/// or project fields themselves, so `--fields` stays uniform.
pub fn emit<T: Serialize>(mode: Mode, fields: Option<&[String]>, value: &T, human: impl FnOnce()) {
    match mode {
        Mode::Json => {
            let mut doc = serde_json::to_value(value).expect("output types serialize");
            if let Some(fields) = fields {
                let mut matched = std::collections::HashSet::new();
                doc = project_tracking(doc, fields, &mut matched);
                // A typo'd field must not become silent data loss with exit 0.
                for field in fields {
                    if !matched.contains(field.as_str()) {
                        eprintln!("warning: --fields: no such field \"{field}\"");
                    }
                }
            }
            print_json(&doc);
        }
        Mode::Human => human(),
    }
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
pub fn print_key_values(pairs: &[(&str, String)]) {
    let width = pairs.iter().map(|(k, _)| k.len()).max().unwrap_or(0);
    for (key, value) in pairs {
        println!("{key:width$}  {}", escape_controls(value));
    }
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
