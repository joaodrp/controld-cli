//! `cdctl api`: the raw passthrough (D9). A distinct verb so sandboxes can
//! deny `Bash(cdctl api:*)` while allowing `Bash(cdctl:*)`; GET by default;
//! non-GET gated on `-X` **and** `--yes`; encoding explicit, never sniffed
//! (D9b). Path and `-F` pairs are forwarded raw by design — the escape hatch
//! must not reshape what it carries.

use std::io::{Read, Write};

use clap::Args;
use reqwest::{Method, Url};

use crate::api::client::RawBody;
use crate::cli::Globals;
use crate::error::{Error, Exit};

#[derive(Debug, Args)]
pub struct ApiArgs {
    /// Path relative to the API origin, query string included
    /// (e.g. '/access?device_id=abc')
    #[expect(
        clippy::doc_markdown,
        reason = "doc comments are clap help text; backticks would render literally"
    )]
    #[arg(value_name = "path")]
    pub path: String,

    /// HTTP method; non-GET also requires --yes
    #[arg(
        short = 'X',
        long,
        default_value = "GET",
        value_name = "METHOD",
        value_parser = parse_method
    )]
    pub method: Method,

    /// Form field (repeatable); keys are sent literally — write
    /// 'hostnames[]=a' yourself. Requires a non-GET -X
    // The explicit id keeps this distinct from the global `--fields`
    // projection flag, whose value lookup would otherwise capture -F values.
    #[arg(
        id = "form_field",
        short = 'F',
        long = "field",
        value_name = "key=value"
    )]
    pub form_fields: Vec<String>,

    /// Read a JSON body from stdin, sent verbatim; "-" is the only accepted
    /// value. Requires a non-GET -X
    #[arg(long, value_name = "-", conflicts_with = "form_field")]
    pub input: Option<String>,
}

pub async fn run(args: ApiArgs, globals: &Globals) -> Result<(), Error> {
    super::reject_explicit_json(globals, "api", "the upstream body verbatim")?;
    validate_path(&args.path)?;
    let body = validate_body(&args)?;
    if args.method != Method::GET && !globals.yes {
        return Err(Error::new(
            "confirmation.required",
            format!(
                "cdctl api -X {} can mutate the account; pass --yes to confirm",
                args.method
            ),
            Exit::ConfirmationRequired,
        ));
    }

    let (client, _source) = super::authenticated_client(globals)?;
    let bytes = if args.method == Method::GET {
        client.get_raw(&args.path, "resource").await?
    } else {
        let body = body.map(read_body).transpose()?;
        client
            .write_raw(args.method, &args.path, body, "resource")
            .await?
    };

    // The upstream body verbatim — no added newline; piping the binary
    // /mobileconfig response to a file must not corrupt it.
    std::io::stdout()
        .write_all(&bytes)
        .map_err(|e| Error::stdout_write_failed(&e))?;
    Ok(())
}

/// The validated-but-unread body: stdin is consumed only after every
/// rejection ran, so a gated command never half-drains a pipe.
#[derive(Debug)]
enum PendingBody {
    Ready(RawBody),
    Stdin,
}

fn read_body(body: PendingBody) -> Result<RawBody, Error> {
    match body {
        PendingBody::Ready(body) => Ok(body),
        PendingBody::Stdin => {
            let mut raw = Vec::new();
            std::io::stdin()
                .read_to_end(&mut raw)
                .map_err(|e| Error::usage(format!("could not read the body from stdin: {e}")))?;
            Ok(RawBody::Json(raw))
        }
    }
}

/// D9 parse-time rejection, pre-auth: absolute URLs, scheme-relative forms,
/// and userinfo all resolve off-origin when joined. Escaping is a property
/// of the path's shape alone, so probing against a fixed base runs the same
/// join-and-compare algorithm the client enforces at its choke point.
fn validate_path(path: &str) -> Result<(), Error> {
    let probe = Url::parse("https://cdctl-path-probe.invalid").expect("the probe base is valid");
    crate::api::client::join_pinned_to_origin(&probe, path)
        .map(drop)
        .map_err(|_| {
            Error::usage(format!(
                "{path:?} is not a relative path; pass one relative to the API origin, \
                 e.g. \"/users\" (absolute and scheme-relative URLs are rejected)"
            ))
        })
}

