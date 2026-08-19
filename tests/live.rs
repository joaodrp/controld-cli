//! Opt-in end-to-end suite against the REAL Control D API — no wiremock.
//!
//! Skipped unless `CONTROLD_LIVE_TESTS=1` (plain `cargo test` and CI never
//! run it). The profile is the isolation boundary (docs/testing.md "Live-test
//! isolation"): every mutation happens inside a fresh `cdctl-test-<ts>-<nonce>`
//! profile, pre-existing profiles are never mutated and their contents never
//! read, and a startup sweep reaps stale leftovers from crashed runs by
//! name + age. Profile create/delete go through `cdctl api` (the escape
//! hatch) because typed profile writes are Phase 5 and `profile delete`
//! demands `--confirm=<name>` rather than `--yes` (docs/commands.md
//! "Confirmation matrix").

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use assert_cmd::Command;

/// A hermetic `cdctl` aimed at the real API: empty environment, config under
/// a private tempdir, no `CONTROLD_API_URL` override — the default base URL
/// is part of what this suite verifies.
fn live_cdctl(config_home: &Path, token: &str) -> Command {
    // Compile-time binary path, not `cargo_bin` - this also runs from
    // `ProfileGuard`'s `Drop`, where a panic during unwind would abort.
    let mut cmd = Command::from_std(std::process::Command::new(env!("CARGO_BIN_EXE_cdctl")));
    cmd.env_clear()
        .env("XDG_CONFIG_HOME", config_home)
        .env("CONTROLD_API_TOKEN", token)
        .timeout(Duration::from_secs(60));
    cmd
}

/// Parses `stdout` as the single JSON document the output contract allows
/// (normalized for typed commands, the verbatim upstream body for
/// `cdctl api`); on failure the panic carries stdout/stderr so a live
/// failure is diagnosable from CI logs — never the token, which responses
/// don't echo and cdctl never prints.
fn json_stdout(output: &std::process::Output, what: &str) -> serde_json::Value {
    serde_json::from_slice(&output.stdout).unwrap_or_else(|e| {
        panic!(
            "{what}: stdout is not valid JSON: {e}\nstdout: {}\nstderr: {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr),
        )
    })
}

/// Matches `cdctl-test-<unix_secs>-<suffix>` exactly: the prefix, then an
/// all-ASCII-digit timestamp (no sign), then a dash, then a nonempty suffix.
/// Anything that doesn't fit — including every pre-existing profile on the
/// account — comes back `None` and is left untouched; this is the
/// account-safety boundary.
fn stale_test_profile_timestamp(name: &str) -> Option<u64> {
    let rest = name.strip_prefix("cdctl-test-")?;
    let (ts, suffix) = rest.split_once('-')?;
    if suffix.is_empty() || !ts.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    ts.parse().ok()
}

#[test]
fn stale_matcher_accepts_only_the_exact_test_profile_shape() {
    assert_eq!(
        stale_test_profile_timestamp("cdctl-test-1234-abc"),
        Some(1234)
    );
    // The split is at the first dash: a multi-dash suffix keeps the first
    // segment as the timestamp.
    assert_eq!(stale_test_profile_timestamp("cdctl-test-12-34-x"), Some(12));

    for name in [
        "production",
        "my-cdctl-test-1-x",                    // prefix must be at the start
        "cdctl-test-1234",                      // no suffix dash
        "cdctl-test-1234-",                     // empty suffix
        "cdctl-test-abc-x",                     // non-numeric timestamp
        "cdctl-test-+1234-x",                   // a sign is not a digit
        "cdctl-test--x",                        // empty timestamp
        "cdctl-test-99999999999999999999999-x", // overflows u64
    ] {
        assert_eq!(
            stale_test_profile_timestamp(name),
            None,
            "{name} must never be swept"
        );
    }
}

