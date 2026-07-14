# Plan

Spec extracted and validated, API verified live (reads **and** writes), stack compile-verified,
decisions recorded. Command contracts in [commands.md](commands.md).

**Delivery is vertically sliced ([D17](decisions.md)).** Each phase past Phase 2 ships a release.
Every global contract — exit codes, output schema, error envelope, shared flags, config format —
is complete in v0.1 and **frozen from then on**, so later groups only add commands. Anything not
yet typed is reachable via `cdctl api` from day one.

| Release | Ships |
| --- | --- |
| v0.1 | Core + `cdctl api` + `profile list/get` + `rule`/`folder` CRUD + release machinery |
| v0.2 | `rule import` + `rule restore` |
| v0.3 | Protection: `profile` writes/options/default, `filter *`, `service *` |
| v0.4 | Fleet & account: `device *`, `access *`, `proxy`, `analytics`, `account`, `billing`, `network`, `ip` |
| 1.0 | The full mapped surface |

## Phase 0 — Write encoding :white_check_mark: done *(investigation, not code — no Rust exists yet)*

[reference/write-verification.md](reference/write-verification.md):

- No contradiction existed — `hostnames[]` and `hostnames[0]` both work; the *bare repeat* fails.
  **We send `hostnames[]`.**
- Backend is **PHP** (leaked a `print_r()` dump). Upstream messages can be multi-line.
- `Content-Type` ignored — we don't rely on it ([D16](decisions.md)).
- `groups/import` **500s** — dropped.
- :warning: **The server parses ~1001 form vars and silently drops the rest** — a 1000-hostname batch 200s
  and stores 999. Chunk at 500, scalars first, **verify the full desired state** (a bare count can
  be masked by concurrent changes; a dropped trailing `status=0` silently defaults to enabled).
  Oversized batches and batches crossing the **10,000 rules/profile cap** fail atomically. **No
  bulk delete exists.**
- Redirect is **plan-gated (402)** — proxy validation unverifiable on a free account.
- `PUT /rules` **merges**; folders need no `do`; filter levels **swap atomically**; full detail in
  [reference/write-verification.md](reference/write-verification.md).

## Phase 1 — Core

Structure per [D18](decisions.md): single crate, **no `lib.rs`**; `src/main.rs` (tokio entry,
exit-code mapping), `src/cli.rs` (root parser), `src/commands/<noun>.rs` (that noun's subcommands
+ handlers), `src/api/`, `src/model/`, `src/config.rs`, `src/error.rs`, `src/output/`.

- `Cargo.toml` — `controld-cli` / bin `cdctl`, edition 2024 (the latest), MSRV 1.85 (minimum
  supported Rust version — the oldest compiler we promise will build it, and the first with
  edition-2024 support; development uses latest stable), `license = "MIT OR Apache-2.0"`
  (LICENSE-MIT + LICENSE-APACHE are in the repo root).
- `api/envelope.rs` — `body` as `Value`; per-op unwrap: **keyed**, **flat** (`/users`, `/ip`),
  **keyed+siblings** (`/network`). `body` flips to `[]` on error.
- `api/client.rs` — reqwest async, rustls, form, bearer; timeouts 30 s total / 10 s connect,
  `--timeout` overrides (D12).
- `main.rs` returns `ExitCode` (stdout flushes) and handles SIGINT explicitly to exit `130` —
  Rust does not produce either for free.
- `error.rs` — **classify on the `error.code` prefix**, never HTTP status. Handle **empty/non-JSON
  bodies** (a 500 can send `content-type: application/json` with 0 bytes). D4 envelope types
  including the `details` variants (`multi_target`, `collisions`, `unconvergeable`) and the exact
  `upstream` shape (`null` or `{code, http_status, message}`, members nullable) — fixed here,
  before the public schema ships.
- `config.rs` — XDG, `0600`, `secrecy`, **context-keyed** (D15). **Lazy auth** (D7).
- `output/` — table + JSON. stdout data-only.
- Retry: client built with `retry::never()`; **cdctl-owned GET-only loop** — `Retry-After`
  (integer + HTTP-date), full-jitter backoff, 3 attempts / 30 s caps, `--no-retry` (D12).

**Gate:**

