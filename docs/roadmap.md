# Roadmap

What ships next, in what order, and the test gates each slice must pass. Delivery is vertically
sliced ([D17](decisions.md)): v0.1 freezes every global contract, so later slices only add
commands. Anything not yet typed is reachable via `cdctl api` today.

Current state: **v0.1 is code-complete on `main`** (core, `cdctl api`, `profile list/get`,
`rule`/`folder` CRUD, release machinery), verified by the full suite and the opt-in live suite.
Publishing it is one action — merging the open release-plz PR — which tags `v0.1.0`, publishes to
crates.io, and fires cargo-dist (binaries, GitHub Release, Homebrew formula).

| Release | Ships |
| --- | --- |
| v0.2 | `rule import` + `rule restore` |
| v0.3 | Protection: `profile` writes/options/default, `filter *`, `service *` |
| v0.4 | Fleet & account: `device *`, `access *`, `proxy`, `analytics`, `account`, `billing`, `network`, `ip` |
| 1.0 | The full mapped surface |

## First-release watch items

Provable only when the release runs:

- The Homebrew tap must receive `Formula/cdctl.rb` — an `installers`/`publish-jobs`
  desync in `dist-workspace.toml` makes the publish loop no-op green while the tap silently never
  updates.
- `RELEASE_PLZ_TOKEN` and `HOMEBREW_TAP_TOKEN` scopes prove out only at release time.
- Until the first tag exists, release-plz bases its PR branch on the last release-machinery
  commit, whose tree still carries the invalid pre-move `.github/workflows/build-setup.yml` —
  every rotation therefore mints one phantom failed workflow run (notification noise, nothing
  more). The first release moves the base past it permanently.

## v0.2 — `rule import` + `rule restore`

The feature that justifies the project — two blocklist-sync tools exist because it doesn't.
Full semantics in [commands.md](commands.md#rule-import-semantics): **folder-scoped**
diff -> converge -> add over one **profile-wide** fetch — quota against the **10,000 rules/profile
cap**, cross-folder collisions fail fast (exit `6`, nothing written) — chunks of **500** with
scalar params first, **full desired-state verification** after every chunk plus a final full-scope
pass (the silent ~1001-var truncation makes 200 meaningless alone), idempotent re-run. :warning:
`--replace` is **add-first, delete-last** — a failure leaves a superset, never a protection gap;
delete-first needs `--force-delete-first` and writes a versioned JSON restore manifest
(`rule restore` replays it). One request per deletion (no bulk delete). Missing hostnames are
created via `POST` — `PUT /rules` rejects targets with no existing rule
([write-verification](reference/write-verification.md)). HaGeZi-scale lists (~100k) **cannot
fit** — point users at Control D's native filters.

**Parked decisions for this slice:**

- Import is the third consumer of hostname canonicalization (`rule create`/`update`/`delete` argv
  are the other two), which is when a `CanonicalHostname` newtype (distinguishing form-safe vs
  path-safe validation state) earns its keep — introduce it here rather than threading another
  bare `String` through a third call site.
- `resolve_against_known` (rule.rs), `find_profile`, and `find_folder` (scope.rs) are three
  hand-rolled copies of the same exact/unique-fold/ambiguous/missing resolution shape — a shared
  resolver in `scope.rs` would collapse them; natural to do when import adds resolution pressure.
- A `--install` flag for `completions` (write to the conventional per-shell path directly) —
  additive, deferred from the v0.1 UX audit.
- `rule create` does not warn when a case variant of the hostname already exists as a distinct
  rule (case variants coexist server-side; a warning needs a pre-write fetch `create` does not
  do today).

**Gate — adversarial tests:**

- Out-of-scope rules untouched; mismatched existing rules converged.
- Kill mid-chunk, then the re-run reaches the declared state.
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

## v0.3 — Protection

`profile create/update/delete`, `profile option list/set`, `profile default get/set`,
`filter list/enable/disable/set`, `service list/set/categories/catalog`

- Live probes owed **before the gate**: the `dropdown` option write and the level-less filter
  write (Open items 5 and 7, [decisions.md](decisions.md)) — captured as sanitized fixtures.

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

## v0.4 — Fleet & account

`device list/get/create/update/delete/types`, `access list/add/remove`, `proxy list`,
`analytics levels/regions`, `account get`, `billing products/subscriptions` *(payments deferred
— no verifiable schema, [D2](decisions.md))*, `network`, `ip`

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

## Post-1.0 candidates

- **Machine-readable command spec** — generate a versioned schema from the same centralized
  metadata that drives clap and `cdctl reference`; implement only when a concrete consumer
  exists. No handwritten parallel contract ([D3](decisions.md)).