/// Deletes stale `cdctl-test-*` profiles left by crashed prior runs (older
/// than an hour). Runs before anything else so accumulated leaks can't fail
/// setup for this run too.
fn sweep_stale_profiles(config_home: &Path, token: &str) {
    let assert = live_cdctl(config_home, token)
        .args(["profile", "list", "--json"])
        .assert()
        .success();
    let profiles = json_stdout(assert.get_output(), "profile list --json (sweep)");
    let profiles = profiles
        .as_array()
        .expect("profile list --json is an array");

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock is after the epoch")
        .as_secs();

    for profile in profiles {
        // `profile list --json` is cdctl's own normalized schema; a missing
        // field is a cdctl bug, never a skippable entry.
        let name = profile["name"].as_str().expect("profile has a name");
        let id = profile["id"].as_str().expect("profile has an id");
        let Some(created) = stale_test_profile_timestamp(name) else {
            continue;
        };
        let age = now.saturating_sub(created);
        if age <= 3600 {
            continue;
        }
        eprintln!("sweep: deleting stale leftover profile {name} ({id}), age {age}s");
        let path = format!("/profiles/{id}");
        // Best-effort: a concurrent run may have reaped it first, and a
        // failed sweep delete must not fail this run's own suite.
        let output = live_cdctl(config_home, token)
            .args(["api", &path, "-X", "DELETE", "--yes"])
            .output()
            .expect("cdctl spawns for the sweep");
        if !output.status.success() {
            eprintln!(
                "sweep: delete of {name} ({id}) exited {} — leaving it for a later sweep",
                output.status
            );
        }
    }
}

/// Best-effort cleanup for the window where a `POST /profiles` may have
/// landed but no `ProfileGuard` exists to own it: resolve the run-unique
/// name via `profile list` and delete the match. Runs on an already-failing
/// path, so it never panics — every miss is logged and left to the sweep.
fn reap_by_name(config_home: &Path, token: &str, name: &str) {
    let manual = format!("delete profile {name} manually if it exists");
    let Ok(output) = live_cdctl(config_home, token)
        .args(["profile", "list", "--json"])
        .output()
    else {
        eprintln!("reap: cdctl failed to spawn — {manual}");
        return;
    };
    let Ok(profiles) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
        eprintln!("reap: profile list output unreadable — {manual}");
        return;
    };
    let Some(id) = profiles
        .as_array()
        .into_iter()
        .flatten()
        .find(|p| p["name"] == name)
        .and_then(|p| p["id"].as_str())
    else {
        eprintln!("reap: no profile named {name} found — nothing landed");
        return;
    };
    let path = format!("/profiles/{id}");
    match live_cdctl(config_home, token)
        .args(["api", &path, "-X", "DELETE", "--yes"])
        .output()
    {
        Ok(output) if output.status.success() => {
            eprintln!("reap: deleted leaked profile {name} ({id})");
        }
        _ => eprintln!("reap: delete of {name} ({id}) failed — {manual}"),
    }
}

/// Owns the test profile: `create` acquires it and `teardown`/`Drop` release
/// it. `delete` shells out with `std::process::Command` (not `assert_cmd`)
/// so it also runs cleanly from `Drop` during a panicking unwind; a failed
/// assertion mid-suite still leaves the profile deleted. Errors inside
/// `Drop` are swallowed but logged — a leak must be visible, never silent.
struct ProfileGuard {
    pk: String,
    name: String,
    token: String,
    config_home: PathBuf,
    armed: bool,
}

