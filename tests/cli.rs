//! Binary-level contract tests (D18: no lib target — the CLI contract is the
//! public API, so tests drive `cdctl` itself).

use std::process::Stdio;
use std::time::Duration;

use assert_cmd::Command;
use wiremock::matchers::{body_string, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

mod common;
use common::{cdctl, tempdir};

/// Point cdctl at a mock server, authenticated via env token.
fn cdctl_against(server_uri: &str, config_home: &std::path::Path) -> Command {
    let mut cmd = cdctl(config_home);
    cmd.env("CONTROLD_API_URL", server_uri)
        .env("CONTROLD_UNSAFE_BASE_URL", "1")
        .env("CONTROLD_API_TOKEN", "api.test-token");
    cmd
}

fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/api")
            .join(name),
    )
    .expect("fixture exists")
}

fn error_envelope(status: u16, code: i64, message: &str) -> ResponseTemplate {
    ResponseTemplate::new(status).set_body_json(serde_json::json!({
        "body": [],
        "success": false,
        "error": {"code": code, "message": message}
    }))
}

// --- D7: completions and reference need no token, no config, no network ---

#[test]
fn completions_work_without_a_token() {
    let dir = tempdir();
    let assert = cdctl(dir.path())
        .args(["completions", "bash"])
        .assert()
        .success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        stdout.contains("cdctl"),
        "a completion script mentions the binary"
    );
}

#[test]
fn reference_works_without_a_token() {
    let dir = tempdir();
    let assert = cdctl(dir.path()).arg("reference").assert().success();
    let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    for expected in [
        "# cdctl reference",
        "## Global flags",
        "--json",
        "auth login",
    ] {
        assert!(
            stdout.contains(expected),
            "reference must mention {expected:?}"
        );
    }
}

#[test]
fn artifact_commands_reject_explicit_json() {
    let dir = tempdir();
    let man_dir = tempdir();
    let man_out = man_dir.path().to_str().expect("utf8 tempdir path");
    for args in [
        ["completions", "bash", "--json"].as_slice(),
        ["reference", "--json"].as_slice(),
        ["man", "--out-dir", man_out, "--json"].as_slice(),
        ["auth", "logout", "--json"].as_slice(),
        ["config", "get", "current_context", "--json"].as_slice(),
        ["config", "set", "default_profile", "Home", "--json"].as_slice(),
        ["config", "path", "--json"].as_slice(),
    ] {
        let assert = cdctl(dir.path()).args(args).assert().code(2);
        assert.stdout(predicates::str::is_empty());
    }
    // A rejected `config set` must not have written anything.
    let config_file = dir.path().join("cdctl").join("config.toml");
    assert!(
        !config_file.exists(),
        "a rejected `config set --json` must not create a config file"
    );
    // Ambient CONTROLD_OUTPUT=json is not a conflict.
    cdctl(dir.path())
        .env("CONTROLD_OUTPUT", "json")
        .args(["completions", "bash"])
        .assert()
        .success();
    cdctl(dir.path())
        .env("CONTROLD_OUTPUT", "json")
        .args(["auth", "logout"])
        .assert()
        .success();
}

/// `auth login` needs `--token-stdin` plus a stdin body, so it doesn't fit
/// the args-only loop above; exercised separately.
#[test]
fn auth_login_rejects_explicit_json() {
    let dir = tempdir();
    let assert = cdctl(dir.path())
        .args(["auth", "login", "--token-stdin", "--json"])
        .write_stdin("api.test-token\n")
        .assert()
        .code(2);
    assert.stdout(predicates::str::is_empty());
    let config_file = dir.path().join("cdctl").join("config.toml");
    assert!(
        !config_file.exists(),
        "a rejected login must not write a token"
    );
}

// --- `cdctl man` (hidden packaging command) ---

#[test]
fn man_generates_pages_with_empty_stdout() {
    let dir = tempdir();
    let man_dir = tempdir();
    let assert = cdctl(dir.path())
        .args(["man", "--out-dir"])
        .arg(man_dir.path())
        .assert()
        .success();
    assert
        .stdout(predicates::str::is_empty())
        .stderr(predicates::str::is_empty());
    assert!(man_dir.path().join("cdctl.1").exists());
}

#[test]
fn man_out_dir_collision_with_a_file_is_a_loud_generic_error() {
    let dir = tempdir();
    let parent = tempdir();
    let blocking_file = parent.path().join("not-a-dir");
    std::fs::write(&blocking_file, b"occupied").expect("write");
    let assert = cdctl(dir.path())
        .args(["man", "--out-dir"])
        .arg(&blocking_file)
        .assert()
        .code(1);
    assert
        .stdout(predicates::str::is_empty())
        .stderr(predicates::str::contains("could not create"));
}

#[test]
fn usage_errors_exit_2() {
    let dir = tempdir();
    cdctl(dir.path())
        .arg("no-such-command")
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty());
    cdctl(dir.path())
        .args(["auth", "login", "--token", "x"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty());
    // A malformed ambient CONTROLD_OUTPUT must not silently mean human mode.
    cdctl(dir.path())
        .env("CONTROLD_OUTPUT", "Json")
        .args(["config", "path"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty())
        .stderr(predicates::str::contains("CONTROLD_OUTPUT"));
    // Control characters in values are rejected before any write.
    cdctl(dir.path())
        .args(["config", "set", "default_profile", "bad\u{7}name"])
        .assert()
        .code(2);
}

/// `config path` prints the path unconditionally (it is the contract — where
/// cdctl reads and writes, usable before the first write), but flags a
/// not-yet-created file on stderr so the path never reads as a lie.
#[test]
fn config_path_notes_a_file_that_does_not_exist_yet() {
    use predicates::prelude::PredicateBooleanExt;
    let dir = tempdir();
    let config_file = dir.path().join("cdctl").join("config.toml");

    let assert = cdctl(dir.path())
        .args(["config", "path"])
        .assert()
        .success();
    assert
        .stdout(predicates::str::contains(
            config_file.to_str().expect("utf8"),
        ))
        .stderr(predicates::str::contains("not created yet"));

    // Once the file exists, the note disappears.
    cdctl(dir.path())
        .args(["config", "set", "default_profile", "Home"])
        .assert()
        .success();
    let assert = cdctl(dir.path())
        .args(["config", "path"])
        .assert()
        .success();
    assert
        .stdout(predicates::str::contains(
            config_file.to_str().expect("utf8"),
        ))
        .stderr(predicates::str::contains("not created yet").not());
}

/// Item 3(b): `--timeout`'s value parser (`cli::parse_timeout_secs`) rejects
/// `0` with a plain message, not clap's default `1..18446744073709551615`
/// range dump — clap-level, so no server is needed.
#[test]
fn timeout_zero_is_a_clap_level_usage_error() {
    let dir = tempdir();
    cdctl(dir.path())
        .args(["auth", "status", "--timeout", "0"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty())
        .stderr(predicates::str::contains("must be at least 1"));
}

/// Usage errors must outrank auth errors: a locally-invalid action-flag
/// combination is a mistake regardless of whether a token is configured, so
/// it must not be masked by `auth.missing_token` (exit 4) when one isn't.
#[test]
fn local_flag_errors_outrank_a_missing_token() {
    let dir = tempdir();
    let assert = cdctl(dir.path())
        .args(["rule", "create", "x.example.com", "--action", "spoof"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty());
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("requires --via"), "got: {stderr}");
}

// --- Auth without a token: exit 4, stdout empty, one JSON envelope ---

#[test]
fn auth_status_without_token_is_exit_4() {
    let dir = tempdir();
    let assert = cdctl(dir.path())
        .args(["auth", "status", "--json"])
        .assert()
        .code(4)
        .stdout(predicates::str::is_empty());

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    let doc: serde_json::Value =
        serde_json::from_str(&stderr).expect("stderr is one JSON document");
    assert_eq!(doc["error"]["code"], "auth.missing_token");
    assert_eq!(doc["error"]["retryable"], false);
    assert_eq!(doc["error"]["upstream"], serde_json::Value::Null);
}

/// The missing token outranks request-only environment problems: resolving
/// auth is an authenticated command's first concern, so a malformed
/// `CONTROLD_API_URL` must not demote the documented exit 4 to a usage error.
#[test]
fn missing_token_outranks_a_malformed_base_url() {
    let dir = tempdir();
    let assert = cdctl(dir.path())
        .env("CONTROLD_API_URL", "not a url")
        .args(["auth", "status", "--json"])
        .assert()
        .code(4)
        .stdout(predicates::str::is_empty());

    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    let doc: serde_json::Value =
        serde_json::from_str(&stderr).expect("stderr is one JSON document");
    assert_eq!(doc["error"]["code"], "auth.missing_token");
}

// --- Hostile upstream messages: one clean line, verbatim JSON, escaped debug ---

async fn stderr_for(server: &MockServer, dir: &std::path::Path, extra_args: &[&str]) -> String {
    let uri = server.uri();
    let dir = dir.to_owned();
    let args: Vec<String> = extra_args.iter().map(|s| (*s).to_owned()).collect();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, &dir)
            .args(["auth", "status", "--no-retry"])
            .args(&args)
            .assert()
            .stdout(predicates::str::is_empty());
        String::from_utf8_lossy(&assert.get_output().stderr).into_owned()
    })
    .await
    .expect("command runs")
}

async fn snapshot_three_modes(name: &str, body: ResponseTemplate) {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(body)
        .mount(&server)
        .await;
    let dir = tempdir();

    let human = stderr_for(&server, dir.path(), &[]).await;
    let json = stderr_for(&server, dir.path(), &["--json"]).await;
    let debug = stderr_for(&server, dir.path(), &["--debug"]).await;

    assert_eq!(
        human.lines().count(),
        1,
        "human mode is one line: {human:?}"
    );
    assert!(!human.contains('\u{1b}'), "no raw escapes on the terminal");
    assert!(!debug.contains('\u{1b}'), "debug never emits raw escapes");
    serde_json::from_str::<serde_json::Value>(&json).expect("JSON mode is one valid document");

    insta::with_settings!({filters => port_filters()}, {
        insta::assert_snapshot!(format!("{name}_human"), human);
        insta::assert_snapshot!(format!("{name}_json"), json);
        insta::assert_snapshot!(format!("{name}_debug"), debug);
    });
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn multiline_php_dump_renders_clean() {
    let template = ResponseTemplate::new(400)
        .insert_header("content-type", "application/json")
        .set_body_raw(fixture("err_php_dump.json"), "application/json");
    snapshot_three_modes("php_dump", template).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crlf_message_renders_clean() {
    snapshot_three_modes("crlf", error_envelope(400, 40003, "line one\r\nline two")).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ansi_escape_message_renders_clean() {
    snapshot_three_modes(
        "ansi",
        error_envelope(400, 40003, "bad \u{1b}[31mred\u{1b}[0m \u{7}input"),
    )
    .await;
}

// --- The 0-byte JSON 500 ---

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zero_byte_json_500_is_exit_8_with_null_upstream() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(500).insert_header("content-type", "application/json"))
        .mount(&server)
        .await;

    let stderr = stderr_for(&server, tempdir().path(), &["--json"]).await;
    let doc: serde_json::Value = serde_json::from_str(&stderr).expect("valid envelope");
    assert_eq!(doc["error"]["code"], "upstream.error");
    assert_eq!(doc["error"]["retryable"], true);
    assert_eq!(
        doc["error"]["upstream"],
        serde_json::json!({"code": null, "http_status": 500, "message": null})
    );

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args(["auth", "status", "--no-retry"])
            .assert()
            .code(8);
    })
    .await
    .expect("command runs");
}

// --- Timeouts and SIGINT ---

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_response_trips_the_request_timeout() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(20)))
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["auth", "status", "--no-retry", "--timeout", "1", "--json"])
            .timeout(std::time::Duration::from_secs(10))
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr).expect("valid envelope");
        assert_eq!(doc["error"]["code"], "network.error");
        assert_eq!(doc["error"]["retryable"], true);
    })
    .await
    .expect("command runs");
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sigint_during_a_request_exits_130() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    let status = tokio::task::spawn_blocking(move || {
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("cdctl"))
            .env_clear()
            .env("XDG_CONFIG_HOME", dir.path())
            .env("CONTROLD_API_URL", &uri)
            .env("CONTROLD_UNSAFE_BASE_URL", "1")
            .env("CONTROLD_API_TOKEN", "api.test-token")
            .args(["auth", "status"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawns");

        // Let it get the request in flight, then interrupt.
        std::thread::sleep(Duration::from_millis(800));
        kill(
            Pid::from_raw(i32::try_from(child.id()).expect("pid fits")),
            Signal::SIGINT,
        )
        .expect("signal delivered");
        child.wait().expect("wait")
    })
    .await
    .expect("task runs");

    assert_eq!(status.code(), Some(130), "SIGINT contract");
}

