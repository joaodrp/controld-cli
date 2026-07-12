//! Binary-level contract tests (D18: no lib target — the CLI contract is the
//! public API, so tests drive `cdctl` itself).

use std::process::Stdio;
use std::time::Duration;

use assert_cmd::Command;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// A hermetic `cdctl`: empty environment, config under a private tempdir.
fn cdctl(config_home: &std::path::Path) -> Command {
    let mut cmd = Command::cargo_bin("cdctl").expect("binary builds");
    cmd.env_clear().env("XDG_CONFIG_HOME", config_home);
    cmd
}

fn tempdir() -> tempfile::TempDir {
    tempfile::tempdir().expect("tempdir")
}

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
    for args in [
        ["completions", "bash", "--json"].as_slice(),
        ["reference", "--json"].as_slice(),
    ] {
        let assert = cdctl(dir.path()).args(args).assert().code(2);
        assert.stdout(predicates::str::is_empty());
    }
    // Ambient CONTROLD_OUTPUT=json is not a conflict.
    cdctl(dir.path())
        .env("CONTROLD_OUTPUT", "json")
        .args(["completions", "bash"])
        .assert()
        .success();
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

    insta::with_settings!({filters => vec![(r"127\.0\.0\.1:\d+", "127.0.0.1:[port]")]}, {
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

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_misspelled_field_draws_a_warning() {
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
            .args(["auth", "status", "--fields", "emial"])
            .assert()
            .success();
        let doc: serde_json::Value =
            serde_json::from_slice(&assert.get_output().stdout).expect("data");
        assert_eq!(doc, serde_json::json!({}));
        let stderr = String::from_utf8_lossy(&assert.get_output().stderr).into_owned();
        assert!(stderr.contains("no such field \"emial\""), "got: {stderr}");
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
            "hostnames%5B%5D=a.example",
        ))
        .and(wiremock::matchers::body_string_contains(
            "hostnames%5B%5D=b.example",
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
        .and(wiremock::matchers::body_string_contains(
            "ips%5B%5D=1.2.3.4",
        ))
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