impl ProfileGuard {
    /// `POST /profiles` via the escape hatch, named `cdctl-test-<ts>-<nonce>`;
    /// the guard is armed from birth.
    fn create(config_home: &Path, token: &str) -> Self {
        // The pid + sub-second nanos suffix is unique enough to avoid
        // collisions between runs without a `rand` dependency. Folded to 40
        // bits (10 hex chars) because the API rejects profile names over 32
        // characters, and prefix + 10-digit timestamp + dashes leave exactly
        // 10 (write-verification.md).
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock is after the epoch");
        let nonce =
            (u64::from(std::process::id()) << 30 | u64::from(now.subsec_nanos())) & 0xff_ffff_ffff;
        let name = format!("cdctl-test-{}-{nonce:x}", now.as_secs());
        let create_form = format!("name={name}");

        // Both failure arms below reap by name before panicking: a nonzero
        // exit is an ambiguous write and a PK-less body is a landed write
        // with no handle — either way the profile may exist while no guard
        // does, and the hour-late sweep is a backstop, not a cleanup.
        let output = live_cdctl(config_home, token)
            .args([
                "api",
                "/profiles",
                "-X",
                "POST",
                "-F",
                &create_form,
                "--yes",
            ])
            .output()
            .expect("cdctl spawns for profile create");
        if !output.status.success() {
            reap_by_name(config_home, token, &name);
            panic!(
                "POST /profiles exited {}\nstderr: {}",
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let created = json_stdout(&output, "POST /profiles");
        // The PK is opaque; `body.profiles[]` — an array — is the
        // live-verified shape (write-verification.md).
        let Some(pk) = created["body"]["profiles"][0]["PK"].as_str() else {
            reap_by_name(config_home, token, &name);
            panic!("POST /profiles: no body.profiles[0].PK in {created}");
        };
        Self {
            pk: pk.to_string(),
            name,
            token: token.to_string(),
            config_home: config_home.to_path_buf(),
            armed: true,
        }
    }

    fn delete(&self) -> std::io::Result<std::process::Output> {
        // Bounded by `live_cdctl`'s 60s timeout: a hung API must not stall
        // teardown - and with it the leak guard - indefinitely.
        let path = format!("/profiles/{}", self.pk);
        live_cdctl(&self.config_home, &self.token)
            .args(["api", &path, "-X", "DELETE", "--yes"])
            .output()
    }

    /// Explicit happy-path teardown: delete, confirm the name is gone.
    /// Consuming `self` makes a second teardown a compile error, and the
    /// disarm happens right after the delete lands so a flaky verification
    /// read-back can't trigger a spurious re-delete from `Drop`.
    fn teardown(mut self) {
        let output = self.delete().expect("cdctl spawns for teardown");
        assert!(
            output.status.success(),
            "explicit profile teardown via cdctl api DELETE: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        self.armed = false;

        let assert = live_cdctl(&self.config_home, &self.token)
            .args(["profile", "list", "--json"])
            .assert()
            .success();
        let profiles = json_stdout(assert.get_output(), "profile list --json (teardown check)");
        assert!(
            profiles
                .as_array()
                .expect("array")
                .iter()
                .all(|p| p["name"] != self.name.as_str()),
            "deleted profile must not reappear in profile list"
        );
    }
}

impl Drop for ProfileGuard {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        match self.delete() {
            Ok(output) if output.status.success() => {}
            Ok(output) => eprintln!(
                "leak guard: cdctl api delete exited {} for profile {} ({}) — delete it manually",
                output.status, self.name, self.pk
            ),
            Err(e) => eprintln!(
                "leak guard: could not delete profile {} ({}) — delete it manually: {e}",
                self.name, self.pk
            ),
        }
    }
}

/// `rule list --json` inside the test profile, as an owned array.
fn list_rules(config_home: &Path, token: &str, pk: &str, what: &str) -> Vec<serde_json::Value> {
    let assert = live_cdctl(config_home, token)
        .args(["rule", "list", "--profile", pk, "--json"])
        .assert()
        .success();
    json_stdout(assert.get_output(), what)
        .as_array()
        .expect("rule list --json is an array")
        .clone()
}

/// Steps a-h: the folder + rule lifecycle inside the isolated profile.
#[expect(
    clippy::too_many_lines,
    reason = "one flow, one profile, one teardown — splitting further would just move the \
              narrative across files without reducing what a reader must hold at once"
)]
fn drive_profile_lifecycle(config_home: &Path, token: &str, pk: &str, profile_name: &str) {
    // a. profile list --json
    let assert = live_cdctl(config_home, token)
        .args(["profile", "list", "--json"])
        .assert()
        .success();
    let profiles = json_stdout(assert.get_output(), "profile list --json");
    assert!(
        profiles
            .as_array()
            .expect("array")
            .iter()
            .any(|p| p["id"] == pk && p["name"] == profile_name),
        "new profile must appear in profile list"
    );

    // b. folder create
    let assert = live_cdctl(config_home, token)
        .args(["folder", "create", "smoke", "--profile", pk, "--json"])
        .assert()
        .success();
    let folder = json_stdout(assert.get_output(), "folder create smoke --json");
    let folder_id = folder["id"]
        .as_i64()
        .unwrap_or_else(|| panic!("folder create: no integer id in {folder}"));
    let folder_id_arg = folder_id.to_string();

    // c. rule create (two targets: plain + wildcard)
    let one = "one.cdctl-smoke.example.com";
    let wild = "*.wild.cdctl-smoke.example.com";
    let assert = live_cdctl(config_home, token)
        .args([
            "rule",
            "create",
            one,
            wild,
            "--action",
            "block",
            "--folder",
            &folder_id_arg,
            "--profile",
            pk,
            "--json",
        ])
        .assert()
        .success();
    let created_rules = json_stdout(assert.get_output(), "rule create --json");
    let created_rules = created_rules
        .as_array()
        .expect("rule create --json prints the normalized read-back array");
    assert_eq!(created_rules.len(), 2, "read-back must show both targets");
    for rule in created_rules {
        assert_eq!(rule["action"], "block");
        assert_eq!(
            rule["folder_id"].as_i64(),
            Some(folder_id),
            "rule landed inside the smoke folder"
        );
        assert_eq!(rule["enabled"], true);
    }

    // d. rule update --disabled
    let assert = live_cdctl(config_home, token)
        .args([
            "rule",
            "update",
            one,
            "--disabled",
            "--profile",
            pk,
            "--json",
        ])
        .assert()
        .success();
    let updated = json_stdout(assert.get_output(), "rule update --disabled --json");
    let updated = updated.as_array().expect("array");
    assert_eq!(updated.len(), 1);
    assert_eq!(updated[0]["hostname"], one);
    assert_eq!(updated[0]["enabled"], false);
    // PUT is a merge: the fields the update did not send must survive.
    assert_eq!(updated[0]["action"], "block");
    assert_eq!(updated[0]["folder_id"].as_i64(), Some(folder_id));

    // e. rule list
    let listed = list_rules(config_home, token, pk, "rule list --json");
    assert_eq!(listed.len(), 2, "both rules still present");
    assert!(
        listed.iter().any(|r| r["hostname"] == wild),
        "wildcard hostname intact"
    );

    // f. rule delete (percent-encoded wildcard path). DELETE has no
    // read-back of its own (D11), so the re-list is what proves the
    // wildcard went away — and that nothing else did.
    live_cdctl(config_home, token)
        .args(["rule", "delete", wild, "--profile", pk, "--yes"])
        .assert()
        .success();
    let after_delete = list_rules(config_home, token, pk, "rule list --json (after delete)");
    assert_eq!(after_delete.len(), 1, "only the plain rule remains");
    assert_eq!(after_delete[0]["hostname"], one);

    // g. folder delete takes the contained rule with it.
    live_cdctl(config_home, token)
        .args(["folder", "delete", &folder_id_arg, "--profile", pk, "--yes"])
        .assert()
        .success();
    let after_folder = list_rules(
        config_home,
        token,
        pk,
        "rule list --json (after folder delete)",
    );
    assert!(
        after_folder.is_empty(),
        "folder delete cascades to its rules"
    );
    // h. case-aware target resolution, split out (below) to fit clippy's cap.
    drive_mixed_case_resolution(config_home, token, pk);
}