// --- auth/config lifecycle against a mock API ---

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn auth_lifecycle_roundtrips() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture("users.json"), "application/json"),
        )
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let with_env = |cmd: &mut Command| {
            cmd.env("CONTROLD_API_URL", &uri)
                .env("CONTROLD_UNSAFE_BASE_URL", "1");
        };

        // login stores the token.
        let mut login = cdctl(dir.path());
        with_env(&mut login);
        login
            .args(["auth", "login", "--token-stdin"])
            .write_stdin("api.stored-token\n")
            .assert()
            .success()
            .stdout(predicates::str::is_empty());

        // status authenticates with the stored token and reports its source.
        let mut status = cdctl(dir.path());
        with_env(&mut status);
        let assert = status.args(["auth", "status", "--json"]).assert().success();
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stdout).expect("data on stdout");
        assert_eq!(doc["authenticated"], true);
        assert_eq!(doc["email"], "user@example.com");
        assert_eq!(doc["region"], "europe");
        assert_eq!(doc["token_source"], "config");

        // --fields projects.
        let mut fields = cdctl(dir.path());
        with_env(&mut fields);
        let assert = fields
            .args(["auth", "status", "--fields", "email"])
            .assert()
            .success();
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stdout).expect("projected JSON");
        assert_eq!(doc, serde_json::json!({"email": "user@example.com"}));

        // env beats config.
        let mut env_wins = cdctl(dir.path());
        with_env(&mut env_wins);
        let assert = env_wins
            .env("CONTROLD_API_TOKEN", "api.env-token")
            .args(["auth", "status", "--json"])
            .assert()
            .success();
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stdout).expect("data");
        assert_eq!(doc["token_source"], "env");

        // logout removes it; status is exit 4 again.
        cdctl(dir.path())
            .args(["auth", "logout"])
            .assert()
            .success();
        let mut after = cdctl(dir.path());
        with_env(&mut after);
        after.args(["auth", "status"]).assert().code(4);
    })
    .await
    .expect("lifecycle runs");
}

#[test]
fn config_lifecycle_roundtrips() {
    let dir = tempdir();

    cdctl(dir.path())
        .args(["config", "get", "current_context"])
        .assert()
        .success()
        .stdout("personal\n");
    cdctl(dir.path())
        .args(["config", "get", "default_profile"])
        .assert()
        .success()
        .stdout("");

    cdctl(dir.path())
        .args(["config", "set", "default_profile", "Home"])
        .assert()
        .success();
    cdctl(dir.path())
        .args(["config", "get", "default_profile"])
        .assert()
        .success()
        .stdout("Home\n");

    let assert = cdctl(dir.path())
        .args(["config", "list", "--json"])
        .assert()
        .success();
    let doc: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).expect("data");
    assert_eq!(doc["context"]["value"], "personal");
    assert_eq!(doc["context"]["source"], "default");
    assert_eq!(
        doc["token"],
        serde_json::json!({"set": false, "source": null})
    );
    assert_eq!(
        doc["default_profile"],
        serde_json::json!({"value": "Home", "source": "config"})
    );

    let assert = cdctl(dir.path())
        .args(["config", "path"])
        .assert()
        .success();
    let path = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
    assert!(
        path.trim_end().ends_with("cdctl/config.toml"),
        "got: {path}"
    );

    // An unknown key is a usage error, not a silent write.
    cdctl(dir.path())
        .args(["config", "set", "no_such_key", "x"])
        .assert()
        .code(2);
}

/// `config list`'s document shape (`context`/`token`/`default_profile`) is
/// known upfront, so a typo'd `--fields` is a usage error like every other
/// noun's upfront check.
#[test]
fn config_list_fields_typo_is_a_usage_error() {
    let dir = tempdir();
    let assert = cdctl(dir.path())
        .args(["config", "list", "--fields", "bogus"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty());
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    let doc: serde_json::Value =
        serde_json::from_str(&stderr).expect("stderr is one JSON document");
    assert_eq!(doc["error"]["code"], "usage.invalid");
    assert!(
        doc["error"]["message"]
            .as_str()
            .expect("message is a string")
            .contains("no such field \"bogus\""),
        "got: {stderr}"
    );
}

// --- D9 origin rules at the binary level ---

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_token_never_reaches_an_unpinned_origin_without_the_switch() {
    let server = MockServer::start().await;
    // The mock proves zero requests leave the process.
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl(dir.path())
            .env("CONTROLD_API_URL", &uri)
            .env("CONTROLD_API_TOKEN", "api.test-token")
            // Deliberately no CONTROLD_UNSAFE_BASE_URL.
            .args(["auth", "status", "--json"])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "usage.unsafe_base_url");
    })
    .await
    .expect("command runs");
    // MockServer verifies .expect(0) on drop.
}

#[test]
fn an_invalid_api_url_is_a_usage_error() {
    let dir = tempdir();
    cdctl(dir.path())
        .env("CONTROLD_API_URL", "not a url")
        .env("CONTROLD_API_TOKEN", "api.test-token")
        .args(["auth", "status"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty());
}

// --- auth login validation (D6) ---

#[test]
fn auth_login_rejects_bad_input_and_writes_nothing() {
    let dir = tempdir();
    let config_file = dir.path().join("cdctl").join("config.toml");

    // Missing --token-stdin.
    cdctl(dir.path())
        .args(["auth", "login"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty());
    // Empty stdin.
    cdctl(dir.path())
        .args(["auth", "login", "--token-stdin"])
        .write_stdin("")
        .assert()
        .code(2);
    // Whitespace/control characters inside the token.
    cdctl(dir.path())
        .args(["auth", "login", "--token-stdin"])
        .write_stdin("api.bad token\n")
        .assert()
        .code(2);
    cdctl(dir.path())
        .args(["auth", "login", "--token-stdin"])
        .write_stdin("api.bad\u{7}token\n")
        .assert()
        .code(2);

    assert!(
        !config_file.exists(),
        "no rejected login may create a config file"
    );
}

#[test]
fn auth_logout_is_idempotent() {
    let dir = tempdir();
    let assert = cdctl(dir.path())
        .args(["auth", "logout"])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("no token stored"), "got: {stderr}");
    let config_file = dir.path().join("cdctl").join("config.toml");
    assert!(
        !config_file.exists(),
        "logout must not create a config file"
    );
}

/// Codex round-2: `auth logout` never emits JSON, so an explicit
/// `--fields`/`--json` must be rejected upfront (`super::reject_explicit_json`)
/// before the token is touched — not after deleting it and exiting `0`.
#[test]
fn auth_logout_rejects_explicit_fields_and_preserves_the_token() {
    let dir = tempdir();
    cdctl(dir.path())
        .args(["auth", "login", "--token-stdin"])
        .write_stdin("api.stored-token\n")
        .assert()
        .success();

    let assert = cdctl(dir.path())
        .args(["auth", "logout", "--fields", "bogus"])
        .assert()
        .code(2);
    assert.stdout(predicates::str::is_empty());

    let assert = cdctl(dir.path())
        .args(["config", "list", "--json"])
        .assert()
        .success();
    let doc: serde_json::Value = serde_json::from_slice(&assert.get_output().stdout).expect("data");
    assert_eq!(
        doc["token"]["set"], true,
        "the rejected logout must not have deleted the stored token"
    );
}

#[test]
fn a_corrupt_config_file_fails_with_its_path() {
    let dir = tempdir();
    let config_dir = dir.path().join("cdctl");
    std::fs::create_dir_all(&config_dir).expect("mkdir");
    std::fs::write(config_dir.join("config.toml"), "not = [valid").expect("write");

    let assert = cdctl(dir.path())
        .args(["config", "get", "current_context"])
        .assert()
        .code(1)
        .stdout(predicates::str::is_empty());
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(
        stderr.contains("config.toml"),
        "the error names the file: {stderr}"
    );
}

#[test]
fn switching_to_an_unknown_context_notes_the_missing_token() {
    let dir = tempdir();
    let assert = cdctl(dir.path())
        .args(["config", "set", "current_context", "acme"])
        .assert()
        .success();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
    assert!(stderr.contains("no stored token"), "got: {stderr}");
}

// --- output contracts ---

/// A `--fields` typo is a usage error, not a silent no-op: a warning-and-exit-0
/// would let an agent read an empty `{}` as "the field is genuinely absent"
/// rather than "you misspelled it". `AuthStatus::FIELDS` validates upfront,
/// so `mount_no_requests`'s `.expect(0)` proves the check runs before the
/// `/users` request, not on the post-request `emit` call.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_misspelled_field_is_a_usage_error() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["auth", "status", "--fields", "emial"])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value =
            serde_json::from_str(&stderr).expect("stderr is one JSON document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"emial\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

/// The behavioral fix: on a mutating handler, a `--fields` typo must fail
/// *before* the write, not on the post-write read-back's `emit()` call —
/// catching it there would perform the mutation, then exit 2 with nothing
/// printed, misreadable as "nothing happened, retry" (a retried `create`
/// duplicate-POSTs). `mount_no_requests`'s `.expect(0)` is the actual proof:
/// no request of any kind — the POST included — ever leaves.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_fields_typo_is_exit_2_before_any_write() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "x.example.com",
                "--action",
                "block",
                "--fields",
                "bogus",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr)
            .expect("--fields implies JSON mode: stderr is one document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"bogus\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

/// Mirrors `rule_create_fields_typo_is_exit_2_before_any_write` for the
/// other noun that shares the upfront `--fields` check.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_create_fields_typo_is_exit_2_before_any_write() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "create",
                "Games",
                "--fields",
                "bogus",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr)
            .expect("--fields implies JSON mode: stderr is one document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"bogus\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

/// Item 2: unified dry-run `--fields` semantics — the upfront check
/// (`Rule::FIELDS`) now runs unconditionally, live or dry-run, so a typo
/// exits 2 before the plan is even built, same as the live path.
/// `mount_no_requests`'s `.expect(0)` proves it: no request, not even a
/// validation GET, ever leaves. Check order (Codex round-2): the fields-name
/// check runs before the `--fields`/`--dry-run` conflict check
/// (`commands::reject_fields_with_dry_run`), so a typo still wins here even
/// though this invocation also carries `--dry-run` — pinned by the message
/// asserting "no such field", not the conflict wording.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_fields_typo_is_exit_2_before_any_request_on_a_dry_run() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "x.example.com",
                "--action",
                "block",
                "--fields",
                "bogus",
                "--dry-run",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr)
            .expect("--fields implies JSON mode: stderr is one document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"bogus\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_token_never_appears_in_debug_output() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture("users.json"), "application/json"),
        )
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["auth", "status", "--debug", "--json"])
            .assert()
            .success();
        let output = assert.get_output();
        let combined = format!(
            "{}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            !combined.contains("api.test-token"),
            "the token must never surface in any output"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn retries_are_logged_to_stderr() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(ResponseTemplate::new(500))
        .up_to_n_times(1)
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture("users.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        // No --no-retry: the second attempt succeeds and the retry is visible.
        let assert = cdctl_against(&uri, dir.path())
            .args(["auth", "status", "--json"])
            .timeout(Duration::from_secs(20))
            .assert()
            .success();
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(
            stderr.contains("retrying in"),
            "a silent backoff is indistinguishable from a hang: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

// --- cdctl api: the D9 escape hatch (plan.md Phase 2 gate) ---

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_get_prints_the_body_verbatim_and_query_rides_the_path() {
    // Distinctive spacing and no trailing newline: any reshaping would show.
    let raw_body = "{\"body\": {\"actively\":  \"unstable\"},   \"success\": true}";
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/access"))
        .and(wiremock::matchers::query_param("device_id", "abc"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(raw_body, "application/json"))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["api", "/access?device_id=abc"])
            .assert()
            .success();
        assert_eq!(
            assert.get_output().stdout,
            raw_body.as_bytes(),
            "the body must pass through byte-for-byte"
        );
    })
    .await
    .expect("command runs");
}

#[test]
fn api_rejects_urls_that_leave_the_origin() {
    let dir = tempdir();
    for url in [
        "https://evil.example/x",
        "//evil.example/x",
        "/\\evil.example/x",
        "https://user:pw@evil.example/x",
    ] {
        // No token in the environment: rejection happens before any auth work.
        cdctl(dir.path())
            .args(["api", url])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_non_get_requires_yes_and_exits_7() {
    let server = MockServer::start().await;
    // The gate fires before any request leaves the process.
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .env("CONTROLD_OUTPUT", "json")
            .args(["api", "/profiles", "-X", "POST", "-F", "name=x"])
            .assert()
            .code(7)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "confirmation.required");
        assert_eq!(doc["error"]["retryable"], false);
    })
    .await
    .expect("command runs");
}

