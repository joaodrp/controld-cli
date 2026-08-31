# Roadmap

What ships next, in what order, and the test gates each slice must pass. Delivery is vertically
sliced ([D17](decisions.md#d17--ship-in-vertical-slices-not-all-40-operations-at-once)): v0.1 freezes every global contract, so later slices only add
commands. Anything not yet typed is reachable via `cdctl api` today.

Current state: **v0.1 shipped** (core, `cdctl api`, `profile list/get`, `rule`/`folder` CRUD,
release machinery). Each later slice is one release-plz PR: merging it tags, publishes to
crates.io, and fires cargo-dist (binaries, GitHub Release, Homebrew formula). A slice is a
feature, so it bumps the minor version (`features_always_increment_minor` in
`release-plz.toml`); patch releases carry fixes only.

| Release | Ships |
| --- | --- |
| v0.2 | `device list/get` |
| v0.3 | `device update` |
| v0.4 | Rule comments (`--comment` on `rule create/update`) |
| v0.5 | `rule import` + `rule restore` |
| v0.6 | Protection: `profile` writes/options/default, `filter *`, `service *` |
| v0.7 | Fleet & account: `device create/delete/types`, `access *`, `proxy`, `analytics`, `account`, `billing`, `network`, `ip` |
| 1.0 | The full mapped surface |

## v0.2 — `device list/get`

The read half of the fleet group, pulled ahead on its own: consumers were reaching it through
`cdctl api /devices` with no typed contract. Read-only, no write-path hazards. Contract in
[commands](commands.md#device-api-endpoints).

**Gate** *(v0.1 global gates inherited)*:

- All four `status` values and all three `analytics` levels render by name; an unknown code is
  exit `8`, never a passthrough int.
- A `stats`-less device reports `analytics: null`, distinct from `"none"`.
- A resolver family the API omits is `[]` in JSON.
- `PK != device_id` exits `8` with nothing on stdout.
- Name resolution: id wins, unique case-insensitive name matches, ambiguity exits `2` naming the
  candidate ids, no match exits `3` (`device.not_found`).
- `--fields` typos and control characters in the selector exit `2` before any request.

## v0.3 — `device update`

The fleet write consumers asked for, pulled ahead like the reads. All owed live probes ran
before implementation (2026-08-19, [write-verification](reference/write-verification.md)):
`icon` accepted on PUT, `status` writable on a `pending` device, `profile_id2` `-1` removal,
and the PUT echo carrying stale state — hence the mandatory `GET /devices` read-back.
Contract in [commands](commands.md#device-api-endpoints).

**Gate** *(v0.1 global gates inherited)*:

- The PUT echo is never the verdict: a stale echo with a converged read-back succeeds; an
  acked write whose read-back mismatches exits `8` (`device.unverified`) with nothing on stdout.
- A retryable write error followed by a converged read-back succeeds with an info line.
- `--enforce` resolves a profile name; the global `--profile` flag on `device update` exits `2`
  before any request (with or without `--enforce`); `CONTROLD_PROFILE` alone stays inert.
- A 33-character `--name` exits `2` before any request (the API caps names at 32).
- No-change invocation exits `2` before any request.
- `--status pending` and integer `--analytics` values are parse errors (exit `2`).
- Dry run sends nothing and prints the resolved intent (profile as `{id, name}`).
- Live: a fresh `cdctl-test-*` device runs the full update lifecycle and is torn down.

## v0.4 — Rule comments

Upstream added an optional `comment` (max 64 chars) to custom rules; `spec-drift.yml` caught it
on 2026-08-31 and the read/echo semantics were probed the same day
([write-verification](reference/write-verification.md#rule-comment-top-level-on-reads-empty-clears-64-rejects-whole-chunk-probed-2026-08-31)). A small slice pulled ahead of import so the
flag exists before manifests do. Contract in [commands](commands.md#rule).

**Gate** *(v0.1 global gates inherited)*:

- `--comment` rides `rule create`/`rule update`; the read-back verifies it like every other field.
- `--comment=` on `update` clears and verifies against an absent read-back comment; on `create`
  it exits `2`. Omission preserves (the `PUT /rules` merge).
- An over-64-byte or whitespace-padded comment exits `2` before any request (fail-fast; the
  server 400s the former and silently trims the latter).
- `comment` lands in the JSON schema (`null` when absent) and the table (`COMMENT` column).
- Live: the smoke lifecycle sets, preserves-through-merge, and clears a comment.

## v0.5 — `rule import` + `rule restore`

The feature that justifies the project: two blocklist-sync tools exist because it doesn't.
Full semantics in [commands](commands.md#rule-import-semantics-v05): **folder-scoped**
diff -> converge -> add over one **profile-wide** fetch (quota against the **10,000 rules/profile
cap**), cross-folder collisions fail fast (exit `6`, nothing written), chunks of **500** with
scalar params first, **full desired-state verification** after every chunk plus a final full-scope
pass (the silent ~1001-var truncation makes 200 meaningless alone), idempotent re-run. ⚠️
`--replace` is **add-first, delete-last**: a failure leaves a superset, never a protection gap.
Delete-first needs `--force-delete-first` and writes a versioned JSON restore manifest
(`rule restore` replays it). One request per deletion (no bulk delete). Missing hostnames are
created via `POST`, since `PUT /rules` rejects targets with no existing rule
([write-verification](reference/write-verification.md#put-rules-does-not-upsert-via-case-is-preserved--reject-at-create-probed-2026-07-18)). HaGeZi-scale lists (~100k) **cannot
fit**. Point users at Control D's native filters.

**Parked decisions for this slice:**

- Import is the third consumer of hostname canonicalization (`rule create`/`update`/`delete` argv
  are the other two), which is when a `CanonicalHostname` newtype (distinguishing form-safe vs
  path-safe validation state) earns its keep: introduce it here rather than threading another
  bare `String` through a third call site.
- `resolve_against_known` (rule.rs), `find_profile`, and `find_folder` (scope.rs) are three
  hand-rolled copies of the same exact/unique-fold/ambiguous/missing resolution shape. A shared
  resolver in `scope.rs` would collapse them, natural to do when import adds resolution pressure.
- A `--install` flag for `completions` (write to the conventional per-shell path directly),
  additive, deferred from the v0.1 UX audit.
- `rule create` does not warn when a case variant of the hostname already exists as a distinct
  rule (case variants coexist server-side, and a warning needs a pre-write fetch `create` does not
  do today).

**Gate (adversarial tests):**

- Out-of-scope rules untouched. Mismatched existing rules converged.
- Kill mid-chunk, then the re-run reaches the declared state.
- `rule restore` round-trips action, state, `via`, `via6`, and folder **with duplicate folder
  names present**.
- Parser: `rule restore` takes no `--action` and no positional file.
- Quota accounting stays correct when most rules live **outside** the import folder.
- The final full-scope verification catches a concurrent mid-import mutation of an earlier chunk.
  A `via6` mismatch fails verification.
- Cross-folder collisions (root->folder **and** folder->folder) exit `6` with the full collision
  list in `details` and **nothing written**, even when valid additions precede the collision in
  the file.
- Restore **converges** a hostname present with the wrong action, state, `via6`, or folder,
  skipping only a full-tuple match.
- A desired `via6: null` against a live set `via_v6` refuses fast: full stderr envelope
  snapshotted (`rule.unconvergeable`, exit `6`, nothing written) for **homogeneous and
  heterogeneous** live-`via6` sets, and only the homogeneous case's hint offers a single `--via6`.
- A `--force-delete-first` dry run creates **no manifest**.
- Dry-run plan snapshots cover root import, folder import, heterogeneous restore, add-first
  replace, and delete-first replace: `operation` discriminates, and `quota.peak`/`final` differ
  between the two replace modes.

## v0.6 — Protection

`profile create/update/delete`, `profile option list/set`, `profile default get/set`,
`filter list/enable/disable/set`, `service list/set/categories/catalog`

- Live probes owed **before the gate**: the `dropdown` option write and the level-less filter
  write (Open items 5 and 7, [decisions](decisions.md#open)), captured as sanitized fixtures.

**Gate** *(v0.1 global gates inherited)*:

- `--yes` alone on `profile delete` (a `--confirm=<name>` op) **always exits `7`**: the flag
  tables and the confirmation matrix proven to agree.
- `profile option set` covers toggle, field, dropdown, and an unknown future type.
- The two owed live probes are captured and drive the `dropdown` and level-less filter tests
  (single **and** batch).
- Service `via6`: set and preserve-on-omission verified. `--via6=` exits `2`. `service set --via6`
  round-trips through the GET fixture (`p_service_v6.json`).
- `profile default set --via6` exits `2` (shared syntax != shared capability).
- Creation-intent snapshot for `profile create` incl. `--clone`.

## v0.7 — Fleet & account

`device create/delete/types`, `access list/add/remove`, `proxy list`,
`analytics levels/regions`, `account get`, `billing products/subscriptions` *(payments deferred,
no verifiable schema, [D2](decisions.md#d2--the-cli-is-the-stability-layer-own-the-output-schema))*, `network`, `ip`

- Multi-target cap ([D11](decisions.md#d11--form-encoded-hostnames-resolved-live)): **50 IPs** on `access add/remove`.

**Gate** *(v0.1 global gates inherited)*:

- Device `--status` covers all four lifecycle values. `--analytics` covers `none|some|full` and
  rejects integers.
- `access add` caps at 50. Read-back asserts every added IP inside the 50-entry window.
- `access remove` issues **one request per IP** and reports a mixed valid/invalid batch
  per-target.
- `--yes` alone on `device delete` exits `7` (`--confirm=<name>`).
- Creation-intent snapshot for `device create`.

## 1.0

The full mapped surface: 40/46 operations, org ([D15](decisions.md#d15--personal-accounts-only-orgs-addable-without-breaking-changes)) and `billing payments`
([D2](decisions.md#d2--the-cli-is-the-stability-layer-own-the-output-schema)) stay deferred, reachable via `cdctl api`. 1.0 declares the contracts, frozen
since v0.1, semver-guaranteed.

## Post-1.0 candidates

- **PTY test harness**: the three TTY-gated behaviors (hidden token prompt, slow-request
  notice, interactive confirmation) are manual-test only. A `portable-pty` dev-dependency
  would let one integration test pin the highest-risk property: a slow write still sends
  exactly one request while the notice fires (the notice wrapper re-awaits the same future,
  and only a test can keep it that way).
- **Machine-readable command spec**: generate a versioned schema from the same centralized
  metadata that drives clap and `cdctl reference`. Implement only when a concrete consumer
  exists. No handwritten parallel contract ([D3](decisions.md#d3--no-tty-based-format-switching)).
- **Endpoint schedules**: `/endpointschedules` (full CRUD) is live but undocumented, so it stays
  behind `cdctl api` ([D16](decisions.md#d16--documented-surface-only)). Implement only if Control D
  documents it — `spec-drift.yml` goes red the week that happens. Shape and traps in
  [`reference/api-contract.md`](reference/api-contract.md#endpoint-schedules-undocumented).