/// A second, differently-cased variant of `mixed`, created via the same
/// `api` escape hatch as its own distinct rule: the resolution flow in
/// [`drive_mixed_case_resolution`] rests entirely on the server storing (and
/// matching) hostnames case-sensitively, never folding case variants onto
/// one rule (write-verification.md "coexist as two distinct rules").
/// Re-probing that live here, not just in the unit/e2e suites, means a
/// server-side fold shows up as a failed coexistence assertion, not a
/// confusing resolution mismatch further down. Cleaned up before returning:
/// the resolution flow that follows targets `mixed`'s all-lowercase form and
/// requires it to resolve unambiguously to `mixed` alone.
fn probe_case_coexistence(config_home: &Path, token: &str, pk: &str, mixed: &str) {
    let coexisting_variant = mixed.to_ascii_uppercase();
    let rules_path = format!("/profiles/{pk}/rules");
    live_cdctl(config_home, token)
        .args([
            "api",
            &rules_path,
            "-X",
            "POST",
            "-F",
            "do=0",
            "-F",
            "status=1",
            "-F",
            &format!("hostnames[]={coexisting_variant}"),
            "--yes",
        ])
        .assert()
        .success();
    let coexisting = list_rules(
        config_home,
        token,
        pk,
        "rule list --json (case coexistence probe)",
    );
    assert!(
        coexisting.iter().any(|r| r["hostname"] == mixed),
        "the mixed-case rule must be present"
    );
    assert!(
        coexisting
            .iter()
            .any(|r| r["hostname"] == coexisting_variant),
        "the second case variant must coexist as its own distinct rule, not fold onto the first"
    );
    live_cdctl(config_home, token)
        .args([
            "api",
            &format!("{rules_path}/{coexisting_variant}"),
            "-X",
            "DELETE",
            "--yes",
        ])
        .assert()
        .success();
}