#[test]
fn api_body_forms_require_a_non_get_method_and_are_exclusive() {
    let dir = tempdir();
    // -F without -X.
    cdctl(dir.path())
        .args(["api", "/x", "-F", "a=b"])
        .assert()
        .code(2)
        .stdout(predicates::str::is_empty());
    // --input - without -X; stdin must not be drained (no hang without input).
    cdctl(dir.path())
        .args(["api", "/x", "--input", "-"])
        .write_stdin("{}")
        .assert()
        .code(2);
    // An explicit -X GET is still a GET.
    cdctl(dir.path())
        .args(["api", "/x", "-X", "GET", "-F", "a=b"])
        .assert()
        .code(2);
    // -F and --input are mutually exclusive (clap).
    cdctl(dir.path())
        .args([
            "api", "/x", "-X", "PUT", "--yes", "-F", "a=b", "--input", "-",
        ])
        .assert()
        .code(2);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_form_keys_reach_the_wire_with_literal_brackets() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/profiles/p1/rules"))
        .and(wiremock::matchers::header(
            "content-type",
            "application/x-www-form-urlencoded",
        ))
        .and(wiremock::matchers::body_string_contains("do=1"))
        .and(wiremock::matchers::body_string_contains(
            "hostnames[]=a.example",
        ))
        .and(wiremock::matchers::body_string_contains(
            "hostnames[]=b.example",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"body": [], "success": true})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "api",
                "/profiles/p1/rules",
                "-X",
                "POST",
                "--yes",
                "-F",
                "do=1",
                "-F",
                "hostnames[]=a.example",
                "-F",
                "hostnames[]=b.example",
            ])
            .assert()
            .success();
    })
    .await
    .expect("matched mock proves the bracketed keys went as typed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_stdin_json_reaches_the_wire_verbatim() {
    // Odd spacing survives: no validation, no reshaping (D9).
    let raw_json = "{\"filters\": [ {\"filter\":\"ads\",\"status\":1} ]}";
    let server = MockServer::start().await;
    Mock::given(method("PUT"))
        .and(path("/profiles/p1/filters"))
        .and(wiremock::matchers::header(
            "content-type",
            "application/json",
        ))
        .and(wiremock::matchers::body_string(raw_json))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"body": [], "success": true})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "api",
                "/profiles/p1/filters",
                "-X",
                "PUT",
                "--yes",
                "--input",
                "-",
            ])
            .write_stdin(raw_json)
            .assert()
            .success();
    })
    .await
    .expect("matched mock proves stdin went verbatim");
}

/// A failed upstream pipeline stage (`jq bad | cdctl api --input -`) hands
/// us an empty stdin — that must never become a 0-byte mutation that could
/// exit 0 against live DNS config. The `.expect(0)` mock proves nothing is
/// sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_rejects_an_empty_stdin_body() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "api",
                "/profiles/p1/filters",
                "-X",
                "PUT",
                "--yes",
                "--input",
                "-",
            ])
            .write_stdin("")
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains("empty body"), "names the cause: {stderr}");
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_delete_carries_its_body() {
    let server = MockServer::start().await;
    Mock::given(method("DELETE"))
        .and(path("/access"))
        .and(wiremock::matchers::body_string_contains("ips[]=1.2.3.4"))
        .and(wiremock::matchers::body_string_contains("device_id=abc"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"body": [], "success": true})),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "api",
                "/access",
                "-X",
                "DELETE",
                "--yes",
                "-F",
                "ips[]=1.2.3.4",
                "-F",
                "device_id=abc",
            ])
            .assert()
            .success();
    })
    .await
    .expect("matched mock proves DELETE carried its form body");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_writes_are_never_retried() {
    let server = MockServer::start().await;
    // .expect(1): a second request panics — no --no-retry is passed here.
    Mock::given(method("POST"))
        .and(path("/profiles"))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args(["api", "/profiles", "-X", "POST", "--yes", "-F", "name=x"])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_errors_classify_to_standard_exit_codes() {
    let server = MockServer::start().await;
    // .expect(1) also proves the terminal 404 is not retried.
    Mock::given(method("GET"))
        .and(path("/nope"))
        .respond_with(error_envelope(404, 40401, "No such thing"))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args(["api", "/nope"])
            .assert()
            .code(3)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

/// The confirmation gate runs before auth: a non-GET without `--yes` exits
/// 7 even with no token configured — a request that will not be sent does
/// not demand credentials first.
#[test]
fn api_confirmation_gate_fires_before_auth() {
    let dir = tempdir();
    // No token anywhere in the environment.
    cdctl(dir.path())
        .args(["api", "/profiles", "-X", "POST", "-F", "name=x"])
        .assert()
        .code(7)
        .stdout(predicates::str::is_empty());
}

/// The full binary path must keep non-UTF8 bytes intact — a regression
/// routing api output through a `String` would corrupt /mobileconfig
/// downloads while every text-based test still passed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_passes_binary_bodies_through_unscathed() {
    let binary: &[u8] = &[0x3c, 0x3f, 0x78, 0x6d, 0x6c, 0x00, 0xff, 0xfe, 0x0a, 0x80];
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/mobileconfig/abc"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(binary, "application/x-apple-aspen-config"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["api", "/mobileconfig/abc"])
            .assert()
            .success();
        assert_eq!(
            assert.get_output().stdout,
            binary,
            "stdout must carry the exact bytes"
        );
    })
    .await
    .expect("command runs");
}

/// Production envelope parsing accepts unknown fields, so a non-envelope
/// JSON error (a gateway, an undocumented endpoint) parses as a marker-less
/// envelope — it must classify on the HTTP status: terminal exit 3, one
/// request, never the retryable unconfirmed-success stance. Unit tests
/// cannot catch a regression here: the test-only `deny_unknown_fields`
/// pushes this body down the parse-error branch instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_classifies_non_envelope_json_errors_on_the_http_status() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/plain-404"))
        .respond_with(
            ResponseTemplate::new(404)
                .set_body_raw(r#"{"detail": "no such endpoint"}"#, "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        // No --no-retry: a second request would trip the .expect(1) above.
        cdctl_against(&uri, dir.path())
            .args(["api", "/plain-404"])
            .assert()
            .code(3)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_never_sends_the_token_to_an_unpinned_origin() {
    let server = MockServer::start().await;
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl(dir.path())
            .env("CONTROLD_API_URL", &uri)
            .env("CONTROLD_API_TOKEN", "api.test-token")
            // Deliberately no CONTROLD_UNSAFE_BASE_URL.
            .args(["api", "/users"])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(
            stderr.contains("pinned origin"),
            "the refusal names the policy: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

/// D7: with no token configured the request still goes out, carrying no
/// `Authorization` header — the spec marks `/network` (and `/ip`, service
/// categories/catalog) `security: []`, and `cdctl api` is the only route to
/// them until their typed commands ship.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_tokenless_get_sends_no_authorization_header() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/network"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture("network.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        // No CONTROLD_API_TOKEN, no config file, no unsafe switch: a
        // tokenless client has nothing to leak off the pinned origin.
        cdctl(dir.path())
            .env("CONTROLD_API_URL", &uri)
            .args(["api", "/network"])
            .assert()
            .success();
    })
    .await
    .expect("command runs");

    let requests = server
        .received_requests()
        .await
        .expect("request recording is on");
    assert!(
        requests
            .iter()
            .all(|r| !r.headers.contains_key("authorization")),
        "a tokenless invocation must not invent an Authorization header"
    );
}

/// A tokenless request the server rejects classifies like any other auth
/// failure: the 400/`40001` trap maps to exit 4 — the server's verdict,
/// not a client-side preemption.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_tokenless_request_rejected_upstream_exits_4() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/users"))
        .respond_with(error_envelope(400, 40001, "No session token provided"))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl(dir.path())
            .env("CONTROLD_API_URL", &uri)
            .args(["api", "/users"])
            .assert()
            .code(4)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

/// Ctrl-C must stay live while `--input -` waits on stdin: the read runs off
/// the runtime thread and the interrupt path exits without waiting for it.
/// The pipe is held open so the read never completes on its own.
#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sigint_while_reading_stdin_exits_130() {
    use nix::sys::signal::{Signal, kill};
    use nix::unistd::Pid;

    let dir = tempdir();
    let status = tokio::task::spawn_blocking(move || {
        let mut child = std::process::Command::new(assert_cmd::cargo::cargo_bin("cdctl"))
            .env_clear()
            .env("XDG_CONFIG_HOME", dir.path())
            // Default origin; the request is never sent — stdin blocks first.
            .env("CONTROLD_API_TOKEN", "api.test-token")
            .args(["api", "/x", "-X", "PUT", "--yes", "--input", "-"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawns");
        // Keep the write end open: dropping it would EOF the read and turn
        // this into the empty-body rejection instead of a blocked read.
        let stdin = child.stdin.take();

        std::thread::sleep(Duration::from_millis(800));
        kill(
            Pid::from_raw(i32::try_from(child.id()).expect("pid fits")),
            Signal::SIGINT,
        )
        .expect("signal delivered");
        let status = child.wait().expect("wait");
        drop(stdin);
        status
    })
    .await
    .expect("task runs");

    assert_eq!(
        status.code(),
        Some(130),
        "SIGINT contract holds during stdin reads"
    );
}

#[test]
fn api_rejects_explicit_json_flags() {
    let dir = tempdir();
    for args in [
        ["api", "/users", "--json"].as_slice(),
        ["api", "/users", "--fields", "email"].as_slice(),
    ] {
        cdctl(dir.path())
            .args(args)
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    }
}

// --- profile list/get: human/plain/json rendering, name resolution, hazards ---

/// The insta filter shared by every profile/folder snapshot (and the hostile
/// stderr snapshots): the mock server's ephemeral port — precautionary, since
/// stdout never embeds the URI today. The `UPDATED` column's relative-time
/// phrase needs no filter — `CONTROLD_UNIX_NOW` freezes `now()`, so it
/// renders a stable phrase on its own.
fn port_filters() -> Vec<(&'static str, &'static str)> {
    vec![(r"127\.0\.0\.1:\d+", "127.0.0.1:[port]")]
}

/// A mock that proves the command never built a single request — local
/// validation must fail before any GET/POST/PUT/DELETE leaves the process.
async fn mount_no_requests(server: &MockServer) {
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(server)
        .await;
}

/// A `MockServer` with `GET /profiles` mounted, serving `fixture_name` as the
/// raw body; `expect` is the number of `/profiles` calls the test drives.
async fn profiles_server(fixture_name: &str, expect: u64) -> MockServer {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/profiles"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture(fixture_name), "application/json"),
        )
        .expect(expect)
        .mount(&server)
        .await;
    server
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_list_renders_all_three_modes() {
    let server = profiles_server("profiles.json", 3).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let human_assert = cdctl_against(&uri, dir.path())
            .env("CONTROLD_UNIX_NOW", "1783300000")
            .args(["profile", "list"])
            .assert()
            .success();
        let human = String::from_utf8_lossy(&human_assert.get_output().stdout).into_owned();

        let plain_assert = cdctl_against(&uri, dir.path())
            .env("CONTROLD_UNIX_NOW", "1783300000")
            .args(["profile", "list", "--plain"])
            .assert()
            .success();
        let plain = String::from_utf8_lossy(&plain_assert.get_output().stdout).into_owned();
        assert!(
            !plain.contains('│') && !plain.contains('─'),
            "plain mode drops border glyphs: {plain}"
        );

        let json_assert = cdctl_against(&uri, dir.path())
            .env("CONTROLD_UNIX_NOW", "1783300000")
            .args(["profile", "list", "--json"])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&json_assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let profiles = doc.as_array().expect("array of profiles");
        assert_eq!(profiles.len(), 4);
        let first = profiles[0].as_object().expect("object");
        assert_eq!(
            first.keys().collect::<Vec<_>>(),
            vec![
                "id",
                "name",
                "enabled_rules",
                "enabled_filters",
                "enabled_services",
                "folders",
                "options",
                "default_action",
                "enabled",
                "disabled_until",
                "updated",
            ]
        );

        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("profile_list_human", human);
            insta::assert_snapshot!("profile_list_plain", plain);
            insta::assert_snapshot!("profile_list_json", json);
        });
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_list_normalizes_disabled_and_unset_defaults() {
    let server = profiles_server("profiles_dup_names.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["profile", "list", "--json"])
            .assert()
            .success();
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stdout).expect("data");
        let profiles = doc.as_array().expect("array");

        let paused = profiles
            .iter()
            .find(|p| p["name"] == "Paused")
            .expect("Paused profile present");
        assert_eq!(paused["enabled"], false);
        assert_eq!(paused["disabled_until"], "2026-07-15T08:00:00Z");
        assert_eq!(paused["default_action"]["action"], "block");

        let fresh = profiles
            .iter()
            .find(|p| p["name"] == "Fresh")
            .expect("Fresh profile present");
        assert_eq!(
            fresh["default_action"],
            serde_json::json!({"action": "bypass", "via": null, "enabled": true})
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_get_resolves_names_case_insensitively() {
    let server = profiles_server("profiles.json", 2).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let by_name = cdctl_against(&uri, dir.path())
            .args(["profile", "get", "kids", "--json"])
            .assert()
            .success();
        let doc: serde_json::Value =
            serde_json::from_slice(&by_name.get_output().stdout).expect("data");
        assert!(doc.is_object(), "get returns one object, not an array");
        assert_eq!(doc["name"], "Kids");
        assert_eq!(doc["id"], "pk69038f3c");

        let by_id = cdctl_against(&uri, dir.path())
            .args(["profile", "get", "pk69038f3c", "--json"])
            .assert()
            .success();
        let doc: serde_json::Value =
            serde_json::from_slice(&by_id.get_output().stdout).expect("data");
        assert_eq!(doc["id"], "pk69038f3c");
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_get_human_renders_key_values() {
    let server = profiles_server("profiles.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .env("CONTROLD_UNIX_NOW", "1783300000")
            .args(["profile", "get", "Kids"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("profile_get_human", stdout);
        });
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_get_ambiguous_name_exits_2() {
    let server = profiles_server("profiles_dup_names.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["profile", "get", "Home"])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(
            stderr.contains("pk11aa22bb"),
            "names the first match: {stderr}"
        );
        assert!(
            stderr.contains("pk33cc44dd"),
            "names the second match: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_get_unknown_selector_exits_3() {
    let server = profiles_server("profiles.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["profile", "get", "nope", "--json"])
            .assert()
            .code(3)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "profile.not_found");
        assert_eq!(doc["error"]["retryable"], false);
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_selector_control_chars_exit_2_before_any_request() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args(["profile", "get", "bad\u{7}name"])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
    // MockServer verifies .expect(0) on drop: no request was ever built.
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_tables_escape_hostile_names() {
    let server = profiles_server("profiles_hostile_name.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .env("CONTROLD_UNIX_NOW", "1783300000")
            .args(["profile", "list"])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        assert!(
            !stdout.contains('\u{1b}'),
            "no raw escape reaches the terminal: {stdout:?}"
        );
        assert!(
            stdout.contains("^["),
            "the escape is caret-rendered: {stdout}"
        );

        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("profile_list_hostile_human", stdout);
        });
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_list_empty_is_exit_0_with_empty_json_array() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/profiles"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"profiles": []},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["profile", "list", "--json"])
            .assert()
            .success();
        assert_eq!(assert.get_output().stdout, b"[]\n");
    })
    .await
    .expect("command runs");
}

/// `profile list` names a known field on an empty account: `Profile::FIELDS`
/// validates upfront, before any request, so a *valid* field is inert on a
/// zero-profile account — exit 0, `[]` — same as the vacuous-empty rule in
/// `output::emit` would have given it anyway.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_list_fields_on_an_empty_account_is_exit_0_with_empty_json_array() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/profiles"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"profiles": []},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["profile", "list", "--fields", "name"])
            .assert()
            .success();
        assert_eq!(assert.get_output().stdout, b"[]\n");
    })
    .await
    .expect("command runs");
}

/// The unified contract: an unknown `--fields` name is a usage error
/// rejected upfront, before any request — even on an account that would
/// otherwise have printed the vacuous-empty `[]`. Before the fix, `profile
/// list` checked fields only at emit time, so a typo on a zero-profile
/// account slipped through as exit 0 `[]` (the vacuous-empty rule
/// swallowing it, not the check catching it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_list_fields_typo_on_an_empty_account_is_exit_2_before_any_request() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["profile", "list", "--fields", "bogus"])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value =
            serde_json::from_str(&stderr).expect("stderr is one JSON document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"bogus\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_list_error_keeps_stdout_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/profiles"))
        .respond_with(error_envelope(400, 40001, "No session token provided"))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args(["profile", "list"])
            .assert()
            .code(4)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_list_projects_fields() {
    let server = profiles_server("profiles.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["profile", "list", "--fields", "name,id"])
            .assert()
            .success();
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stdout).expect("data");
        let profiles = doc.as_array().expect("array");
        assert_eq!(profiles.len(), 4);
        for profile in profiles {
            let keys: std::collections::HashSet<&str> = profile
                .as_object()
                .expect("object")
                .keys()
                .map(String::as_str)
                .collect();
            let expected: std::collections::HashSet<&str> = ["name", "id"].into_iter().collect();
            assert_eq!(keys, expected);
        }
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(!stderr.contains("warning"), "no typo warning: {stderr}");
    })
    .await
    .expect("command runs");
}

/// `body` flips to `[]` on success too (the documented hazard) — but a
/// non-object body defeats `keyed("profiles")`, so it must classify as an
/// unconfirmable shape, exit 8, never a panic or a false-empty list.
/// `--no-retry`: exit-8 errors are retryable and GETs retry by default,
/// which would trip the `.expect(1)` mock.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn profile_list_shape_error_exits_8() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/profiles"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": [],
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["profile", "list", "--json", "--no-retry"])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "upstream.error");
        assert_eq!(doc["error"]["retryable"], true);
    })
    .await
    .expect("command runs");
}

