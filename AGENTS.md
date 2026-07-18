# AGENTS.md

Instructions for any coding agent working in this repository (see [agents.md](https://agents.md)).

## What this repo is

`cdctl` — a CLI for the **Control D REST API** (`https://api.controld.com`), in Rust. Crate/repo
`controld-cli`, binary **`cdctl`** — **never rename the binary to `controld`**: Control D's DNS
daemon is already `ctrld`, and a trailing `-d` reads as daemon ([D1](docs/decisions.md#d1--crate-controld-cli-binary-cdctl)). `cdctl` manages the *account*
over REST; [`ctrld`](https://github.com/Control-D-Inc/ctrld) runs DNS on the machine.

Delivery is sliced per [`docs/roadmap.md`](docs/roadmap.md).

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
| [`docs/testing.md`](docs/testing.md) | Test layers, fixture policy, live-test isolation |
| [`docs/reference/`](docs/reference/) | OpenAPI spec + provenance; how the live API departs from it |

## Build and test

```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check

./scripts/fetch-spec.sh
cp .env.example .env
```

`cargo test` never touches the network — the live suite is opt-in (below). `fetch-spec.sh`
re-fetches the OpenAPI spec and fails if the 46 docs pages disagree. `.env` supplies
`CONTROLD_API_TOKEN`, needed only for the live suite and manual probes.

Toolchain: pins live in `Cargo.toml`; the edition/MSRV choice and stack rationale in
[D13](docs/decisions.md#d13--rust-stack-compile-verified-july-2026).

### The account behind `.env`

**Never assume it is disposable.** It may be someone's real account, and mutating it rewrites live
DNS behavior — some contributors will knowingly test against their own account, and that's their
call, not yours. Confine live probes to a temporary `cdctl-test-<timestamp>-<nonce>` profile and
delete it afterwards ([`docs/testing.md`, Live-test isolation](docs/testing.md#live-test-isolation)). Never touch
pre-existing profiles without the user saying so.

Never pass the token on a command line (`-H "Authorization: Bearer ..."`); argv is world-readable
via `ps`. Write a `curl` config file (mode 0600) and use `curl -K`.

### Testing

Layers, fixture policy, and the live-suite isolation rules live in
[`docs/testing.md`](docs/testing.md). The non-negotiables, wherever you read them:

- The live suite runs only with `CONTROLD_LIVE_TESTS=1` (plus `CONTROLD_API_TOKEN`); plain
  `cargo test` and CI never touch the network.
- **Never touch pre-existing profiles** (above) — every mutation stays inside the run's own
  fresh profile.
- New fixtures in `tests/fixtures/api/` must be sanitized: no emails, device names, real domains,
  public IPs, or account PKs.

### Release

`dist build` needs `man/` and `completions/` populated first (`dist plan` does not) — CI does
this via `.github/dist-build-setup.yml`; locally, run `cargo run -- man --out-dir man` and
the `completions` step in that file yourself before building.

`.github/workflows/release.yml` is machine-generated (`dist generate`) — never hand-edit it;
change `dist-workspace.toml` and regenerate.

## Conventions

- Comments and docs describe the **current state**, never the change or what it replaced — git
  history holds that. Commit messages are the exception: there, the change is the point.
- Help strings avoid semicolons — use parentheticals or separate sentences.
- `->` in comments and docs, never a unicode arrow; ASCII symbols generally (em dashes allowed,
  never the default connector).
- Emoji in docs only as GitHub shortcodes (`:warning:`), never raw unicode.

## API hazards

The Control D API is unversioned and departs from its own docs in ways that cause silent damage
(defaulted BLOCK rules, silently truncated writes, meaningless DELETE acks). Read
[`docs/reference/hazards.md`](docs/reference/hazards.md) before touching `src/api/` or any write
path.

## Contracts that are public API

Breaking either is a semver-major event. Test them like it.

- **Exit codes:** `0` ok, `1` generic, `2` usage, `3` not found, `4` auth, `5` forbidden/plan,
  `6` conflict, `7` confirmation required, **`8` retryable**, `130` SIGINT, `141` broken pipe
  (SIGPIPE, Unix). Exit `8` is retryable; everything else is terminal.
- **Output:** stdout carries **only** data — valid JSON or nothing; diagnostics go to stderr. The
  JSON schema is ours, normalized ([D2](docs/decisions.md#d2--the-cli-is-the-stability-layer-own-the-output-schema)): `PK` and raw `do`/`status` integers never appear.

## Scope

Personal accounts, documented API surface only ([D15](docs/decisions.md#d15--personal-accounts-only-orgs-addable-without-breaking-changes), [D16](docs/decisions.md#d16--documented-surface-only)). Org support must stay additive;
`cdctl api` is the escape hatch for everything else.

