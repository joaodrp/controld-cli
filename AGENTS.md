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
| [`docs/plan.md`](docs/plan.md) | Release slices (v0.1-1.0), phases, test gates |
| [`docs/reference/`](docs/reference/) | OpenAPI spec + provenance; how the live API departs from it |

## Build and test

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`cargo test` never touches the network — the live suite is opt-in (below).

### Release

`dist build` needs `man/` and `completions/` populated first (`dist plan` does not) — CI does
this via `.github/workflows/build-setup.yml`; locally, run `cargo run -- man --out-dir man` and
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

- Every mutation happens inside a fresh `cdctl-test-<timestamp>-<nonce>` profile, created and
  deleted by the run. **Never touch pre-existing profiles** without the user saying so — the
  account behind the token may be someone's real account.
- Never pass the token on a command line (`-H "Authorization: Bearer ..."`) — argv is
  world-readable via `ps`.

## Fixtures

`tests/fixtures/api/` holds **real, sanitized API responses** — prefer them to hand-written mocks;
they carry every hazard in the API-hazards index. Sanitize any new fixture: no emails, device
names, real domains, public IPs, or account PKs.