/// A typo among otherwise-valid fields still fails the whole request: partial
/// projection with a silently dropped field would be worse than an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_fields_typo_among_valid_fields_is_still_a_usage_error() {
    // Upfront validation rejects the whole `--fields` list before any
    // request, so the mock must see zero calls, not one.
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["profile", "list", "--fields", "name,nmae"])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value =
            serde_json::from_str(&stderr).expect("stderr is one JSON document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"nmae\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

// --- SIGPIPE: `cdctl reference | head` must die quietly with 141, never panic ---

#[cfg(unix)]
#[test]
fn a_closed_pipe_kills_quietly_with_sigpipe() {
    use std::os::unix::process::ExitStatusExt;

    // A pipe whose read end is already closed: the first stdout write EPIPEs.
    let (read_end, write_end) = nix::unistd::pipe().expect("pipe");
    drop(read_end);

    let dir = tempdir();
    let status = std::process::Command::new(assert_cmd::cargo::cargo_bin("cdctl"))
        .env_clear()
        .env("XDG_CONFIG_HOME", dir.path())
        .arg("reference")
        .stdout(std::process::Stdio::from(write_end))
        .stderr(std::process::Stdio::null())
        .status()
        .expect("spawns");

    assert_eq!(
        status.signal(),
        Some(libc::SIGPIPE),
        "SIGPIPE default disposition: the shell reports 141, no panic, no exit 101"
    );
}

// --- folder: list/create/update/delete, action-flag validation, dry-run,
// D8 confirmation ---

const AGGRESSIVE_PK: &str = "pk4cada1c5";
const AGGRESSIVE_NAME: &str = "Aggressive";

async fn mount_profiles(server: &MockServer, expect: u64) {
    Mock::given(method("GET"))
        .and(path("/profiles"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture("profiles.json"), "application/json"),
        )
        .expect(expect)
        .mount(server)
        .await;
}

/// `GET /profiles/{AGGRESSIVE_PK}/groups`, parameterized by fixture — mirrors
/// `mount_rules`'s shape.
async fn mount_groups_fixture(server: &MockServer, fixture_name: &str, expect: u64) {
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups")))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture(fixture_name), "application/json"),
        )
        .expect(expect)
        .mount(server)
        .await;
}

async fn mount_groups(server: &MockServer, expect: u64) {
    mount_groups_fixture(server, "p_groups_full.json", expect).await;
}

async fn mount_proxies(server: &MockServer, expect: u64) {
    Mock::given(method("GET"))
        .and(path("/proxies"))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture("proxies.json"), "application/json"),
        )
        .expect(expect)
        .mount(server)
        .await;
}

/// `GET /profiles/{AGGRESSIVE_PK}/rules` — the profile-wide (no folder
/// segment) GET plain `rule list` renders and `rule create`/`update`'s
/// read-back re-fetches. `rule delete` never reads back (per-request
/// outcomes instead, D11), so it never mounts this.
async fn mount_rules(server: &MockServer, fixture_name: &str, expect: u64) {
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture(fixture_name), "application/json"),
        )
        .expect(expect)
        .mount(server)
        .await;
}

/// The verbatim `{"body":{"rules":[]},"success":true}` envelope — shared by
/// every mock (root or per-folder) whose read-back needs *some* rules
/// response but has nothing to say about them.
fn empty_rules_response() -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_json(serde_json::json!({
        "body": {"rules": []},
        "success": true
    }))
}

/// Mounted with `.expect(0)` in tests that must prove no mutation was
/// attempted, whatever it would have been.
async fn mount_no_writes(server: &MockServer) {
    for verb in ["POST", "PUT", "DELETE"] {
        Mock::given(method(verb))
            .respond_with(ResponseTemplate::new(200))
            .expect(0)
            .mount(server)
            .await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_list_renders_all_three_modes_plus_fields() {
    let server = MockServer::start().await;
    mount_profiles(&server, 4).await;
    mount_groups(&server, 4).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let human_assert = cdctl_against(&uri, dir.path())
            .args(["folder", "list", "--profile", AGGRESSIVE_PK])
            .assert()
            .success();
        let human = String::from_utf8_lossy(&human_assert.get_output().stdout).into_owned();

        let plain_assert = cdctl_against(&uri, dir.path())
            .args(["folder", "list", "--profile", AGGRESSIVE_PK, "--plain"])
            .assert()
            .success();
        let plain = String::from_utf8_lossy(&plain_assert.get_output().stdout).into_owned();
        assert!(
            !plain.contains('│') && !plain.contains('─'),
            "plain mode drops border glyphs: {plain}"
        );

        let json_assert = cdctl_against(&uri, dir.path())
            .args(["folder", "list", "--profile", AGGRESSIVE_PK, "--json"])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&json_assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let folders = doc.as_array().expect("array of folders");
        assert_eq!(folders.len(), 5);
        assert_eq!(
            folders[0]
                .as_object()
                .expect("object")
                .keys()
                .collect::<Vec<_>>(),
            vec!["id", "name", "action", "via", "enabled", "rules"]
        );

        let fields_assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "list",
                "--profile",
                AGGRESSIVE_PK,
                "--fields",
                "id,name",
            ])
            .assert()
            .success();
        let fields_json = String::from_utf8_lossy(&fields_assert.get_output().stdout).into_owned();

        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("folder_list_human", human);
            insta::assert_snapshot!("folder_list_plain", plain);
            insta::assert_snapshot!("folder_list_json", json);
            insta::assert_snapshot!("folder_list_fields", fields_json);
        });
    })
    .await
    .expect("command runs");
}

/// Mirrors `rule_list_fields_on_an_empty_profile_is_exit_0_with_empty_json_array`
/// for the other read handler that carries an upfront `Folder::FIELDS`
/// check: an empty top-level array must still exit 0 and print `[]`, never a
/// false "no such field" — `p_groups.json` is the empty-groups fixture.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_list_fields_on_an_empty_profile_is_exit_0_with_empty_json_array() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "list",
                "--fields",
                "name",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
        assert_eq!(assert.get_output().stdout, b"[]\n");
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_create_spoof_sends_the_exact_form_and_prints_the_response() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups")))
        .and(body_string("name=Games&do=2&status=1&via=192.0.2.53"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_folder_create.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "create",
                "Games",
                "--profile",
                AGGRESSIVE_PK,
                "--action",
                "spoof",
                "--via",
                "192.0.2.53",
            ])
            .assert()
            .success();
        let stdout = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        insta::assert_snapshot!("folder_create_spoof_human", stdout);
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_create_action_less_sends_only_name_and_status() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups")))
        .and(body_string("name=Plain&status=1"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_folder_noaction.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args(["folder", "create", "Plain", "--profile", AGGRESSIVE_PK])
            .assert()
            .success()
            .stdout(predicates::str::contains("noaction"));
    })
    .await
    .expect("command runs");
}

/// A malformed create response (a success envelope with zero folders, so
/// `single_folder_from` cannot confirm the shape) exits 8 — retryable — but
/// the write already landed, so retrying `create` would duplicate it; the
/// error must steer the caller to re-fetch instead.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_create_malformed_response_hints_a_refetch_instead_of_a_retry() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"groups": []},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "create",
                "X",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.unverified");
        assert_eq!(
            doc["error"]["retryable"], false,
            "a landed write must never invite a replay"
        );
        assert_eq!(
            doc["error"]["hint"],
            "the API acknowledged the write; re-fetch with `cdctl folder list` instead of retrying"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_update_rename_only_sends_only_the_name() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups/1")))
        .and(body_string("name=New"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_folder_update.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "update",
                "1",
                "--name",
                "New",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
    })
    .await
    .expect("command runs");
}