- `completions`/`reference` work with **no token**.
- Every envelope shape (keyed, flat, keyed+siblings, `[]` on error *and* success, filters
  map<->`[]`, 0-byte JSON) deserializes from fixtures.
- The [error-codes.md](reference/error-codes.md) table classifies green, defensive-parsing rows
  included.
- The multi-line PHP dump, a CRLF message, and an ANSI-escape message each render as **one clean**
  `error:` line with verbatim text preserved in JSON, and `--debug` shows them JSON-escaped,
  never raw — snapshot all three modes.
- A client-side error carries `upstream: null`; the 0-byte 500 carries
  `{code: null, http_status: 500, message: null}`.
- Under wiremock 429/500/timeout, **write endpoints receive exactly one request**.
- Ctrl-C during an in-flight request exits `130`; a stalled response trips the request timeout.

## Phase 2 — `cdctl api`

The first command surface — the escape hatch (D9) makes every later deferral safe.

**Gate:**

- Absolute/scheme-relative URLs and userinfo rejected.
- A stored token is never sent to an overridden base URL without the unsafe switch.
- Non-GET without `-X` + `--yes` exits 7 (D9).
- Encoding proven under wiremock: `-F` form arrays keep **literal bracketed keys**; `--input -`
  reaches the wire verbatim as JSON; `-F` + `--input` conflict exits `2`; a body form on GET
  (`-F` **or** `--input -`) exits `2`; `DELETE /access` carries its body.

## Phase 3 — Rules & folders — **ships v0.1**

`profile list/get`, `rule list/create/update/delete`, `folder list/create/update/delete`

Plus the release machinery, because v0.1 is the first public artifact:

- README leading with the `ctrld` disambiguation.
- `AGENTS.md` — the [agents.md](https://agents.md) convention: repo-root instructions any coding
  agent reads.
- `insta` snapshots, man pages, completions.
- `cargo-dist` (linux gnu/musl, macOS arm64/x64, Windows) + Homebrew tap; `release-plz` ->
  crates.io. First-release traps recorded in [D14](decisions.md).
- CI re-runs `scripts/fetch-spec.sh` on a schedule, then
  `git diff --exit-code docs/reference/controld-openapi.json` — the fetch itself always exits 0;
  **only the diff step detects drift**.
- CI runs `cargo +1.85 check` — the MSRV claim is untested until a job enforces it.

Behavior:

- `rule list` **omits** the folder segment (`folder_id=0` 404s).
- `profile get` is a **client-side filter** — no `GET /profiles/{id}` exists.
- Shared action flags (D10). Tiered confirmation (D8). **Never auto-retry a write.**
- `-n/--dry-run` on every typed remote mutation — resolve, validate, print the normalized plan,
  persist nothing ([commands.md](commands.md#dry-run)).
- [Input hardening](commands.md#input-hardening): reject control characters everywhere and
  `?`/`#`/`%` in path-bound identifiers — exit `2` before any request.
- Multi-target cap ([D11](decisions.md)): **500 hostnames** on `rule create/update`; per-target
  outcomes (read-back for creates/updates, one request per target for `rule delete`); partial
  failure per the D4 `details` contract.
- Percent-encode hostnames into DELETE paths (`*` -> `%2A`).
- Form bodies are hand-built with literal bracket keys — the encoding live verification proved
  ([Open item 9](decisions.md#open), resolved). No probe owed.

**Gate — global, inherited by every later group's commands:**

- Fixture-tested table + JSON schema (stable key set, `null` for absent optionals).
- stdout empty on every error path.
- Table output escapes C0/DEL in server-supplied strings — snapshot a resource name carrying an
  ANSI escape.
- Write output matches its declared source (R / read-back); multi-target writes always print
  arrays.
- Under wiremock, `--dry-run` sends **zero non-GET requests**, leaves filesystem and config
  untouched, matches the published plan schema, exits `0` on a valid plan, and still fails with
  the normal code on a bad `--via` (the redirect destination —
  [shared flags](commands.md#the-shared-action-flags)).

**Gate — group-specific:**

- Request-body tests for the rule and folder rows of the shared-flag matrix
  ([commands.md](commands.md#the-shared-action-flags)) — rename-only updates send no `status`,
  action-only updates send no `via` remnant, and body assertions prove **scalar params precede
  the repeated `hostnames[]` pairs**.
- A hostname carrying `?`, `%`, or a control character exits `2` with no request sent, while
  `*.x.com` stays legal.
- A simulated silent truncation (200, read-back missing one hostname) **never reports success**
  and the re-run command excludes landed hostnames.
- Partial-failure aggregation: partial success + 429 exits `8` **only** when every failed target
  is retryable; mixed and terminal batches exit terminal; stdout empty throughout. **Complete
  stderr envelope** snapshots cover homogeneous-retryable, homogeneous-terminal, and mixed cases
  (`retry_argv` as an argv array of **resolved ids** — a config-default change between failure
  and retry must not redirect it, colliding folder names must not break it, and a mid-batch
  delete failure retries non-interactively).
- Plan snapshots cover this group's families (rule and folder creation intents, incl. the
  action-less folder; patches: rename-only, status-only, action-only, move-to-folder,
  move-to-root), proving omitted fields **never appear** in `changes` and server-generated fields
  never appear in creation intent.
- `via6` on rules: set and preserve-on-omission verified; the clear request (`--via6=`) exits `2`
  with the targeted remedy and sends nothing; `--via6` on `folder create/update` exits `2`
  (shared syntax != shared capability).
- A spoof folder flipped to block carries **no stale `via`** (`write_folder_update.json`).
- A multi-rule `rule update` whose targets start with different actions and folders converges
  each one.
- Ambiguous name resolution exits `2`; no match exits `3`.

## Phase 4 — `rule import` + `rule restore` — **ships v0.2**

The feature that justifies the project — two blocklist-sync tools exist because it doesn't.
Full semantics in [commands.md](commands.md#rule-import-semantics): **folder-scoped**
diff -> converge -> add over one **profile-wide** fetch — quota against the **10,000 rules/profile
cap**, cross-folder collisions fail fast (exit `6`, nothing written) — chunks of **500** with
scalar params first, **full desired-state verification** after every chunk plus a final full-scope
pass (the silent ~1001-var truncation makes 200 meaningless alone), idempotent re-run. :warning: `--replace` is
**add-first, delete-last** — a failure leaves a superset, never a protection gap; delete-first
needs `--force-delete-first` and writes a versioned JSON restore manifest (`rule restore` replays
it). One request per deletion (no bulk delete). HaGeZi-scale lists (~100k) **cannot fit** — point
users at Control D's native filters.

**Parked decision:** import is the third consumer of hostname canonicalization (`rule
create`/`update`/`delete` argv are the other two), which is when a `CanonicalHostname` newtype
(distinguishing form-safe vs path-safe validation state) earns its keep — introduce it here rather
than threading another bare `String` through a third call site.

**Gate — adversarial tests:**

- Out-of-scope rules untouched; mismatched existing rules converged.
- Kill mid-chunk, then re-run reaches the declared state.
- `rule restore` round-trips action, state, `via`, `via6`, and folder **with duplicate folder
  names present**.
- Parser: `rule restore` takes no `--action` and no positional file.
- Quota accounting stays correct when most rules live **outside** the import folder.
- The final full-scope verification catches a concurrent mid-import mutation of an earlier chunk;
  a `via6` mismatch fails verification.
- Cross-folder collisions (root->folder **and** folder->folder) exit `6` with the full collision
  list in `details` and **nothing written**, even when valid additions precede the collision in
  the file.
- Restore **converges** a hostname present with the wrong action, state, `via6`, or folder —
  skipping only a full-tuple match.
- A desired `via6: null` against a live set `via_v6` refuses fast — full stderr envelope
  snapshotted (`rule.unconvergeable`, exit `6`, nothing written) for **homogeneous and
  heterogeneous** live-`via6` sets, and only the homogeneous case's hint offers a single `--via6`.
- A `--force-delete-first` dry run creates **no manifest**.
- Dry-run plan snapshots cover root import, folder import, heterogeneous restore, add-first
  replace, and delete-first replace — `operation` discriminates, and `quota.peak`/`final` differ
  between the two replace modes.

## Phase 5 — Protection — **ships v0.3**

`profile create/update/delete`, `profile option list/set`, `profile default get/set`,
`filter list/enable/disable/set`, `service list/set/categories/catalog`

- Live probes owed **before the gate**: the `dropdown` option write and the level-less filter
  write (Open items 5 and 7) — captured as sanitized fixtures.

**Gate** *(v0.1 global gates inherited)*:

- `--yes` alone on `profile delete` (a `--confirm=<name>` op) **always exits `7`** — the flag
  tables and the confirmation matrix proven to agree.
- `profile option set` covers toggle, field, dropdown, and an unknown future type.
- The two owed live probes are captured and drive the `dropdown` and level-less filter tests
  (single **and** batch).
- Service `via6`: set and preserve-on-omission verified; `--via6=` exits `2`; `service set --via6`
  round-trips through the GET fixture (`p_service_v6.json`).
- `profile default set --via6` exits `2` (shared syntax != shared capability).
- Creation-intent snapshot for `profile create` incl. `--clone`.

## Phase 6 — Fleet & account — **ships v0.4**

`device list/get/create/update/delete/types`, `access list/add/remove`, `proxy list`,
`analytics levels/regions`, `account get`, `billing products/subscriptions` *(payments deferred
— no verifiable schema, D2)*, `network`, `ip`

- `device get` is a **client-side filter** — no `GET /devices/{id}` exists.
- Multi-target cap ([D11](decisions.md)): **50 IPs** on `access add/remove`.

**Gate** *(v0.1 global gates inherited)*:

- Device `--status` covers all four lifecycle values; `--analytics` covers `none|some|full` and
  rejects integers.
- `access add` caps at 50; read-back asserts every added IP inside the 50-entry window.
- `access remove` issues **one request per IP** and reports a mixed valid/invalid batch
  per-target.
- `--yes` alone on `device delete` exits `7` (`--confirm=<name>`).
- Creation-intent snapshot for `device create`.

## 1.0

The full mapped surface: 40/46 operations — org ([D15](decisions.md)) and `billing payments`
([D2](decisions.md)) stay deferred, reachable via `cdctl api`. 1.0 declares the contracts, frozen
since v0.1, semver-guaranteed.

## Testing

| Layer | Tool | What |
| --- | --- | --- |
| Envelope/models | `serde` on fixtures | Every shape hazard, straight from **real captured payloads** — no HTTP server needed |
| Client behavior | `wiremock` | Retry (GET-only, exactly-once writes), origin rules, the 0-byte JSON 500, headers |
| Command contract | `assert_cmd` | Exit codes; stdout clean on error |
| Snapshots | `insta` | `--help`, JSON shapes |
| Live | opt-in suite | Runs only when `CONTROLD_LIVE_TESTS=1` (plus `CONTROLD_API_TOKEN`); isolated to a randomized profile (below) |

Exit codes and the retryable set are **public API**. Test them like it.

Model types deserialize leniently in the binary; fixture tests use `deny_unknown_fields`, so an
API field addition fails tests instead of passing silently — the drift tripwire for an unversioned
API.

### Live-test isolation

Profiles are the isolation boundary — rules, folders, the default rule, filters, services, and
options are all profile-scoped, so a test run that stays inside its own profile is safe on **any**
account, not just a throwaway.

- **Setup:** create `cdctl-test-<timestamp>-<nonce>`; every profile-scoped mutation happens inside
  it. Existing profiles are never read or written. Each run gets a fresh 10,000-rule quota.
- **Teardown:** delete the profile; its contents go with it. Devices pointed at the test profile are
  deleted **before** the profile — deleting a profile out from under a device is unverified.
- **Leaks:** the prefix makes crashed-run leftovers identifiable, and the timestamp lets a sweep
  delete stale `cdctl-test-*` profiles safely. Sweep on suite start — plans cap profile counts, so
  accumulated leaks eventually fail setup itself.
- **Limits:** account-scoped surfaces (devices, access, proxy, account, billing, network) cannot be
  profile-isolated; those tests (v0.4) instead use the same `cdctl-test-` naming on the resources
  they create.

**Not in 1.0:** org commands ([D15](decisions.md)).

## Post-1.0 candidates

- **Machine-readable command spec** — generate a versioned schema from the same centralized metadata
  that drives clap and `cdctl reference`; implement only when a concrete consumer exists. No
  handwritten parallel contract ([D3](decisions.md)).