/// Step h: case-aware target resolution (commands.md#rule,
/// write-verification.md "coexist as two distinct rules"): `rule create`
/// always lowercases, so a mixed-case rule is created raw via the `api`
/// escape hatch, then exercised through the typed CLI with a lowercase
/// target — proving `rule update`/`rule delete` resolve it by stored PK
/// rather than by a client-side case fold that would 400/no-op upstream.
fn drive_mixed_case_resolution(config_home: &Path, token: &str, pk: &str) {
    let mixed = "MiXeD.cdctl-smoke.example.com";
    let rules_path = format!("/profiles/{pk}/rules");
    live_cdctl(config_home, token)
        .args([
            "api",
            &rules_path,
            "-X",
            "POST",
            "-F",
            "do=0",
            "-F",
            "status=1",
            "-F",
            &format!("hostnames[]={mixed}"),
            "--yes",
        ])
        .assert()
        .success();
    probe_case_coexistence(config_home, token, pk, mixed);

    let assert = live_cdctl(config_home, token)
        .args([
            "rule",
            "update",
            &mixed.to_ascii_lowercase(),
            "--disabled",
            "--profile",
            pk,
            "--json",
        ])
        .assert()
        .success();
    let updated = json_stdout(
        assert.get_output(),
        "rule update (mixed-case target) --json",
    );
    let updated = updated.as_array().expect("array");
    assert_eq!(updated.len(), 1);
    assert_eq!(
        updated[0]["hostname"], mixed,
        "the resolved stored PK keeps its original mixed case"
    );
    assert_eq!(updated[0]["enabled"], false);

    live_cdctl(config_home, token)
        .args([
            "rule",
            "delete",
            &mixed.to_ascii_lowercase(),
            "--profile",
            pk,
            "--yes",
        ])
        .assert()
        .success();
    let after_mixed_delete = list_rules(
        config_home,
        token,
        pk,
        "rule list --json (after mixed-case delete)",
    );
    assert!(
        !after_mixed_delete.iter().any(|r| r["hostname"] == mixed),
        "the mixed-case rule must be gone"
    );
}

/// Read-only: `device list` normalizes whatever the account holds, and
/// `device get` by id finds the first entry again. Only identity is compared
/// across the two requests: `ctrld.last_fetch`, `clients`, and a `pending`
/// flip can legitimately move between them. Nothing is created or mutated,
/// so this needs no profile isolation.
fn drive_device_reads(config_home: &Path, token: &str) {
    let output = live_cdctl(config_home, token)
        .args(["device", "list", "--json"])
        .output()
        .expect("device list runs");
    assert!(output.status.success(), "device list: {output:?}");
    let devices = json_stdout(&output, "device list");
    let devices = devices.as_array().expect("device list prints an array");
    let Some(first) = devices.first() else {
        eprintln!("device get: skipped, the account has no devices");
        return;
    };
    let id = first["id"].as_str().expect("id is a string");
    let output = live_cdctl(config_home, token)
        .args(["device", "get", id, "--json"])
        .output()
        .expect("device get runs");
    assert!(output.status.success(), "device get: {output:?}");
    let got = json_stdout(&output, "device get");
    assert_eq!(got["id"], first["id"]);
    assert_eq!(got["name"], first["name"]);
    assert_eq!(got["profile"], first["profile"]);
}

#[test]
fn live_smoke() {
    // CI and plain `cargo test` take this path. Any other set value is a
    // config error: `CONTROLD_LIVE_TESTS=true` must fail loudly, not skip
    // while reporting ok.
    let gate = std::env::var("CONTROLD_LIVE_TESTS").unwrap_or_default();
    if gate.is_empty() {
        eprintln!(
            "skipped: set CONTROLD_LIVE_TESTS=1 and CONTROLD_API_TOKEN to run the live suite \
             against the real Control D API"
        );
        return;
    }
    assert_eq!(
        gate, "1",
        "CONTROLD_LIVE_TESTS must be `1` (or unset to skip)"
    );
    // Explicit opt-in without a token is a config error, never a silent skip.
    let token = std::env::var("CONTROLD_API_TOKEN").unwrap_or_default();
    assert!(
        !token.is_empty(),
        "CONTROLD_LIVE_TESTS=1 requires CONTROLD_API_TOKEN to be set"
    );

    let dir = tempfile::tempdir().expect("tempdir");
    let config_home = dir.path();

    sweep_stale_profiles(config_home, &token);
    drive_device_reads(config_home, &token);

    let guard = ProfileGuard::create(config_home, &token);
    drive_profile_lifecycle(config_home, &token, &guard.pk, &guard.name);

    // No devices were created, so the devices-before-profile ordering rule
    // (docs/testing.md) is satisfied vacuously.
    guard.teardown();
}