/// `--action block` alone must clear a stale `via` client-side — the wire
/// body carries `do=0` only, never a stale `via` pair (write-verification.md:
/// the server also clears `via` on this transition, but the CLI must not
/// *resend* it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_update_action_change_sends_only_do() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups/2")))
        .and(body_string("do=0"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_folder_update.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "update",
                "2",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_update_disabled_sends_status_zero_only() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups/2")))
        .and(body_string("status=0"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_folder_update.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "update",
                "2",
                "--disabled",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_create_via6_is_rejected() {
    let server = MockServer::start().await;
    // Action-flag validation runs before profile resolution: a folder never
    // accepts --via6, and that's local, so no request is ever built.
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "create",
                "X",
                "--profile",
                AGGRESSIVE_PK,
                "--via6",
                "2001:db8::1",
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_create_redirect_unknown_proxy_hints_nearest_matches() {
    let server = MockServer::start().await;
    // Action-flag validation (including the redirect --via proxy check)
    // runs before profile resolution, so /profiles is never called here.
    mount_proxies(&server, 1).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "create",
                "X",
                "--profile",
                AGGRESSIVE_PK,
                "--action",
                "redirect",
                "--via",
                "LON",
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains("LHR"), "got: {stderr}");
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_create_via_without_action_is_rejected() {
    let server = MockServer::start().await;
    // Action-flag validation runs before profile resolution: --via with no
    // --action is rejected locally, so no request is ever built.
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "create",
                "X",
                "--profile",
                AGGRESSIVE_PK,
                "--via",
                "192.0.2.1",
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_update_with_no_flags_is_a_usage_error_before_any_request() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args(["folder", "update", "1", "--profile", AGGRESSIVE_PK])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

/// Item 3(a): the upfront `--fields` check is a per-handler call site — the
/// `create` typo tests above don't exercise `update`'s copy of it.
/// `mount_no_requests`'s `.expect(0)` proves it fires before the scope
/// resolution GETs `update` would otherwise need to resolve `selector`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_update_fields_typo_is_exit_2_before_any_request() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "update",
                "1",
                "--enabled",
                "--fields",
                "bogus",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr)
            .expect("--fields implies JSON mode: stderr is one document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"bogus\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_create_dry_run_prints_the_plan_and_validation_gets_still_run() {
    let server = MockServer::start().await;
    mount_profiles(&server, 2).await;
    mount_proxies(&server, 2).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let human_assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "create",
                "Games",
                "--profile",
                AGGRESSIVE_PK,
                "--action",
                "redirect",
                "--via",
                "LHR",
                "--dry-run",
            ])
            .assert()
            .success();
        let human = String::from_utf8_lossy(&human_assert.get_output().stdout).into_owned();

        let json_assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "create",
                "Games",
                "--profile",
                AGGRESSIVE_PK,
                "--action",
                "redirect",
                "--via",
                "LHR",
                "--dry-run",
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&json_assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc["requests"][0]["method"], "POST");
        assert_eq!(doc["requests"][0]["intent"]["name"], "Games");
        assert_eq!(doc["requests"][0]["intent"]["action"], "redirect");
        assert_eq!(doc["requests"][0]["intent"]["via"], "LHR");
        assert_eq!(doc["requests"][0]["intent"]["enabled"], true);

        insta::assert_snapshot!("folder_create_dry_run_human", human);
        insta::assert_snapshot!("folder_create_dry_run_json", json);
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_update_dry_run_prints_a_sparse_changes_patch() {
    let server = MockServer::start().await;
    mount_profiles(&server, 2).await;
    mount_groups(&server, 2).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let human_assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "update",
                "2",
                "--profile",
                AGGRESSIVE_PK,
                "--name",
                "Renamed",
                "--action",
                "block",
                "--dry-run",
            ])
            .assert()
            .success();
        let human = String::from_utf8_lossy(&human_assert.get_output().stdout).into_owned();

        let json_assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "update",
                "2",
                "--profile",
                AGGRESSIVE_PK,
                "--name",
                "Renamed",
                "--action",
                "block",
                "--dry-run",
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&json_assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            doc["requests"][0]["intent"]["changes"],
            serde_json::json!({"name": "Renamed", "action": "block"})
        );

        insta::assert_snapshot!("folder_update_dry_run_human", human);
        insta::assert_snapshot!("folder_update_dry_run_json", json);
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_delete_dry_run_prints_the_resolved_target_and_writes_nothing() {
    let server = MockServer::start().await;
    mount_profiles(&server, 2).await;
    mount_groups(&server, 2).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let human_assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "delete",
                "2",
                "--profile",
                AGGRESSIVE_PK,
                "--dry-run",
            ])
            .assert()
            .success();
        let human = String::from_utf8_lossy(&human_assert.get_output().stdout).into_owned();

        let json_assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "delete",
                "2",
                "--profile",
                AGGRESSIVE_PK,
                "--dry-run",
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&json_assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            doc["requests"][0]["intent"],
            serde_json::json!({"id": 2, "name": "Spoofed", "rules": 5})
        );

        insta::assert_snapshot!("folder_delete_dry_run_human", human);
        insta::assert_snapshot!("folder_delete_dry_run_json", json);
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_delete_without_yes_is_exit_7_non_interactively() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["folder", "delete", "2", "--profile", AGGRESSIVE_PK])
            .assert()
            .code(7)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        // Folder id 2 ("Spoofed") carries 5 contained rules in
        // p_groups_full.json; the cascade count must reach the user before
        // they confirm.
        assert!(stderr.contains("5 contained rules"), "got: {stderr}");
    })
    .await
    .expect("command runs");
}

/// `CONTROLD_PROFILE` (no `--profile` flag) is an **explicit** selector
/// (D8), just like `--profile` — it must never print the implicit-default
/// info line, and `--yes` must be honored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_delete_honors_the_controld_profile_env_as_explicit() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;
    Mock::given(method("DELETE"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": [],
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .env("CONTROLD_PROFILE", AGGRESSIVE_PK)
            .args(["folder", "delete", "2", "--yes"])
            .assert()
            .success();
        assert_eq!(assert.get_output().stdout, b"");
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(
            !stderr.contains("using default profile"),
            "CONTROLD_PROFILE is explicit, not implicit: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_delete_with_yes_and_an_explicit_profile_succeeds() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;
    Mock::given(method("DELETE"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": [],
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["folder", "delete", "2", "--profile", AGGRESSIVE_PK, "--yes"])
            .assert()
            .success();
        assert_eq!(assert.get_output().stdout, b"");
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains("deleted folder"), "got: {stderr}");
    })
    .await
    .expect("command runs");
}

/// `folder delete`'s twin of `rule_delete_fields_typo_is_exit_2_before_any_request_or_delete`:
/// an unknown `--fields` name is a usage error rejected upfront, before any
/// request, before the confirmation prompt, before the delete. Before the
/// fix, `folder delete` never validated `--fields` at all: the typo was
/// silently ignored, the delete ran, and the command exited 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_delete_fields_typo_is_exit_2_before_any_request_or_delete() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "delete",
                "2",
                "--fields",
                "bogus",
                "--profile",
                AGGRESSIVE_PK,
                "--yes",
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value =
            serde_json::from_str(&stderr).expect("stderr is one JSON document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"bogus\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_delete_yes_is_ignored_for_an_implicit_default_profile() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl(dir.path())
            .args(["config", "set", "default_profile", AGGRESSIVE_PK])
            .assert()
            .success();

        let assert = cdctl_against(&uri, dir.path())
            .args(["folder", "delete", "2", "--yes"])
            .assert()
            .code(7)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains("--yes"), "got: {stderr}");
        assert!(stderr.contains("implicit"), "got: {stderr}");
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_list_with_an_implicit_default_profile_prints_an_info_line() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl(dir.path())
            .args(["config", "set", "default_profile", AGGRESSIVE_PK])
            .assert()
            .success();

        let assert = cdctl_against(&uri, dir.path())
            .args(["folder", "list"])
            .assert()
            .success();
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(
            stderr.contains(&format!(
                "using default profile \"{AGGRESSIVE_NAME}\" ({AGGRESSIVE_PK}) from config"
            )),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_list_without_any_profile_is_a_usage_error() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["folder", "list", "--json"])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "usage.no_profile");
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_update_ambiguous_name_exits_2() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "update",
                "ads",
                "--name",
                "X",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(
            stderr.contains('4') && stderr.contains('5'),
            "names the candidate ids: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_delete_unknown_selector_exits_3() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "delete",
                "nope",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(3)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "folder.not_found");
    })
    .await
    .expect("command runs");
}

/// A name selector never reaches the URL — resolution is strictly
/// client-side (write-verification.md: the folder name 404s in the path).
/// "Spoofed" is unique in `p_groups_full.json`, PK 2; the path matcher
/// proves the resolved integer, not the name, was sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn folder_delete_by_name_hits_the_resolved_integer_path() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;
    Mock::given(method("DELETE"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/groups/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": [],
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "folder",
                "delete",
                "Spoofed",
                "--yes",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
    })
    .await
    .expect("command runs");
}

// --- `rule` (PR 5: rule CRUD + the multi-target write engine) ---

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_list_root_renders_all_three_modes_sorted_by_order() {
    let server = MockServer::start().await;
    mount_profiles(&server, 3).await;
    mount_groups(&server, 3).await;
    // `p_groups_full.json` has 5 folders; the profile-wide fetch queries
    // every one of them unconditionally (rule.rs's `fetch_all_rules`) —
    // none has any rules here, so the root fixture's 8 rules stay the whole
    // story.
    for folder_id in 1..=5 {
        Mock::given(method("GET"))
            .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/{folder_id}")))
            .respond_with(empty_rules_response())
            .expect(3)
            .mount(&server)
            .await;
    }
    mount_rules(&server, "rules_nofolder.json", 3).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let human_assert = cdctl_against(&uri, dir.path())
            .args(["rule", "list", "--profile", AGGRESSIVE_PK])
            .assert()
            .success();
        let human = String::from_utf8_lossy(&human_assert.get_output().stdout).into_owned();

        let plain_assert = cdctl_against(&uri, dir.path())
            .args(["rule", "list", "--profile", AGGRESSIVE_PK, "--plain"])
            .assert()
            .success();
        let plain = String::from_utf8_lossy(&plain_assert.get_output().stdout).into_owned();
        assert!(
            !plain.contains('│'),
            "plain mode drops border glyphs: {plain}"
        );

        let json_assert = cdctl_against(&uri, dir.path())
            .args(["rule", "list", "--profile", AGGRESSIVE_PK, "--json"])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&json_assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let rules = doc.as_array().expect("array of rules");
        assert_eq!(rules.len(), 8);
        assert_eq!(
            rules[0]
                .as_object()
                .expect("object")
                .keys()
                .collect::<Vec<_>>(),
            vec![
                "hostname",
                "action",
                "via",
                "via6",
                "enabled",
                "folder",
                "folder_id",
                "order"
            ]
        );
        let orders: Vec<i64> = rules.iter().map(|r| r["order"].as_i64().unwrap()).collect();
        assert_eq!(
            orders,
            vec![1, 2, 3, 5, 6, 7, 8, 9],
            "kept the fixture's order, unscrambled"
        );

        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("rule_list_human", human);
            insta::assert_snapshot!("rule_list_plain", plain);
            insta::assert_snapshot!("rule_list_json", json);
        });
    })
    .await
    .expect("command runs");
}

/// The root listing omits foldered rules entirely (read-verification.md
/// "Listing root rules") — `rule list` with no `--folder` must union it with
/// one `GET /rules/{folder}` per folder to see them at all. Fails before the
/// fix: no `/rules/1` request is ever made, so the foldered rule never
/// appears and the merged sort can't be exercised either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_list_root_aggregates_rules_from_every_folder() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_smoke.json", 1).await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("rules_root_one.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/1")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("rules_folder_smoke.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["rule", "list", "--profile", AGGRESSIVE_PK, "--json"])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let rules = doc.as_array().expect("array of rules");
        assert_eq!(rules.len(), 2, "the root rule plus the foldered one");
        assert_eq!(rules[0]["hostname"], "probe.cdctl-smoke.example.com");
        assert_eq!(rules[0]["folder"], "Smoke");
        assert_eq!(rules[0]["folder_id"], 1);
        assert_eq!(rules[0]["order"], 1, "the foldered rule sorts first");
        assert_eq!(rules[1]["hostname"], "host.example.com");
        assert_eq!(rules[1]["folder"], serde_json::Value::Null);
        assert_eq!(rules[1]["order"], 2);
    })
    .await
    .expect("command runs");
}

/// The folder ids driving `fetch_all_rules`'s per-folder GETs come from a
/// `/groups` fetch moments earlier, so a 404 on `GET /rules/{folder_id}` can
/// only mean the folder was deleted in the race between the two calls — its
/// rules died with it, so `rule list` must still succeed with the folder
/// contributing zero rules, not fail outright (read-verification.md "An
/// empty folder is not an error; a deleted one is").
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_list_tolerates_a_folder_404_as_the_folder_having_vanished() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "host.example.com", "order": 1, "group": 0, "action": {"do": 1, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(error_envelope(404, 40401, "No such group exists."))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args(["rule", "list", "--profile", AGGRESSIVE_PK, "--json"])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let rules = doc.as_array().expect("array of rules");
        assert_eq!(rules.len(), 1, "the deleted folder contributes zero rules");
        assert_eq!(rules[0]["hostname"], "host.example.com");
    })
    .await
    .expect("command runs");
}

/// The negative space the tolerance must not widen: a 500 on a folder's leg
/// is a real error, never a "folder vanished" — pinning that a future
/// "tolerate flaky folders" regression would print silently-partial data
/// instead of failing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_list_a_folder_500_is_a_classified_error_not_a_partial_list() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    mount_rules(&server, "rules_root_one.json", 1).await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(error_envelope(500, 0, "boom"))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "list",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
                "--no-retry",
            ])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "upstream.error");
        assert_eq!(doc["error"]["retryable"], true);
    })
    .await
    .expect("command runs");
}

