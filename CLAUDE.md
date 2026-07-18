# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## What this repo is

`cdctl` — the first CLI for the **Control D REST API** (`https://api.controld.com`), in Rust. Crate/repo
`controld-cli`, binary **`cdctl`** — **never rename the binary to `controld`** (D1: Control D's DNS
daemon is already `ctrld`, and a trailing `-d` reads as daemon). `cdctl` manages the *account* over
REST; [`ctrld`](https://github.com/Control-D-Inc/ctrld) runs DNS on the machine. They coexist.

Delivery is sliced per [`docs/roadmap.md`](docs/roadmap.md): v0.1 ships profiles, rules, folders,
and the `cdctl api` escape hatch; `rule import`/`restore` follow in v0.2.

## Doc map

[`docs/decisions.md`](docs/decisions.md) is **authoritative**. Several decisions look wrong until you
read the reasoning and the cost accepted (no keyring; no TTY-based format switching; one retryable
exit code). Changing a decision requires the maintainer's explicit approval; record the change by
amending that file, never by drifting.

| Doc | |
| --- | --- |
| [`docs/decisions.md`](docs/decisions.md) | D1-D19. Authoritative. |
| [`docs/design.md`](docs/design.md) | Command surface, all API operations mapped |
| [`docs/commands.md`](docs/commands.md) | Per-command flags, table columns, JSON field names |
| [`docs/roadmap.md`](docs/roadmap.md) | What ships next (v0.2-1.0) and each slice's test gates |
| [`docs/reference/`](docs/reference/) | OpenAPI spec + provenance; how the live API departs from it |

## Commands

```bash
./scripts/fetch-spec.sh          # re-fetch the OpenAPI spec; fails if the 46 docs pages disagree
cp .env.example .env             # then add a CONTROLD_API_TOKEN
```

`cargo build` / `cargo test` / `cargo clippy --all-targets`. Toolchain and dependency choices
are pinned in `Cargo.toml`; the edition/MSRV rationale is in [`AGENTS.md`](AGENTS.md).

### The account behind `.env`

**Never assume it is disposable.** It may be someone's real account, and mutating it rewrites live
DNS behavior — some contributors will knowingly test against their own account, and that's their
call, not yours. Confine live probes to a temporary `cdctl-test-<timestamp>-<nonce>` profile and
delete it afterwards (isolation rules: [`AGENTS.md`](AGENTS.md), section Live-test isolation). Never touch
pre-existing profiles without the user saying so.

Never pass the token on a command line (`-H "Authorization: Bearer ..."`); argv is world-readable via
`ps`. Write a `curl` config file (mode 0600) and use `curl -K`.

## API hazards — index

The Control D API is unversioned; everything below was verified live and contradicts their docs or spec.
Evidence: [`read-verification.md`](docs/reference/read-verification.md) (read path) and
[`write-verification.md`](docs/reference/write-verification.md) (mutations). **If the live API ever
contradicts this list, update the reference doc and this index.**

- **Missing `action.do` is an error, never a default** — defaulting it silently creates a BLOCK rule,
  the worst failure a DNS tool can have.
- **Auth failures return HTTP 400, not 401** — classify on `error.code`, never on HTTP status.
- **Three envelope shapes**, and `body` becomes `[]` on error — deserialize `body` as `Value`, unwrap
  per operation.
- **`GET /profiles/{id}/rules` must omit the folder segment** — the documented `folder_id=0` 404s.
  The segment-less path returns **root rules only**; a foldered rule is invisible on it. Seeing every
  rule in a profile needs one `GET /rules/{folder_id}` per folder on top of it.
- **`content-type: application/json` can carry a 0-byte body.**
- **Error messages can be multi-line dumps** — never parse them.
- **`error.code` is a coarse bucket** — classify on its 3-digit HTTP prefix
  ([`error-codes.md`](docs/reference/error-codes.md)).
- **Filter level names cannot be constructed** — read `levels[]`.
- **Writes are form-encoded and the server silently drops form variables past ~1001** — chunk at 500,
  scalars first, then **re-fetch and verify the full desired state** (action, enabled, `via`, `via6`,
  folder). Client caps: 500 hostnames, 50 IPs. 10,000 rules/profile. No bulk delete; percent-encode
  hostnames into DELETE paths (`*` -> `%2A`).
- **`body: []` is not an error marker** — successful deletes return it too. Branch on
  `success`/`error` only.
- **`DELETE` of a non-matching hostname returns `success: true`** (`"Custom rule(s) deleted"`) — a
  silent no-op. Delete success proves nothing; check existence before, not after.

## Contracts that are public API

Breaking either is a semver-major event. Test them like it.

- **Exit codes.** `0` ok, `1` generic, `2` usage, `3` not found, `4` auth, `5` forbidden/plan,
  `6` conflict, `7` confirmation required, **`8` retryable**, `130` SIGINT.
  **Exit 8 is retryable; everything else is terminal.**
- **Output.** stdout carries **only** data — valid JSON or nothing; diagnostics to stderr. The JSON
  schema is **ours**, normalized (D2): `PK` and raw `do`/`status` integers never appear.

## Scope

Personal accounts, documented API surface only (D15, D16). Orgs must stay additive; `cdctl api` is
the escape hatch for everything else.

## Tests

`tests/fixtures/api/` holds **real, sanitized API responses** — prefer them to hand-written mocks;
they carry every hazard above. Sanitize new fixtures: no emails, device names, real domains, public
IPs, or account PKs.