/// Both body forms require a non-GET `-X` — no documented GET takes a body.
/// (`-F` vs `--input` exclusivity is enforced by clap.)
fn validate_body(args: &ApiArgs) -> Result<Option<PendingBody>, Error> {
    let has_body = !args.form_fields.is_empty() || args.input.is_some();
    if has_body && args.method == Method::GET {
        return Err(Error::usage(
            "a request body requires a non-GET -X <method>; no documented GET takes one",
        ));
    }
    if let Some(input) = &args.input {
        if input != "-" {
            return Err(Error::usage(format!(
                "--input accepts only \"-\" (stdin), got {input:?}"
            )));
        }
        return Ok(Some(PendingBody::Stdin));
    }
    if args.form_fields.is_empty() {
        return Ok(None);
    }
    let pairs = args
        .form_fields
        .iter()
        .map(|field| {
            field
                .split_once('=')
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .ok_or_else(|| Error::usage(format!("-F expects key=value, got {field:?}")))
        })
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Some(PendingBody::Ready(RawBody::Form(pairs))))
}

fn parse_method(raw: &str) -> Result<Method, String> {
    Method::from_bytes(raw.to_ascii_uppercase().as_bytes())
        .map_err(|_| format!("{raw:?} is not an HTTP method"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> ApiArgs {
        use clap::Parser;
        match crate::cli::Cli::parse_from([&["cdctl", "api"], args].concat()).command {
            crate::cli::Command::Api(args) => args,
            other => panic!("expected the api command, parsed {other:?}"),
        }
    }

    /// Regression: `-F` and the global `--fields` share a name; without a
    /// distinct arg id, the global's value lookup captured `-F` values and
    /// every `cdctl api -F ...` died on the explicit-JSON rejection.
    #[test]
    fn form_fields_do_not_leak_into_the_global_fields_flag() {
        use clap::Parser;
        let cli = crate::cli::Cli::parse_from(["cdctl", "api", "/x", "-X", "POST", "-F", "a=b"]);
        assert_eq!(cli.globals.fields, None);
        assert!(!cli.globals.json);
    }

    #[test]
    fn method_parses_case_insensitively_and_defaults_to_get() {
        assert_eq!(parse(&["/users"]).method, Method::GET);
        assert_eq!(parse(&["/users", "-X", "post"]).method, Method::POST);
        assert_eq!(parse(&["/users", "-X", "DELETE"]).method, Method::DELETE);
    }

    #[test]
    fn paths_that_leave_the_origin_are_rejected() {
        for path in [
            "https://evil.example/x",
            "//evil.example/x",
            "/\\evil.example/x",
            "\\\\evil.example/x",
            "https://user:pw@evil.example/x",
            "mailto:x@example.com",
        ] {
            let error = validate_path(path).expect_err("rejected");
            assert_eq!(error.exit(), Exit::Usage, "path {path:?}");
        }
        for path in ["/users", "users", "/access?device_id=abc", ""] {
            assert!(validate_path(path).is_ok(), "path {path:?} is relative");
        }
    }

    #[test]
    fn body_forms_require_a_non_get_method() {
        let error = validate_body(&parse(&["/x", "-F", "a=b"])).expect_err("GET with a form");
        assert_eq!(error.exit(), Exit::Usage);
        let error =
            validate_body(&parse(&["/x", "-X", "GET", "--input", "-"])).expect_err("explicit GET");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(validate_body(&parse(&["/x", "-X", "POST", "-F", "a=b"])).is_ok());
    }

    #[test]
    fn form_pairs_split_on_the_first_equals_only() {
        let body = validate_body(&parse(&["/x", "-X", "POST", "-F", "name=a=b", "-F", "k="]))
            .expect("valid")
            .expect("present");
        let PendingBody::Ready(RawBody::Form(pairs)) = body else {
            panic!("form body expected");
        };
        assert_eq!(
            pairs,
            vec![
                ("name".to_owned(), "a=b".to_owned()),
                ("k".to_owned(), String::new())
            ]
        );

        let error = validate_body(&parse(&["/x", "-X", "POST", "-F", "no-equals"]))
            .expect_err("k=v required");
        assert_eq!(error.exit(), Exit::Usage);
    }

    #[test]
    fn input_accepts_only_stdin() {
        let error = validate_body(&parse(&["/x", "-X", "PUT", "--input", "file.json"]))
            .expect_err("files are not accepted");
        assert_eq!(error.exit(), Exit::Usage);
        assert!(matches!(
            validate_body(&parse(&["/x", "-X", "PUT", "--input", "-"])),
            Ok(Some(PendingBody::Stdin))
        ));
    }

    #[test]
    fn field_and_input_are_mutually_exclusive() {
        use clap::Parser;
        let result = crate::cli::Cli::try_parse_from([
            "cdctl", "api", "/x", "-X", "PUT", "-F", "a=b", "--input", "-",
        ]);
        assert!(result.is_err(), "clap enforces the conflict (exit 2)");
    }
}