/// A name selector never reaches the URL — `--folder` resolves to the
/// integer pk client-side, and the resolved rule's `folder`/`folder_id`
/// prove the same fetch served both resolution and display naming
/// (commands.md#rule).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_list_by_folder_name_resolves_to_the_pk_scoped_path() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "spoofed.example.com", "order": 1, "group": 2,
                 "action": {"do": 2, "status": 1, "via": "192.0.2.10"}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "list",
                "--folder",
                "Spoofed",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc[0]["folder"], "Spoofed");
        assert_eq!(doc[0]["folder_id"], 2);
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_list_ambiguous_folder_name_exits_2() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "list",
                "--folder",
                "ads",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_list_unknown_folder_exits_3() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups(&server, 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "list",
                "--folder",
                "nope",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(3)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "folder.not_found");
    })
    .await
    .expect("command runs");
}

/// The behavioral fix (item 1): an empty top-level array has no elements for
/// a requested field to (mis)match against, so the check must be vacuous —
/// `--fields hostname` on a profile with zero rules must still exit 0 and
/// print `[]`, not a false "no such field" (there's nothing here to compare
/// the name against, valid or not). `p_groups.json` is the empty-groups
/// fixture, so `fetch_all_rules` never issues a per-folder GET — the root
/// listing alone is the whole story.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_list_fields_on_an_empty_profile_is_exit_0_with_empty_json_array() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(empty_rules_response())
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "list",
                "--fields",
                "hostname",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
        assert_eq!(assert.get_output().stdout, b"[]\n");
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_sends_the_exact_form_and_prints_the_readback() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=0&status=1&hostnames[]=a.com&hostnames[]=b.com",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.com", "order": 1, "group": 0, "action": {"do": 0, "status": 1}},
                {"PK": "b.com", "order": 2, "group": 0, "action": {"do": 0, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.com",
                "b.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let hostnames: Vec<&str> = doc
            .as_array()
            .expect("always an array, even here")
            .iter()
            .map(|r| r["hostname"].as_str().unwrap())
            .collect();
        assert_eq!(hostnames, vec!["a.com", "b.com"], "argv order");
    })
    .await
    .expect("command runs");
}

/// The `group` scalar precedes the `hostnames[]` pairs (D11: scalars first),
/// and the resolved folder pk — never the name — reaches the wire. The
/// read-back mocks the real shape: the root listing never carries a
/// foldered rule (read-verification.md "Listing root rules"), only
/// `GET /rules/2` does — fails before the fix with a false
/// `write.partial_failure`, since only the root path was ever fetched.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_with_folder_sends_group_before_hostnames() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("do=0&status=1&group=2&hostnames[]=a.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(empty_rules_response())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.com", "order": 1, "group": 2, "action": {"do": 0, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.com",
                "--action",
                "block",
                "--folder",
                "Spoofed",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc[0]["folder_id"], 2);
        assert_eq!(doc[0]["folder"], "Spoofed");
    })
    .await
    .expect("command runs");
}

/// A spoof create never validates `--via` against `GET /proxies` — that
/// check is `--action redirect`-only (commands.md#the-shared-action-flags);
/// no `/proxies` mock is mounted, so an unexpected call would 404 and fail
/// this test.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_spoof_sends_via_and_via6_without_a_proxy_lookup() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=2&status=1&via=192.0.2.10&via_v6=2001%3Adb8%3A%3A1&hostnames[]=a.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0,
                 "action": {"do": 2, "status": 1, "via": "192.0.2.10", "via_v6": "2001:db8::1"}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "spoof",
                "--via",
                "192.0.2.10",
                "--via6",
                "2001:db8::1",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_multi_target_verifies_the_readback_and_prints_every_target() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=0&status=1&hostnames[]=a.example.com&hostnames[]=b.example.com&hostnames[]=c.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    mount_rules(&server, "rules_after_multi_create.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "c.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let hostnames: Vec<&str> = doc
            .as_array()
            .expect("array")
            .iter()
            .map(|r| r["hostname"].as_str().unwrap())
            .collect();
        assert_eq!(
            hostnames,
            vec!["a.example.com", "b.example.com", "c.example.com"]
        );
    })
    .await
    .expect("command runs");
}

/// A truncated read-back (the ~1001-form-var silent drop, simulated) never
/// lands on stdout; the retryable aggregate excludes the two hostnames that
/// did land.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_verification_gap_is_retryable_and_excludes_landed_targets() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=0&status=1&hostnames[]=a.example.com&hostnames[]=b.example.com&hostnames[]=c.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200).set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    mount_rules(&server, "rules_truncated_readback.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "c.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], true);
        let retry_argv: Vec<&str> = doc["error"]["details"]["retry_argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(retry_argv.contains(&"c.example.com"));
        assert!(!retry_argv.contains(&"a.example.com"));
        assert!(!retry_argv.contains(&"b.example.com"));

        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("rule_create_verification_gap_stderr", stderr);
        });
    })
    .await
    .expect("command runs");
}

/// The negative space the fix must preserve: a target absent from the root
/// listing *and* every folder's listing is still `rule.write_dropped` — the
/// profile-wide read-back checking every folder must not manufacture a false
/// positive for a hostname that plainly never landed anywhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_absent_from_every_folder_and_root_is_still_write_dropped() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=0&status=1&hostnames[]=a.example.com&hostnames[]=b.example.com&hostnames[]=c.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0, "action": {"do": 0, "status": 1}},
                {"PK": "b.example.com", "order": 2, "group": 0, "action": {"do": 0, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(empty_rules_response())
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "c.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], true);
        let retry_argv: Vec<&str> = doc["error"]["details"]["retry_argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(retry_argv.contains(&"c.example.com"));
        assert!(!retry_argv.contains(&"a.example.com"));
        assert!(!retry_argv.contains(&"b.example.com"));
        let targets = doc["error"]["details"]["targets"].as_array().unwrap();
        let c = targets
            .iter()
            .find(|t| t["target"] == "c.example.com")
            .expect("c.example.com is a target row");
        assert_eq!(c["outcome"], "failed");
        assert_eq!(c["code"], "rule.write_dropped");
        assert_eq!(c["retryable"], true);
    })
    .await
    .expect("command runs");
}

/// A folder deleted between the `/groups` fetch and its own listing must not
/// misreport a landed write as `rule.write_dropped` — the same 404 tolerance
/// `rule list` gets (module doc, `fetch_all_rules`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_read_back_tolerates_a_folder_404_as_the_folder_having_vanished() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=0&status=1&hostnames[]=a.example.com&hostnames[]=b.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0, "action": {"do": 0, "status": 1}},
                {"PK": "b.example.com", "order": 2, "group": 0, "action": {"do": 0, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(error_envelope(404, 40401, "No such group exists."))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        let hostnames: Vec<&str> = doc
            .as_array()
            .expect("array")
            .iter()
            .map(|r| r["hostname"].as_str().unwrap())
            .collect();
        assert_eq!(hostnames, vec!["a.example.com", "b.example.com"]);
    })
    .await
    .expect("command runs");
}

/// The negative space the tolerance must not widen: a 500 on the folder leg
/// of the read-back's union routes through `landed_write_unverified` exactly
/// like the root leg — never tolerated as an empty contribution.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_read_back_folder_500_is_write_unverified() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(empty_rules_response())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(error_envelope(500, 0, "boom"))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
                "--no-retry",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.unverified");
        assert_eq!(doc["error"]["retryable"], false);
        assert_eq!(doc["error"]["upstream"]["http_status"], 500);
    })
    .await
    .expect("command runs");
}

/// One absent target (safe to retry) plus one present-but-wrong-state
/// target (a create retry would duplicate-POST it) is a mixed batch: exit
/// `1`, `retry_argv` covers only the absent one, and the hint names the
/// mismatch's `rule update` remedy separately.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_mixed_absent_and_mismatched_targets_yields_exit_1() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=0&status=1&hostnames[]=a.example.com&hostnames[]=b.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "b.example.com", "order": 1, "group": 0, "action": {"do": 1, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], false);
        let retry_argv: Vec<&str> = doc["error"]["details"]["retry_argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(retry_argv.contains(&"a.example.com"));
        assert!(
            !retry_argv.contains(&"b.example.com"),
            "the mismatch is named in the hint, not retry_argv"
        );
        let hint = doc["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("b.example.com"), "got: {hint}");
        assert!(hint.contains("rule update"), "got: {hint}");
        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("rule_create_mixed_exit_1_stderr", stderr);
        });
    })
    .await
    .expect("command runs");
}

/// The wrong-folder-landing case is distinguishable only because the union
/// sees every folder (module doc — the fix's own stated rationale): a
/// target that landed with the right action/enabled but in a folder the
/// create never asked for is `rule.state_mismatch`, terminal — never
/// `rule.write_dropped`, which would invite an unsafe duplicate-POST retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_landed_in_the_wrong_folder_is_a_state_mismatch_not_write_dropped() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("do=0&status=1&hostnames[]=a.example.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(empty_rules_response())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 2, "action": {"do": 0, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], false);
        let targets = doc["error"]["details"]["targets"].as_array().unwrap();
        let target = targets
            .iter()
            .find(|t| t["target"] == "a.example.com")
            .expect("a.example.com is a target row");
        assert_eq!(target["outcome"], "failed");
        assert_eq!(target["code"], "rule.state_mismatch");
        assert_eq!(target["retryable"], false);
    })
    .await
    .expect("command runs");
}

/// A retryable read-back failure (a 500 on the verifying `GET`) remaps to
/// `write.unverified`, exit 1, `retryable: false` — the write already landed,
/// so exit 8 must not invite a replay. The remap still preserves the source
/// error's `upstream`, since the API only *acknowledged* the write and that
/// ack cannot reveal a dropped hostname.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_read_back_failure_preserves_upstream_and_remaps_to_write_unverified() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    // `--no-retry`: a 500 read-back is retryable, and the D12 retry loop's
    // "info: retrying" lines would otherwise share stderr with the envelope
    // this test parses as JSON — retrying is exercised elsewhere.
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(error_envelope(500, 0, "boom"))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
                "--no-retry",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.unverified");
        assert_eq!(doc["error"]["retryable"], false);
        assert_eq!(doc["error"]["upstream"]["http_status"], 500);
        let hint = doc["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("acknowledged"), "got: {hint}");
        assert!(hint.contains("cdctl rule list"), "got: {hint}");
    })
    .await
    .expect("command runs");
}

/// A terminal read-back failure (auth expired mid-verification) must pass
/// through unchanged: it already carries the auth login hint and, being
/// terminal, can never invite a replay — remapping it to `write.unverified`
/// would discard both.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_read_back_auth_failure_passes_through_terminal() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(error_envelope(
            400,
            40001,
            "Invalid session, please login again.",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(4)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "auth.invalid_token");
        let hint = doc["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("cdctl auth login"), "got: {hint}");
    })
    .await
    .expect("command runs");
}

/// A read-back rule that fails to normalize (a missing `action.do`, D0's
/// worst-case DNS failure) is `write.unverified` too — the write already
/// landed, so this is not a bare `upstream.error` either.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_read_back_missing_do_is_write_unverified() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0, "action": {"status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.unverified");
        assert_eq!(doc["error"]["retryable"], false);
    })
    .await
    .expect("command runs");
}

/// A retryable `POST` failure (D12's "may still have landed" ambiguity)
/// resolves through the same read-back `create` runs on success: every
/// target converged, so the write did land despite the error — print the
/// read-back rules and say so, rather than surfacing a false failure that
/// would invite an unsafe duplicate-POST retry.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_retryable_write_error_all_landed_succeeds_with_an_info_line() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(error_envelope(500, 0, "boom"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0, "action": {"do": 0, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc[0]["hostname"], "a.example.com");
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(
            stderr.contains(
                "the write reported an error but the read-back confirms every target landed"
            ),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

/// The read-back proves the write demonstrably did *not* apply (the target
/// is absent) — the original classification passes through verbatim, never
/// wrapped as `write.partial_failure`: the retry the exit code invites is
/// now proven safe.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_retryable_write_error_none_landed_passes_through_the_original_error() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(error_envelope(429, 0, "rate limited").insert_header("retry-after", "12"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(empty_rules_response())
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "ratelimit.exceeded");
        assert_ne!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], true);
        assert_eq!(doc["error"]["retry_after"], 12);
    })
    .await
    .expect("command runs");
}

