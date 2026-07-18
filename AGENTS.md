# AGENTS.md

Instructions for any coding agent working in this repository (see [agents.md](https://agents.md)).

## What this repo is

`cdctl` — a CLI for the **Control D REST API** (`https://api.controld.com`), in Rust. Crate/repo
`controld-cli`, binary **`cdctl`** — **never rename the binary to `controld`**: Control D's DNS
daemon is already `ctrld`, and a trailing `-d` reads as daemon (D1). `cdctl` manages the *account*
over REST; [`ctrld`](https://github.com/Control-D-Inc/ctrld) runs DNS on the machine. They coexist.

## Doc map

[`docs/decisions.md`](docs/decisions.md) is **authoritative**. Several decisions look wrong until
you read the reasoning and the cost accepted (no keyring; no TTY-based format switching; one
retryable exit code). Changing a decision needs the maintainer's explicit approval, recorded by
amending that file — never by drifting from it.

| Doc | |
| --- | --- |
| [`docs/decisions.md`](docs/decisions.md) | D1-D19. Authoritative. |
| [`docs/design.md`](docs/design.md) | Command surface, all API operations mapped |
| [`docs/commands.md`](docs/commands.md) | Per-command flags, table columns, JSON field names |
| [`docs/roadmap.md`](docs/roadmap.md) | What ships next (v0.2-1.0) and each slice's test gates |
| [`docs/reference/`](docs/reference/) | OpenAPI spec + provenance; how the live API departs from it |

## Build and test

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`cargo test` never touches the network — the live suite is opt-in (below).

Toolchain: edition 2024, MSRV 1.85 (the first release with edition-2024 support; development uses
latest stable) — exact dependency pins live in `Cargo.toml`, the async/rustls stack rationale in
[decisions.md](docs/decisions.md).

### Test layers

| Layer | Tool | What |
| --- | --- | --- |
| Envelope/models | `serde` on fixtures | Every shape hazard, straight from **real captured payloads** — no HTTP server needed |
| Client behavior | `wiremock` | Retry (GET-only, exactly-once writes), origin rules, the 0-byte JSON 500, headers |
| Command contract | `assert_cmd` | Exit codes; stdout clean on error |
| Snapshots | `insta` | `--help`, JSON shapes |
| Live | opt-in suite | Runs only with `CONTROLD_LIVE_TESTS=1` (plus `CONTROLD_API_TOKEN`); isolated to a randomized profile (below) |

Exit codes and the retryable set are **public API**. Test them like it. Model types deserialize
leniently in the binary; fixture tests use `deny_unknown_fields`, so an API field addition fails
tests instead of passing silently — the drift tripwire for an unversioned API.

### Release

`dist build` needs `man/` and `completions/` populated first (`dist plan` does not) — CI does
this via `.github/dist-build-setup.yml`; locally, run `cargo run -- man --out-dir man` and
the `completions` step in that file yourself before building.

## API hazards

The Control D API is unversioned; [`CLAUDE.md`](CLAUDE.md)'s "API hazards" index catalogues every
verified departure from its docs/spec — missing `action.do`, HTTP 400 on auth failure, the
folder-segment-less rules path, silently dropped form variables past ~1001, and more. Read it
before touching `src/api/` or any write path. Evidence lives in
[`docs/reference/read-verification.md`](docs/reference/read-verification.md) and
[`docs/reference/write-verification.md`](docs/reference/write-verification.md).

## Contracts that are public API

Breaking either is a semver-major event. Test them like it.

- **Exit codes:** `0` ok, `1` generic, `2` usage, `3` not found, `4` auth, `5` forbidden/plan,
  `6` conflict, `7` confirmation required, **`8` retryable**, `130` SIGINT. Exit `8` is retryable;
  everything else is terminal.
- **Output:** stdout carries **only** data — valid JSON or nothing; diagnostics go to stderr. The
  JSON schema is ours, normalized (D2): `PK` and raw `do`/`status` integers never appear.

## Scope

Personal accounts, documented API surface only (D15, D16). Org support must stay additive;
`cdctl api` is the escape hatch for everything else.

## Live-test isolation

The live suite (`tests/live.rs`) runs only with `CONTROLD_LIVE_TESTS=1` set (plus
`CONTROLD_API_TOKEN`) — plain `cargo test` and CI never run it, and it never runs unattended.

Profiles are the isolation boundary — rules, folders, the default rule, filters, services, and
options are all profile-scoped, so a run that stays inside its own profile is safe on **any**
account, not just a throwaway.

- **Setup:** every mutation happens inside a fresh `cdctl-test-<timestamp>-<nonce>` profile,
  created and deleted by the run; each run gets a fresh 10,000-rule quota. **Never touch
  pre-existing profiles** without the user saying so — the account behind the token may be
  someone's real account.
- **Teardown:** delete the profile; its contents go with it. Devices pointed at the test profile
  are deleted **before** the profile — deleting a profile out from under a device is unverified.
- **Leaks:** the prefix makes crashed-run leftovers identifiable; the suite sweeps stale
  `cdctl-test-*` profiles by name + age on start (plans cap profile counts, so accumulated leaks
  would eventually fail setup itself).
- **Limits:** account-scoped surfaces (devices, access, proxy, account, billing, network) cannot
  be profile-isolated; their tests (v0.4) use the same `cdctl-test-` naming on the resources they
  create.
- Never pass the token on a command line (`-H "Authorization: Bearer ..."`) — argv is
  world-readable via `ps`.

## Fixtures

`tests/fixtures/api/` holds **real, sanitized API responses** — prefer them to hand-written mocks;
they carry every hazard in the API-hazards index. Sanitize any new fixture: no emails, device
names, real domains, public IPs, or account PKs.