/// One target landed (the write did apply, at least partly) and one is
/// absent: the aggregate's absent row is attributed to the *original* write
/// error — these targets verifiably never landed, and the write error is
/// their cause — and the landed hostname is excluded from `retry_argv`. The
/// aggregate's max-retry_after propagation then carries the write error's
/// `retry_after` automatically.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_retryable_write_error_partial_landed_attributes_absent_targets_to_it() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(error_envelope(429, 0, "rate limited").insert_header("retry-after", "9"))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0, "action": {"do": 0, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], true);
        assert_eq!(
            doc["error"]["retry_after"], 9,
            "the max-retry_after propagation carries the write error's retry_after"
        );
        let targets = doc["error"]["details"]["targets"].as_array().unwrap();
        let b = targets
            .iter()
            .find(|t| t["target"] == "b.example.com")
            .expect("b.example.com is a target row");
        assert_eq!(b["outcome"], "failed");
        assert_eq!(b["code"], "ratelimit.exceeded");
        assert_eq!(b["retryable"], true);
        let retry_argv: Vec<&str> = doc["error"]["details"]["retry_argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(retry_argv.contains(&"b.example.com"));
        assert!(
            !retry_argv.contains(&"a.example.com"),
            "the landed hostname must never be retried"
        );
    })
    .await
    .expect("command runs");
}

/// When the read-back itself fails, the ambiguity is unresolvable: the
/// failure passes through the existing `landed_write_unverified` path, never
/// the original retryable write error (which would invite an unsafe
/// duplicate-POST replay).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_retryable_write_error_and_read_back_failure_is_write_unverified() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(error_envelope(500, 0, "write boom"))
        .expect(1)
        .mount(&server)
        .await;
    // `--no-retry`: the read-back GET is also retryable, and the D12 retry
    // loop's "info: retrying" lines would otherwise share stderr with the
    // envelope this test parses as JSON — retrying is exercised elsewhere.
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(error_envelope(500, 0, "read boom"))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
                "--no-retry",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.unverified");
        assert_eq!(doc["error"]["retryable"], false);
        let hint = doc["error"]["hint"].as_str().expect("hint present");
        assert!(
            hint.contains("may have landed"),
            "the write errored, so the hint must state uncertainty, got: {hint}"
        );
        assert!(
            !hint.contains("acknowledged"),
            "no acknowledgment happened on this path, got: {hint}"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_upstream_429_is_exit_8() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/profiles/{AGGRESSIVE_PK}/rules/a.example.com"
        )))
        .respond_with(error_envelope(429, 0, "rate limited").insert_header("retry-after", "17"))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "--yes",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr).expect("one envelope");
        assert_eq!(
            doc["error"]["retry_after"], 17,
            "the target's Retry-After must reach the top-level aggregate (D4)"
        );
        // D11: delete has no read-back, so a retryable failure is genuinely
        // ambiguous — the delete may have landed, and blindly re-running
        // retry_argv would 404 on a rule that is actually already gone.
        let hint = doc["error"]["hint"].as_str().unwrap();
        assert!(
            hint.contains("may still have landed") && hint.contains("cdctl rule list"),
            "got: {hint}"
        );
        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("rule_delete_429_stderr", stderr);
        });
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_upstream_404_is_exit_3() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/profiles/{AGGRESSIVE_PK}/rules/a.example.com"
        )))
        .respond_with(error_envelope(404, 40401, "No such rule exists."))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "--yes",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(3)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(
            doc["error"]["retryable"], false,
            "a terminal aggregate (404) must never invite a replay"
        );
        // Terminal-only failures keep the default hint — a 404 rule is
        // already gone, so there is no landed-ambiguity to inspect for.
        let hint = doc["error"]["hint"].as_str().unwrap();
        assert!(!hint.contains("may still have landed"), "got: {hint}");
        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("rule_delete_404_stderr", stderr);
        });
    })
    .await
    .expect("command runs");
}

/// `--disabled` alone sends `status=0` plus the hostnames only — no `do` —
/// and the merge preserves everything else (write-verification.md); the
/// read-back asserts only `enabled`, never the untouched action/via.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_disabled_alone_sends_status_and_hostnames_only() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("status=0&hostnames[]=x.example.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    mount_rules(&server, "rules_converged.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "x.example.com",
                "--disabled",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc[0]["enabled"], false);
        assert_eq!(
            doc[0]["action"], "spoof",
            "preserved by the merge, not sent"
        );
        assert_eq!(
            doc[0]["via"], "192.0.2.10",
            "preserved by the merge, not sent"
        );
    })
    .await
    .expect("command runs");
}

/// No `--folder` flag on `rule update` must not blind the read-back to a
/// target that already lives inside a folder: the root listing alone never
/// carries it (read-verification.md "Listing root rules"), only
/// `GET /rules/2` does — a root-only read-back reads the foldered target as
/// absent, so a converged update falsely reports `rule.write_dropped`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_finds_a_foldered_target_with_no_folder_flag() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("status=0&hostnames[]=x.example.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(empty_rules_response())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "x.example.com", "order": 1, "group": 2, "action": {"do": 1, "status": 0}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "x.example.com",
                "--disabled",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc[0]["hostname"], "x.example.com");
        assert_eq!(doc[0]["folder"], "Spoofed");
        assert_eq!(doc[0]["folder_id"], 2);
        assert_eq!(doc[0]["enabled"], false);
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_root_sends_group_zero_and_clears_folder_id() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    // `--root` fetches folders unconditionally, like every other rule
    // update (module doc, src/commands/rule.rs) — no folder is resolved
    // from it here, so the empty fixture is enough.
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("group=0&hostnames[]=y.example.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    mount_rules(&server, "rules_converged.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "y.example.com",
                "--root",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc[0]["folder_id"], serde_json::Value::Null);
    })
    .await
    .expect("command runs");
}

/// `--root`'s target that never actually left folder 2 is distinguishable
/// from an absent one only because the union sees every folder: it is
/// `rule.state_mismatch`, retryable (the merge is idempotent — a rerun
/// converges it) — never `rule.write_dropped`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_root_target_still_in_a_folder_is_a_state_mismatch_not_write_dropped() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("group=0&hostnames[]=z.example.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(empty_rules_response())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "z.example.com", "order": 1, "group": 2, "action": {"do": 0, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "z.example.com",
                "--root",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], true);
        let targets = doc["error"]["details"]["targets"].as_array().unwrap();
        let target = targets
            .iter()
            .find(|t| t["target"] == "z.example.com")
            .expect("z.example.com is a target row");
        assert_eq!(target["outcome"], "failed");
        assert_eq!(target["code"], "rule.state_mismatch");
        assert_eq!(target["retryable"], true);
    })
    .await
    .expect("command runs");
}

/// A retryable read-back gap on `rule update` (the merge is idempotent, so a
/// re-run converges it): exit `8`, `retry_argv` is a `rule update` covering
/// only the still-mismatched target with the original flags.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_verification_gap_is_retryable_with_a_convergent_retry_argv() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "status=0&hostnames[]=x.example.com&hostnames[]=y.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "x.example.com", "order": 1, "group": 0, "action": {"do": 1, "status": 0}},
                {"PK": "y.example.com", "order": 2, "group": 0, "action": {"do": 1, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "x.example.com",
                "y.example.com",
                "--disabled",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], true);
        let retry_argv: Vec<&str> = doc["error"]["details"]["retry_argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(
            retry_argv,
            vec![
                "cdctl",
                "rule",
                "update",
                "--profile",
                AGGRESSIVE_PK,
                "--disabled",
                "y.example.com",
            ]
        );
    })
    .await
    .expect("command runs");
}

/// A retryable `PUT` failure resolves the same way `create`'s does: the
/// read-back proves the sent fields converged on every target, so it is
/// success (with the info line), not a false `write.partial_failure`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_retryable_write_error_all_converged_succeeds_with_an_info_line() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("status=0&hostnames[]=x.example.com"))
        .respond_with(error_envelope(500, 0, "boom"))
        .expect(1)
        .mount(&server)
        .await;
    mount_rules(&server, "rules_converged.json", 1).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "x.example.com",
                "--disabled",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc[0]["hostname"], "x.example.com");
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(
            stderr.contains(
                "the write reported an error but the read-back confirms every target landed"
            ),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

/// When every verification gap is a mismatch (nothing absent), a create
/// retry must switch verb to `rule update` — re-running `create` would
/// duplicate-POST hostnames that already landed. Create mismatches are
/// terminal (`Generic`), so the aggregate exit is `1`, never `8`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_all_mismatched_targets_retries_with_rule_update() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=0&status=1&hostnames[]=a.example.com&hostnames[]=b.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0, "action": {"do": 1, "status": 1}},
                {"PK": "b.example.com", "order": 2, "group": 0, "action": {"do": 1, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], false);
        let retry_argv: Vec<&str> = doc["error"]["details"]["retry_argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(&retry_argv[1..3], ["rule", "update"], "got: {retry_argv:?}");
        assert!(retry_argv.contains(&"a.example.com"));
        assert!(retry_argv.contains(&"b.example.com"));
    })
    .await
    .expect("command runs");
}

/// A live `via_v6` the desired spec omits can never be cleared by a `rule
/// update` retry — `PUT /rules` preserves an omitted `via_v6`
/// (write-verification.md "`via_v6` cannot be cleared"). Advertising the
/// update anyway would be an unconvergeable remedy loop: the retry
/// converges everything else and reports the same mismatch forever. The
/// only real remedy, delete + recreate, must live in the hint instead, and
/// `retry_argv` must not offer the update at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_via6_mismatch_is_unconvergeable_and_named_in_the_hint() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=2&status=1&via=192.0.2.10&hostnames[]=a.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0,
                 "action": {"do": 2, "status": 1, "via": "192.0.2.10", "via_v6": "2001:db8::1"}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "spoof",
                "--via",
                "192.0.2.10",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        assert_eq!(doc["error"]["retryable"], false);
        let retry_argv: Vec<&str> = doc["error"]["details"]["retry_argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(
            !retry_argv.contains(&"update"),
            "no `rule update` retry can converge an unclearable via6: {retry_argv:?}"
        );
        assert!(
            !retry_argv.contains(&"a.example.com"),
            "got: {retry_argv:?}"
        );
        let hint = doc["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("a.example.com"), "got: {hint}");
        assert!(hint.contains("delete"), "got: {hint}");
        assert!(hint.contains("recreate"), "got: {hint}");
        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("rule_create_via6_mismatch_unconvergeable_stderr", stderr);
        });
    })
    .await
    .expect("command runs");
}

/// Mixed mismatches: one plain state mismatch (convergeable by `rule
/// update`) and one via6 mismatch (unconvergeable). `retry_argv` must cover
/// only the former; the hint must name the latter with the delete +
/// recreate remedy, not the update.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_mixed_convergeable_and_via6_mismatch_splits_retry_argv_and_hint() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string(
            "do=2&status=1&via=192.0.2.10&hostnames[]=a.example.com&hostnames[]=b.example.com",
        ))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0,
                 "action": {"do": 2, "status": 0, "via": "192.0.2.10"}},
                {"PK": "b.example.com", "order": 2, "group": 0,
                 "action": {"do": 2, "status": 1, "via": "192.0.2.10", "via_v6": "2001:db8::1"}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "--action",
                "spoof",
                "--via",
                "192.0.2.10",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(1)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr).expect("one envelope");
        assert_eq!(doc["error"]["code"], "write.partial_failure");
        let retry_argv: Vec<&str> = doc["error"]["details"]["retry_argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert_eq!(&retry_argv[1..3], ["rule", "update"], "got: {retry_argv:?}");
        assert!(retry_argv.contains(&"a.example.com"), "got: {retry_argv:?}");
        assert!(
            !retry_argv.contains(&"b.example.com"),
            "the via6 mismatch is unconvergeable, so it must never ride the update retry: \
             {retry_argv:?}"
        );
        let hint = doc["error"]["hint"].as_str().unwrap();
        assert!(hint.contains("b.example.com"), "got: {hint}");
        assert!(hint.contains("delete"), "got: {hint}");
        assert!(!hint.contains("a.example.com"), "got: {hint}");
    })
    .await
    .expect("command runs");
}

/// `--folder <name>` on `rule update` resolves to the pk and sends
/// `group=<pk>` — only `--root`'s `group=0` was wire-tested before. The
/// read-back mocks the real shape: a rule that landed in folder 2 is
/// invisible on the root listing (read-verification.md "Listing root
/// rules") — only `GET /rules/2` sees it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_folder_sends_the_resolved_group_pk() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups_one.json", 1).await;
    Mock::given(method("PUT"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("group=2&hostnames[]=x.example.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(empty_rules_response())
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules/2")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "x.example.com", "order": 1, "group": 2, "action": {"do": 1, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "x.example.com",
                "--folder",
                "Spoofed",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_with_no_flags_is_a_usage_error_before_any_request() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "x.example.com",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

/// Item 3(a): mirrors `folder_update_fields_typo_is_exit_2_before_any_request`
/// — the upfront `--fields` check is a per-handler call site, so `update`
/// needs its own proof the `create` typo tests don't cover.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_fields_typo_is_exit_2_before_any_request() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "x.example.com",
                "--enabled",
                "--fields",
                "bogus",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr)
            .expect("--fields implies JSON mode: stderr is one document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"bogus\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

/// The attached-empty spelling is refused locally — never forwarded, where
/// it would 400 (`err_via6_clear.json` documents that response).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_via6_clear_is_rejected_locally() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "spoof",
                "--via",
                "192.0.2.10",
                "--via6=",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        insta::with_settings!({filters => port_filters()}, {
            insta::assert_snapshot!("rule_create_via6_clear_rejected_stderr", stderr);
        });
    })
    .await
    .expect("command runs");
}

/// `--via6` must be an IPv6 literal, not a hostname or an IPv4 literal
/// (commands.md#the-shared-action-flags: "IPv6, spoof only") — and
/// `--dry-run` must not bless a plan built from an invalid one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_via6_non_ipv6_literal_is_rejected_locally() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "spoof",
                "--via",
                "192.0.2.10",
                "--via6",
                "not-v6",
                "--dry-run",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_501_hostnames_is_exit_2_before_any_request() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    let hostnames: Vec<String> = (0..501).map(|i| format!("h{i}.example.com")).collect();
    tokio::task::spawn_blocking(move || {
        let mut args = vec!["rule".to_owned(), "create".to_owned()];
        args.extend(hostnames);
        args.extend([
            "--action".to_owned(),
            "block".to_owned(),
            "--profile".to_owned(),
            AGGRESSIVE_PK.to_owned(),
        ]);
        cdctl_against(&uri, dir.path())
            .args(&args)
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_hostile_hostname_is_exit_2_before_any_request() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com\u{7}",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

/// A non-ASCII hostname is rejected locally (`.expect(0)`: no request
/// leaves) until non-ASCII behavior is probed live — the read-back
/// reconciliation keys on the server's `PK` verbatim vs the CLI's
/// ASCII-lowercased input, and a server-side transform would classify the
/// target absent, inviting an exit-8 retry loop that duplicate-POSTs.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_non_ascii_hostname_is_exit_2_before_any_request() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "café.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains("xn--"), "got: {stderr}");
    })
    .await
    .expect("command runs");
}

/// Duplicate hostnames in argv (case- and dot-variant included) collapse to
/// one `hostnames[]` pair — a duplicate inside one `POST` would fail the
/// whole chunk upstream (write-verification.md).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_dedups_duplicate_argv_hostnames() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_groups_fixture(&server, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("do=0&status=1&hostnames[]=a.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [{"PK": "a.com", "order": 1, "group": 0, "action": {"do": 0, "status": 1}}]},
            "success": true
        })))
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.com",
                "A.com",
                "a.com.",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
    })
    .await
    .expect("command runs");
}

/// Happy path: one `DELETE` per hostname, sequential, wildcard percent-encoded.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_happy_path_percent_encodes_wildcards() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/profiles/{AGGRESSIVE_PK}/rules/a.example.com"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_delete_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/profiles/{AGGRESSIVE_PK}/rules/%2A.wild.example.com"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_delete_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "*.wild.example.com",
                "--yes",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
        assert_eq!(assert.get_output().stdout, b"");
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains("deleted 2 rules"), "got: {stderr}");
    })
    .await
    .expect("command runs");
}

/// Sequential, abort-on-first-failure: the third target is never attempted
/// (`.expect(0)`), and it shows up `skipped`, not `failed`, in the aggregate.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_aborts_after_the_first_failure_and_skips_the_rest() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/profiles/{AGGRESSIVE_PK}/rules/a.example.com"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_delete_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/profiles/{AGGRESSIVE_PK}/rules/b.example.com"
        )))
        .respond_with(ResponseTemplate::new(500))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/profiles/{AGGRESSIVE_PK}/rules/c.example.com"
        )))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "b.example.com",
                "c.example.com",
                "--yes",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(8)
            .stdout(predicates::str::is_empty());
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stderr).expect("one envelope");
        let targets = doc["error"]["details"]["targets"].as_array().unwrap();
        assert_eq!(targets[0]["outcome"], "landed");
        assert_eq!(targets[1]["outcome"], "failed");
        assert_eq!(targets[2]["outcome"], "skipped");
        let retry_argv: Vec<&str> = doc["error"]["details"]["retry_argv"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        assert!(retry_argv.contains(&"--yes"), "deletes carry --yes");
        assert!(retry_argv.contains(&"b.example.com"));
        assert!(retry_argv.contains(&"c.example.com"));
        assert!(
            !retry_argv.contains(&"a.example.com"),
            "landed, never resent"
        );
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_without_yes_is_exit_7_non_interactively() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(7)
            .stdout(predicates::str::is_empty());
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_yes_is_ignored_for_an_implicit_default_profile() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        cdctl(dir.path())
            .args(["config", "set", "default_profile", AGGRESSIVE_PK])
            .assert()
            .success();

        let assert = cdctl_against(&uri, dir.path())
            .args(["rule", "delete", "a.example.com", "--yes"])
            .assert()
            .code(7)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains("implicit"), "got: {stderr}");
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_with_yes_and_an_explicit_profile_succeeds() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/profiles/{AGGRESSIVE_PK}/rules/a.example.com"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_delete_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "--yes",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
        assert_eq!(assert.get_output().stdout, b"");
    })
    .await
    .expect("command runs");
}

/// The unified `--fields` contract on `rule delete`: an unknown field is a
/// usage error rejected upfront — before any request, before the
/// confirmation prompt, before the delete. Before the fix, `rule delete`
/// never validated `--fields` at all: the typo was silently ignored, the
/// delete ran, and the command exited 0.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_fields_typo_is_exit_2_before_any_request_or_delete() {
    let server = MockServer::start().await;
    mount_no_requests(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "--fields",
                "bogus",
                "--yes",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value =
            serde_json::from_str(&stderr).expect("stderr is one JSON document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("no such field \"bogus\""),
            "got: {stderr}"
        );
    })
    .await
    .expect("command runs");
}

/// A *valid* `--fields` name is inert on `rule delete`: the command emits no
/// row, but the field namespace is still `Rule::FIELDS` (the command's data
/// schema), so a real field must not be rejected — the delete proceeds and
/// exits 0, same as without `--fields` at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_with_a_valid_fields_name_still_deletes() {
    let server = MockServer::start().await;
    mount_profiles(&server, 1).await;
    Mock::given(method("DELETE"))
        .and(path(format!(
            "/profiles/{AGGRESSIVE_PK}/rules/a.example.com"
        )))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_delete_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "--fields",
                "hostname",
                "--yes",
                "--profile",
                AGGRESSIVE_PK,
            ])
            .assert()
            .success();
        assert_eq!(assert.get_output().stdout, b"");
    })
    .await
    .expect("command runs");
}

/// Item 2's behavioral fix, and item 3(c)'s live end-to-end coverage in one
/// test: `--fields hostname` names a field of `Rule` (the command's data
/// schema), so a live create succeeds and projects normally. Superseded
/// (Codex round-2) on the dry-run half: `--fields` and `--dry-run` now
/// conflict outright — a dry run prints the request plan, not data rows, so
/// projecting row fields over it is meaningless — checked upfront via
/// `commands::reject_fields_with_dry_run`, before any request, so the mock
/// server sees nothing at all (`mount_no_requests`).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_fields_hostname_succeeds_live_and_conflicts_with_dry_run() {
    let live = MockServer::start().await;
    mount_profiles(&live, 1).await;
    mount_groups_fixture(&live, "p_groups.json", 1).await;
    Mock::given(method("POST"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .and(body_string("do=0&status=1&hostnames[]=a.example.com"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_raw(fixture("write_rule_ack.json"), "application/json"),
        )
        .expect(1)
        .mount(&live)
        .await;
    Mock::given(method("GET"))
        .and(path(format!("/profiles/{AGGRESSIVE_PK}/rules")))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "body": {"rules": [
                {"PK": "a.example.com", "order": 1, "group": 0, "action": {"do": 0, "status": 1}}
            ]},
            "success": true
        })))
        .expect(1)
        .mount(&live)
        .await;

    let live_uri = live.uri();
    let live_dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&live_uri, live_dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--fields",
                "hostname",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .success();
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stdout).expect("projected JSON");
        assert_eq!(doc, serde_json::json!([{"hostname": "a.example.com"}]));
    })
    .await
    .expect("live command runs");

    let dry = MockServer::start().await;
    mount_no_requests(&dry).await;

    let dry_uri = dry.uri();
    let dry_dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let assert = cdctl_against(&dry_uri, dry_dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "--action",
                "block",
                "--fields",
                "hostname",
                "--dry-run",
                "--profile",
                AGGRESSIVE_PK,
                "--json",
            ])
            .assert()
            .code(2)
            .stdout(predicates::str::is_empty());
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&stderr)
            .expect("--fields implies JSON mode: stderr is one document");
        assert_eq!(doc["error"]["code"], "usage.invalid");
        assert!(
            doc["error"]["message"]
                .as_str()
                .expect("message is a string")
                .contains("drop --fields or run without --dry-run"),
            "got: {stderr}"
        );
    })
    .await
    .expect("dry-run command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_create_dry_run_prints_the_intent_and_writes_nothing() {
    let server = MockServer::start().await;
    mount_profiles(&server, 2).await;
    // Folders are fetched unconditionally, even on the dry-run path
    // (module doc, src/commands/rule.rs) — cost accepted for one uniform
    // read-back contract; no `--folder` is given, so the fixture's content
    // never surfaces.
    mount_groups_fixture(&server, "p_groups.json", 2).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let human_assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--dry-run",
            ])
            .assert()
            .success();
        let human = String::from_utf8_lossy(&human_assert.get_output().stdout).into_owned();

        let json_assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "create",
                "a.example.com",
                "b.example.com",
                "--action",
                "block",
                "--profile",
                AGGRESSIVE_PK,
                "--dry-run",
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&json_assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc["requests"][0]["method"], "POST");
        assert_eq!(
            doc["requests"][0]["intent"]["hostnames"],
            serde_json::json!(["a.example.com", "b.example.com"])
        );
        assert_eq!(doc["requests"][0]["intent"]["action"], "block");
        assert_eq!(doc["requests"][0]["intent"]["enabled"], true);
        assert_eq!(
            doc["requests"][0]["intent"]["folder_id"],
            serde_json::Value::Null
        );

        insta::assert_snapshot!("rule_create_dry_run_human", human);
        insta::assert_snapshot!("rule_create_dry_run_json", json);
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_update_dry_run_prints_a_sparse_patch_including_root() {
    let server = MockServer::start().await;
    mount_profiles(&server, 2).await;
    // Folders are fetched unconditionally, even on the dry-run path and
    // for `--root` (module doc, src/commands/rule.rs).
    mount_groups_fixture(&server, "p_groups.json", 2).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let human_assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "x.example.com",
                "--root",
                "--profile",
                AGGRESSIVE_PK,
                "--dry-run",
            ])
            .assert()
            .success();
        let human = String::from_utf8_lossy(&human_assert.get_output().stdout).into_owned();

        let json_assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "update",
                "x.example.com",
                "--root",
                "--profile",
                AGGRESSIVE_PK,
                "--dry-run",
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&json_assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(
            doc["requests"][0]["intent"]["changes"],
            serde_json::json!({"folder_id": null})
        );

        insta::assert_snapshot!("rule_update_dry_run_human", human);
        insta::assert_snapshot!("rule_update_dry_run_json", json);
    })
    .await
    .expect("command runs");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rule_delete_dry_run_prints_one_request_per_hostname() {
    let server = MockServer::start().await;
    mount_profiles(&server, 2).await;
    mount_no_writes(&server).await;

    let uri = server.uri();
    let dir = tempdir();
    tokio::task::spawn_blocking(move || {
        let human_assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "*.wild.example.com",
                "--profile",
                AGGRESSIVE_PK,
                "--dry-run",
            ])
            .assert()
            .success();
        let human = String::from_utf8_lossy(&human_assert.get_output().stdout).into_owned();

        let json_assert = cdctl_against(&uri, dir.path())
            .args([
                "rule",
                "delete",
                "a.example.com",
                "*.wild.example.com",
                "--profile",
                AGGRESSIVE_PK,
                "--dry-run",
                "--json",
            ])
            .assert()
            .success();
        let json = String::from_utf8_lossy(&json_assert.get_output().stdout).into_owned();
        let doc: serde_json::Value = serde_json::from_str(&json).expect("valid JSON");
        assert_eq!(doc["requests"].as_array().unwrap().len(), 2);
        assert_eq!(doc["requests"][0]["method"], "DELETE");
        assert_eq!(
            doc["requests"][0]["path"],
            format!("/profiles/{AGGRESSIVE_PK}/rules/a.example.com")
        );
        assert_eq!(
            doc["requests"][1]["path"],
            format!("/profiles/{AGGRESSIVE_PK}/rules/%2A.wild.example.com")
        );
        assert_eq!(
            doc["requests"][0]["intent"],
            serde_json::json!({"hostname": "a.example.com"})
        );

        insta::assert_snapshot!("rule_delete_dry_run_human", human);
        insta::assert_snapshot!("rule_delete_dry_run_json", json);
    })
    .await
    .expect("command runs");
}
