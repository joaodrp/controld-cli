# Command Specification

Per-command contract: flags, human table columns, and normalized JSON fields.
Written before implementation so the commands are consistent by construction, not by luck.

**Conventions used below**
- JSON field names are **ours**, not the API's ([D2](decisions.md)). `PK` never appears in output.
- `do`/`status` integers never appear in output or input ([D10](decisions.md)).
- Timestamps: API sends Unix seconds; we emit **RFC-3339** in JSON, and relative ("3 days ago") in tables.
- Every command accepts the [global flags](design.md#global-flags).
- Every typed **remote-mutation** command accepts `-n, --dry-run` — semantics defined once in
  [Dry run](#dry-run). All argument values pass [input hardening](#input-hardening) before any
  request is built.
- Table columns are the *default human view*; `--json` returns the full object; `--fields a,b`
  projects it.
- **Stable key set.** Fields the API sends optionally are always present in our JSON, `null` when
  absent. Consumers never branch on key existence. Scope: **resource, error, and creation-intent
  schemas** (in a creation intent, `null` means the field will not be sent — a create has nothing
  to preserve, so no ambiguity). Patch
  maps (`changes` in [dry-run](#dry-run) plans) are sparse by definition — presence means the
  field will be sent, and `null` inside one is an explicit clear (`folder_id: null` = move to
  root; `via6: null` is **not representable** — the API has no clear, see [rule](#rule));
  conflating the two would make merge updates unrepresentable.
- Empty result: `[]`, exit `0`. Lists keep API order; `rule list` sorts by `order`.
- **Write output.** `create`/`update` print the normalized resource; `delete` prints nothing.
  Upstream's `message` goes to stderr as an info line. Identical in `--json`. Where the resource
  comes from is fixed per write — see [Write output sources](#write-output-sources).
- **Errors** are a JSON envelope on stderr in JSON mode, single-line `error: ...` text otherwise —
  multi-line upstream text **collapses to one line** in human mode; the JSON envelope keeps it
  verbatim. Exit codes are identical in both modes.

---

## The shared action flags

Reused by `rule`, `folder`, `service`, and `profile default`. Defined once, here.

| Flag | Values | Maps to |
| --- | --- | --- |
| `--action` | `block` \| `bypass` \| `spoof` \| `redirect` | `do` = 0 \| 1 \| 2 \| 3 |
| `--via` | IP / CNAME (spoof), proxy PK (redirect) | `via` |
| `--via6` | IPv6, spoof only | `via_v6` |
| `--enabled` / `--disabled` | | `status` = 1 / 0 |

**All four parse as optional — no clap-level defaults.** A parser default would silently send
`status=1` on every update (`folder update 2 --name New` must not re-enable a disabled folder).
Defaults are applied per command, after dispatch:

| Command | `--action` | `--enabled/--disabled` | Omitted flags |
| --- | --- | --- | --- |
| `rule create` / `rule import` | **required** | default enabled | `via`/`group` omitted from the wire |
| `rule update` | optional | optional | **not sent** — `PUT /rules` merges (verified) |
| `folder create` | optional — omit `do` for an action-less folder | default enabled | `do` omitted from the wire |
| `folder update` | optional | optional | not sent |
| `service set` | **required** | default enabled | `do`/`status`/`via` always sent; **`via_v6` merges** — omitted = preserved (verified) |
| `profile default set` | **required** | default enabled | — (spec: both required) |

Rejected combinations: `--via` without `--action spoof|redirect` (including on updates — the
action context is needed to validate it); `--via6` without `--action spoof`.

**Shared syntax != shared capability.** `--via6` is accepted by **rules and services only** —
service `via_v6` write *and* read-back verified live
([write-verification](reference/write-verification.md)). Folders and the profile default have no
documented `via_v6` field, and [D16](decisions.md) forbids building on undocumented ones:
`--via6` on `folder create/update` or `profile default set` is exit `2`.

---

## Write output sources

"Print the normalized resource" needs a defined source — several responses cannot support it alone.
Sources: **R** (the response) or **RB** (an authoritative read-back). No write prints
request-derived data as if the server confirmed it — several responses are summaries that cannot
prove what landed. Multi-target commands always print an **array**, one element per target,
whatever the count — cardinality never changes the shape.

| Write | Source | Why |
| --- | --- | --- |
| `profile create` | R | full object in `body.profiles[]` |
| `profile update` | RB | the response can echo stale `disable_ttl` |
| `profile option set` | RB | no verified write response |
| `profile default set` | R | returns `body.default` |
| `rule create/update/import/restore` | RB | the response is a hostname-less one-entry summary; the mandatory verification read-back ([rule](#rule)) is authoritative and already paid for |
| `folder create` | R | full folder object |
| `folder update` | R | full folder object in the response (verified live — the spec's empty schema is wrong) |
| `filter enable/disable/set` | RB | the response is only a family-keyed `{do,status,lvl}` map (`[]` when empty) — no titles, levels, or descriptions; re-fetch `filter list` and print the affected families |
| `service set` | RB | neither response nor request carries `category`/`locations`/`warning`; `GET .../services` does |
| `device create/update` | R | device object flat at `body` |
| `access add` | RB | the ack is `body: []` plus a count message; the read-back window is the latest 50 IPs — hence the 50-IP cap ([access](#access--proxy)) |

**Validation, client-side, before the request:**
- `--via` **required** when `--action spoof` or `--action redirect`; **rejected** otherwise.
- With `--action redirect`, `--via` must be a known proxy PK — validate against `GET /proxies`
  and, on failure, print the nearest matches. (`LON` is not a proxy; `LHR` is.)
- With `--action spoof`, `--via` must parse as an IP or a hostname.
- `--enabled/--disabled` is **orthogonal** to `--action`. `--action block --disabled` is legal: a
  *disabled block rule*.

---

## Dry run

Typed **remote-mutation** commands accept `-n, --dry-run` (the clig.dev standard flag): every
Phase-4 write plus `rule import` and `rule restore`. Local-state commands (`auth login/logout`,
`config set`) do **not** take it, and `cdctl api` is excluded — the escape hatch is gated by
[D9](decisions.md) (`-X` + `--yes`) instead.

- **Resolve and validate everything; persist nothing.** Names resolve to ids, `--via` is checked
  against `GET /proxies`, option values against their type, import quota and cross-folder
  collisions against the fetched rule set. Read-only API calls are allowed. Forbidden: any
  mutating request, any config or credential change, **any file creation** — a
  `--force-delete-first` dry run prints the manifest path it *would* write and does not create it.
- **Output is data on stdout**, schema-stable like every other JSON output (semver-governed,
  [D2](decisions.md)). The plan is **normalized intent, not wire bytes**: action names and
  booleans per D2/D10 — integers never appear here either. Wire encoding (form ordering, bracket
  keys, scalar-params-first) is [D11](decisions.md)'s contract, proven by Phase-4 request-body
  tests and visible under `--debug` — the plan does not restate it. `requests` lists the
  **withheld mutations**, in send order; validation GETs execute normally and are not listed:

  ```json
  {
    "requests": [
      {
        "method": "POST",
        "path": "/profiles/pr1/rules",
        "intent": {
          "hostnames": ["a.com", "b.com"],
          "action": "block",
          "via": null,
          "via6": null,
          "enabled": true,
          "folder_id": null
        }
      }
    ]
  }
  ```

  Creates plan the normalized **creation intent**: exactly the fields the CLI will send, plus
  CLI-owned defaults (e.g. `enabled: true` where a command defaults it) — **never server-generated
  fields** (ids, `order`, timestamps, resolver addresses, clone results), which do not exist until
  the write happens and which normal write output is forbidden to fabricate. Each family's key set
  is fixed and part of the contract; `null` means the field will not be sent (a create has nothing
  to preserve, so `null` is unambiguous here, unlike in patches):

  | Create | Intent keys |
  | --- | --- |
  | `profile create` | `{name, clone}` — `clone` = the resolved source profile **id**, else `null` |
  | `rule create` | `{hostnames, action, via, via6, enabled, folder_id}` — `enabled: true` is the CLI-owned default |
  | `folder create` | `{name, action, via, enabled}` — action-less folder => `action: null` |
  | `device create` | `{name, profile_id, type, clients, analytics}` — `profile_id` resolved, flat |
  | `access add` | `{device_id, ips}` |

  `service set`, `profile default set`, `profile option set`, and `filter enable/disable/set`
  send complete state — with one verified exception: service `via_v6` **merges** upstream, so it
  appears in the intent only when `--via6` was given ([service](#service)). Their plans are full
  intents, never patches:
  `{service, action, via, via6, enabled}`, `{action, via, enabled}`, `{name, enabled, value}`,
  `{levels: {"<level-or-family>": true|false}}`. Merge-style updates
  (`rule update`, `folder update`, `device update`, `profile update`, ...) plan a **patch** instead:
  a sparse `changes` map holding exactly the fields the run would send — presence is meaningful,
  omitted fields are preserved by the server's verified merge, and `null` is an explicit clear
  (`folder_id: null` <- `--root`):

  ```json
  {
    "requests": [
      {
        "method": "PUT",
        "path": "/profiles/pr1/rules",
        "intent": {
          "hostnames": ["x.com"],
          "changes": {"enabled": false}
        }
      }
    ]
  }
  ```

  Each patch family's `changes` vocabulary is likewise contractual:

  | Patch | Legal `changes` keys |
  | --- | --- |
  | `rule update` | `action, via, via6, enabled, folder_id` — `folder_id: null` = root; `via6: null` unrepresentable ([rule](#rule)) |
  | `folder update` | `name, action, via, enabled` |
  | `profile update` | `name, disabled_until` |
  | `device update` | `name, profile_id, analytics, status` |

  `rule import` / `rule restore` print a domain plan instead, discriminated by `operation`:

  ```json
  {
    "operation": "import",
    "profile_id": "pr1",
    "folder_id": 2,
    "add": ["<rule>"],
    "converge": ["<rule>"],
    "delete": ["<rule>"],
    "quota": {
      "current": 120,
      "peak": 680,
      "final": 620,
      "cap": 10000
    },
    "manifest": null
  }
  ```

  ```json
  {
    "operation": "restore",
    "profile_id": "pr1",
    "source": "./cdctl-restore-pr1-1720713600.json",
    "add": ["<rule>"],
    "converge": ["<rule>"],
    "skipped": ["<rule>"],
    "quota": {
      "current": 120,
      "peak": 170,
      "final": 170,
      "cap": 10000
    }
  }
  ```

  Rule entries use the normalized [rule schema](#rule) and carry their own `folder_id` — restore
  is **not** folder-scoped, so no top-level `folder_id` appears on it; import's is the scope
  (`null` = root). Restore classifies manifest entries by **full-state diff**, exactly like
  import: `skipped` holds normalized rules already matching the entire desired tuple; anything
  else converges or is added. `quota.peak` is the largest concurrent total the plan can reach —
  add-first `--replace` peaks *before* its deletions, delete-first peaks lower — and **the cap
  check runs against `peak`**; `final` is the end state. `manifest` is the would-be path under
  `--force-delete-first`, else `null`. Human mode renders the same fields as text
  ([D4](decisions.md)).
- **Exit `0` on a valid plan.** Validation failures exit with their normal codes — a hallucinated
  parameter fails locally, with the same error the real run would produce, before it reaches the
  API. An import with cross-folder collisions exits `6` here exactly as the real run would.
- **Confirmation is skipped.** Nothing mutates, so no prompt, no `--yes`, no `--confirm=<name>`.

## Input hardening

Agents hallucinate inputs humans never type — embedded query fragments in ids, pre-encoded
strings, invisible characters. Reject the known shapes with exit `2` **before any request is
built**:

- **ASCII control characters** (below `0x20`, plus `0x7F`) in any argument value, plus the C1
  controls (`U+0080`-`U+009F`), matching the output-escaping stance.
- In values placed into **URL path segments** (hostnames on `rule delete`, service names, filter
  level names, option names, resolved folder/profile/device ids): additionally reject `?`, `#`,
  and `%` — embedded query fragments and pre-encoded input (a passed-in `%2A` would
  double-encode). The error hint states that values are taken literally, never pre-encoded.
  Wildcard `*` stays **legal in hostnames** — it is rule grammar, percent-encoded at the HTTP
  layer.

Percent-encoding into paths stays the load-bearing defense; rejection exists to turn a confusing
`404` (exit `3`) into a precise exit `2` with a usable hint. Form-bound values (names, `--value`)
need only the control-character check — form encoding transmits the rest safely.

`cdctl api` is exempt: its path and `-F` pairs are forwarded raw by design — the escape hatch must
not reshape what it carries. It is gated by [D9](decisions.md) instead.

---

## profile

| Command | Flags |
| --- | --- |
| `profile list` | — |
| `profile get <id\|name>` | — |
| `profile create <name>` | `--clone <PK>` |
| `profile update <id>` | `--name <s>`, `--disable-until <time>`, `--enable` |
| `profile delete <id>` | `--confirm=<name>` — `--yes` alone is never enough |
| `profile option list` | — |
| `profile option set <name>` | `--enabled/--disabled`, `--value <v>` |
| `profile default get` | — |
| `profile default set` | *action flags* |

**Table:** `NAME, ID, RULES, UPDATED` — `RULES` shows the **enabled** count, and `--help` says so
**JSON:** `{id, name, enabled_rules, enabled_filters, enabled_services, folders, options, default_action, enabled, disabled_until, updated}`

The counts come from `profile.*.count`, which tracks **enabled items only** — the list endpoints
return more (observed 16 vs 18). The `enabled_` JSON names carry that semantic; the table keeps just
`RULES` so the misleading pair stays out of the default view.

`default_action` is `profile.da`. The spec types it `[]` when unset, but live it is **always an
object** — even on a freshly created profile. Tolerate both; normalize `[]` -> the implicit default
(`bypass`, enabled).

**There is no boolean disable** — the API field is `disable_ttl`, a *deadline*: disabled until a
unix timestamp, `0` re-enables. Hence `--disable-until <time>` / `--enable`, and no disable on
create. Readback shape-shifts (`disable: null` <-> `disable_ttl: <ts>`); derive
`enabled`/`disabled_until` from it. The PUT response can echo stale state — re-read after writing.

`profile get` is a **client-side filter over `GET /profiles`** — the API has no `GET /profiles/{id}`.

**`profile option list`** — table `OPTION, TITLE, TYPE, DEFAULT`;
JSON `{name, title, description, type, default, info_url}`. `type` is an **open enum** — live values
are `toggle`, `field`, and `dropdown` (the third is absent from the API docs); render unknown types
as-is, never error. `default` is **raw JSON**: an integer for toggles/fields, an object map
(`{"0.9": "Minimal"}`) or a **bare label array** (`ecs_subnet`) for dropdowns. Never assume a shape.

**`profile option set <name> --enabled|--disabled [--value <v>]`** — the API write takes a required
`status` plus an optional `value`; a single positional value cannot express enable/disable/select
unambiguously. Validate `--value` per type against the live catalogue (field -> number, dropdown -> a
key of its map). :warning: The dropdown write is **unprobed** — verify before Phase 4. Prints the new state
`{name, value, enabled}` via read-back (no verified write response exists).

**`profile default get/set`** — one row, `ACTION, VIA, ENABLED`; JSON `{action, via, enabled}`.

---

## rule

| Command | Flags |
| --- | --- |
| `rule list` | `--folder <id\|name>` |
| `rule create <hostname>...` | *action flags*, `--folder <id\|name>` |
| `rule update <hostname>...` | *action flags*, `--folder`, `--root` |
| `rule delete <hostname>...` | `--yes` |
| `rule import <file\|->` | *action flags*, `--folder`, `--replace`, `--force-delete-first` |
| `rule restore <manifest>` | `--force` *(accept a profile mismatch)* |

**Table:** `HOSTNAME, ACTION, VIA, ENABLED, FOLDER`
**JSON:** `{hostname, action, via, via6, enabled, folder, folder_id, order}`

- `hostname` <- `PK`. May be a wildcard (`*.example.com`).
- `folder_id` <- `group`; **`0` normalizes to `null`** (a "no folder" sentinel, not folder zero).
  `folder` is the resolved display name, informational only — **folder names are not unique**;
  anything that must identify a folder exactly (manifests, scripts) uses `folder_id`.
- `rule update --root` moves rules back out of a folder — `group=0` on `PUT`, **verified live**.
  Mutually exclusive with `--folder`.
- `rule list` **omits** the folder segment. Passing `folder_id=0` 404s — see
  [read-verification section 3](reference/read-verification.md).
- **A missing `action.do` is an error, not a default.** Never coerce to `0` (= BLOCK).
- `rule update` sends only the flags given — **`PUT /rules` merges**, preserving omitted fields
  (verified live), so `rule update x.com --disabled` needs no `--action`.
- **`via6` cannot be cleared** while the spoof action persists — probed: omission preserves it,
  empty string and `0` are rejected atomically, and only an action flip clears it (an unprotected
  window `cdctl` never enters implicitly —
  [write-verification](reference/write-verification.md)). `rule update` therefore offers no
  clear-`via6` operation, and a patch of `via6: null` is unrepresentable. The one spelling that
  *requests* it — `--via6=` (the attached empty value, unambiguous across POSIX and Windows
  shells) — is rejected as an **attempted unsupported clear**: exit `2`, hint carrying the
  delete + recreate remedy, never forwarded upstream (where it 400s anyway — fixture
  `err_via6_clear.json`).
- **Multi-target cap: 500 hostnames** per `rule create`/`rule update` invocation — the import
  chunk size, safely inside the server's silent ~1001-form-var ceiling ([D11](decisions.md)).
  More is exit `2` with a hint to `rule import` (resumable, quota-aware). Whatever the count,
  **read back and verify** every target landed in the desired state before printing it — the
  write response is a one-entry summary that cannot reveal a dropped hostname.
- **Partial failure is per-target, inside the error envelope.** When a multi-target
  `create`/`update`/`delete` fails partway, or verification finds a gap, the [D4](decisions.md)
  `details` (`kind: "multi_target"`) carries ordered per-target entries and a shell-neutral
  **`retry_argv`** array covering **only the missing targets** — it must exclude already-landed
  hostnames, since a duplicate in a `POST` fails its whole chunk. Exit by D4's
  aggregation rule: `8` only when every failed target is retryable, else the shared terminal code
  (or `1` when mixed). stdout stays empty.

`rule delete` must **percent-encode the hostname into the path**, wildcards included.

### `rule import` semantics

Input: one hostname per line; `#` comments and blanks skipped; hosts-file format accepted
(`0.0.0.0 ads.example.com` -> `ads.example.com`). Hostnames are canonicalized (lowercased, trailing
dot stripped) and deduplicated within the file; a line that is not a plausible hostname **fails the
whole import with its line number** before anything is written. [`--dry-run`](#dry-run) prints the
add/converge/delete plan and exits `0`.

- **Scope.** An import targets exactly one folder: `--folder <f>`, or the root rules when omitted.
  The diff, convergence, and `--replace` deletions operate **within that scope only** — rules in
  other folders are never read into the plan, never converged, never deleted. There is no
  whole-profile replace in v1; run once per folder.
- **Quota first — and profile-wide.** Profiles cap at **10,000 rules across all folders**
  (enforced; a crossing batch fails whole). Fetch the **whole profile's rules once** (`GET /rules`,
  segment omitted — entries carry `group`, so the folder subset for the diff falls out of the same
  fetch). Compute the budget from that full set — never from `profile.rule.count` (it counts
  *enabled* rules only) and never from the folder subset: 9,900 rules in other folders are
  invisible to a scoped fetch yet count against the cap. Under `--force-delete-first`, recompute
  `peak` and `final` once the deletion set is known, before the first DELETE — the cap check runs
  against `peak` ([dry run](#dry-run) defines both). Fail fast with the shortfall; large
  blocklists belong in Control D's native filters, and the error says so.
- **Diff -> converge -> add.** The scope's subset comes from the profile-wide fetch above — rules in
  other folders inform the quota but never enter the plan. Hostnames already in the desired state
  are skipped. Hostnames present with a **different action, state, `via`, or `via6` are converged
  with `PUT`** (chunked; the merge is verified) — skipping them as "duplicates" would mean import never
  reaches the state the file declares. New hostnames are added with `POST`. A duplicate in a `POST`
  chunk atomically fails the chunk, which is why the diff is not optional.
- **Cross-folder collisions fail fast, exit `6`.** Hostnames are **profile-unique** — `DELETE`
  addresses a rule with no folder segment and `PUT` *moves* one between folders — so a file
  hostname that already lives in a **different** folder cannot converge in scope without mutating
  that other folder, and the contract promises other folders stay untouched. Refuse the whole
  import before any request: the error's [D4](decisions.md) `details` (`kind: "collisions"`)
  lists every collision as `{hostname, folder_id, folder}`, nothing is written (the profile-wide
  fetch makes this decidable up front; `--dry-run` reports the identical failure).
- **Unconvergeable states fail fast too, exit `6`.** A file/manifest rule with `via6: null` whose
  live counterpart has `via_v6` set cannot be converged — the API has no clear operation for it
  ([write-verification](reference/write-verification.md)). Decidable from the same pre-mutation
  diff: refuse with `details` (`kind: "unconvergeable"`, entries
  `{hostname, reason: "via6-clear", current_via6, desired_via6}` — the live value included, so a
  caller decides without another fetch). The **hint separates the two real choices** — they are
  not equivalent. **(a) Accept the live `via6`** — this *revises the desired state*, it clears
  nothing: `--via6 <value>` only when every conflict shares one live value (`--via6` is
  command-wide; a hostname file has no per-line fields), else separate per-value imports,
  individual `rule update`s, or `rule restore` (per-rule `via6` in the manifest). **(b)
  Destructive clear** — the only route to `via6: null`: delete + recreate, which the hint spells
  out with the resolved `--profile <id>`, the folder, and the **full** desired action, `via`, and
  state (recreating in root or with partial flags would be worse than the mismatch), plus `--yes`
  and the **protection-gap warning** (the rules are absent between the two commands). `cdctl`
  never takes route (b) implicitly. A future `--move` flag may
  opt in; an implicit cross-folder move never happens. (What a cross-folder duplicate `POST`
  would do upstream is unprobed — and irrelevant: the plan refuses first.)
- **`--replace` adds first, deletes last.** Deleting up front would leave the profile unprotected
  if anything then fails. So: converge and add (all verified), and only then delete the in-scope
  rules missing from the file — :warning: one request per deletion (no bulk delete). Deletions state the
  exact count and go through the `rule delete` confirmation tier (prompt on TTY, `--yes` in
  scripts). A failure at any point leaves a **superset** of the previous rules, never a gap;
  re-running converges. When old + new cannot fit inside the quota together, **fail with the
  shortfall** — the only recourse is `--force-delete-first`.
- **`--force-delete-first`** (only with `--replace`) accepts the unprotected window explicitly.
  Before the first DELETE it writes every doomed rule to a **restore manifest** —
  `./cdctl-restore-<profile-id>-<timestamp>.json` (the opaque id, not the profile name, which would
  need filename sanitization) — mode `0600`, refuses to overwrite, **fsynced before the first
  delete**, path printed, never auto-deleted. The manifest is versioned JSON:
  `{"version": 1, "profile_id": "...", "rules": [<normalized rule objects>]}`. Rules carry
  `folder_id`, so restore stays exact when folder names collide, and heterogeneous actions
  round-trip (a plain hostname list cannot).
- **`rule restore <manifest>`** is a **separate command** — no positional file and no `--action`
  (each rule carries its own), so it doesn't collide with the import grammar. It refuses a manifest
  whose `profile_id` differs from the target profile unless `--force` is given, and refuses unknown
  `version` values. Recreates action, state, `via`, `via6`, and folder exactly. Idempotent by
  **full-state diff, not by hostname**: a manifest entry is skipped only when the live rule
  already matches the entire desired tuple; a hostname present with any difference **converges via
  `PUT`** (the verified merge — a user may have recreated, moved, or edited it since the manifest
  was written), and missing hostnames are added. Chunked at 500 with the same per-chunk and final
  full-state verification as import.
- **Chunks of 500, scalar params first, verify the full desired state.** After each chunk,
  re-fetch and assert the chunk's hostnames are present with the intended **action, enabled state,
  `via`, `via6`, and folder** — membership alone can miss a dropped trailing scalar (a truncated
  `status=0` silently defaults to enabled). A raw 200 is meaningless (the server silently drops
  form variables past ~1001) and a total-count check is insufficient — a concurrent add elsewhere
  can mask a dropped hostname. After the last chunk, run **one final full-scope verification** so
  a concurrent mutation of an earlier chunk cannot escape detection. On mismatch, abort with a
  resumable report ([write-verification](reference/write-verification.md)).
- **On mid-import failure**: report chunks written, chunks remaining, and the exact re-runnable
  command. Re-running is safe — the diff step makes import idempotent and convergent.

---

## folder (API: "groups")

| Command | Flags |
| --- | --- |
| `folder list` | — |
| `folder create <name>` | *action flags* (optional — omitting `--action` creates an action-less folder, verified live) |
| `folder update <id\|name>` | `--name`, *action flags* |
| `folder delete <id\|name>` | `--yes` |

**Table:** `NAME, ID, RULES, ACTION, ENABLED`
**JSON:** `{id, name, action, via, enabled, rules}`

- **`id` is an integer** here, unlike every other resource. **Folder names are not unique** — resolving
  a name to multiple folders is an error, not a coin-flip. The `{folder}` **path segment must be that
  integer** — a name in the path 404s upstream (`This folder does not exist`, verified live), which is
  why name resolution is strictly client-side.
- `folder update` sends only the flags given — **`PUT /groups/{id}` merges** (verified live:
  rename-only preserves action and status, status-only preserves the rest, and an action-less folder
  survives a rename without gaining a `do`; the spec wrongly marks `do`+`status` required —
  [write-verification](reference/write-verification.md)).
- Folders may have **no action at all** (`{"action":{"status":1}}` with no `do`) — `action` is then
  `null`, which is legal and distinct from `block`.
- controld-go sleeps 2s after creation — **defensive, not evidence**: no lag was observed live, and
  the create response already returns the folder id. Don't sleep, don't poll. If lag is ever
  demonstrated, poll `GET /groups` for the **returned id** with a bounded timeout — never by name
  (names are not unique).

---

## filter

| Command | Flags |
| --- | --- |
| `filter list` | `--external` |
| `filter enable <name>` | — |
| `filter disable <name>` | — |
| `filter set <name>=<on\|off>...` | *(batch — the one JSON write)* |

**Table:** `FAMILY, TITLE, ENABLED, ACTIVE, LEVELS`
**JSON:** `{family, title, description, enabled, active_level, levels: [{name, title}], third_party}`

- One object per **family** (`family` <- `PK`). `active_level` is the enabled level's name; `null`
  when the family is disabled *or has no level system* — `enabled` is the family's own status and
  is what distinguishes those two cases.
- `levels` lists the legal selections and is **always `[]`** when the family has none — most
  families (`noai`, `dating`, ...) and every third-party filter. Arrays are never `null`; the
  stable-key `null` convention covers scalars.
- What `enable`/`disable`/`set` take: for a leveled family, a **level name** from `levels[]`
  (`ads_small`, `ads_medium`, `ads`, `porn_strict` — the suffixes follow **no derivable rule**;
  never construct names). A level-less family is addressed by its **family id** — it has no other
  name. :warning: Level-less writes are **unprobed** ([Open](decisions.md#open)); leveled writes are
  verified on both endpoints.
- Level names are valid on **both** write endpoints (verified live). Enabling a level of an
  already-enabled family **swaps atomically** — levels never stack, so `filter enable` needs no
  disable-first step.
- The write response is a **map keyed by family** (`{"porn": {do, status, lvl}}`), and **`[]` when
  zero filters remain enabled** — the object<->array flip is by emptiness
  ([write-verification](reference/write-verification.md)).
- `filter list` must make the legal level names obvious. This is the most confusing part of the API
  surface, and clearing it up is the CLI's job.
- Third-party filter PKs are **`x-`-prefixed**; set `third_party: true`.
- `PUT /profiles/{id}/filters` is the **only `application/json` write** in the API.

---

## service

| Command | Flags |
| --- | --- |
| `service list` | `--category <c>` |
| `service set <service>` | *action flags* |
| `service categories` | — |
| `service catalog <category>` | — |

**Table:** `SERVICE, NAME, CATEGORY, ACTION, ENABLED`
**JSON:** `{service, name, category, action, via, via6, enabled, unlock_location, locations, warning}`

`unlock_location` is always present. `warning` and `locations` are **optional** and absent on some entries. `service catalog` requires a
real category PK (`audio career finance gaming hosting news recreation shop social tools vendors
video`); a bad one 404s with `Invalid category` — validate client-side and suggest.

`service set --disabled` keeps the rule listed (disabled) — **the API has no way to remove a service
rule**. A bad service name is 400 `Invalid service was provided`; validate client-side and suggest.

`via_v6` on services behaves exactly like rules
([write-verification](reference/write-verification.md)): **omitted = preserved** (the PUT merges
it), `--via6=` is the rejected clear attempt (exit `2`), and only an action transition clears it.
Since services cannot be deleted, the destructive clear is the explicit two-step flip
(`service set <s> --action bypass`, then re-set spoof with `--via` only) — the hint spells it out
with its unprotected-window warning; never implicit. Read-back verification asserts `via6` only
when `--via6` was given.

**`service categories`** — table `CATEGORY, NAME, SERVICES`; JSON `{category, name, description, count}`.
**`service catalog <c>`** — table `SERVICE, NAME, UNLOCK`; JSON
`{service, name, category, unlock_location, locations, warning}` — no `action`: this is the global
catalogue, not the profile's rules.

---

## device (API: "endpoints")

| Command | Flags |
| --- | --- |
| `device list` | — |
| `device get <id\|name>` | — |
| `device create <name>` | `--profile <p>` *(required)*, `--type <t>`, `--clients <n>`, `--analytics <none\|some\|full>` |
| `device update <id>` | `--name`, `--profile`, `--analytics`, `--status <s>` |
| `device delete <id>` | `--confirm=<name>` — `--yes` alone is never enough |
| `device types` | — |

**Table:** `NAME, ID, PROFILE, STATUS, CLIENTS, CTRLD, LAST SEEN`
**JSON:** `{id, device_id, name, profile: {id, name}, status, analytics, clients, ips, learn_ip, ctrld: {version, status, last_fetch}, icon, resolvers, last_seen}`

**Device lifecycle is four states, not a boolean** — `status` is the string
`pending | active | soft-disabled | hard-disabled` (API ints 0-3, never exposed — D10). The two
disabled modes differ materially: *soft* serves plain unfiltered DNS, *hard* serves none.
`device update --status soft-disabled|hard-disabled|active` writes it (**PUT-only** — devices are
born `pending` and flip to `active` on their first DNS query, not via the API; `--status active` on
a pending device is rejected client-side with that explanation). There is no `--enabled/--disabled`
alias — the boolean cannot say which disable it means.

- `--type` maps to the API's `icon` field — the icon key *is* the device type (`desktop-linux`,
  `router-openwrt`, ...). `--clients` maps to `client_count`. Live, only `name` and `--profile` are
  enforced (the spec wrongly marks `icon`/`client_count` required; `client_count` defaults to 1).
  A duplicate name is a conflict (exit 6).
- `--analytics none|some|full` <- the API's `stats` ints 0-2, named after its own `analytics levels`
  catalogue (`No/Some/Full Analytics`). Integers never appear in input or output
  ([D10](decisions.md)); JSON `analytics` is the name, `null` when unset.
- **`ctrld`, `icon`, and `analytics` are optional** and absent on some devices — model them as `Option`.
- `ctrld` reports the *daemon's* version on that endpoint. `device list` can surface
  `version != version_target` as an upgrade hint.
- `device get` is a **client-side filter** — the API has no `GET /devices/{id}`.
- `device types` returns a **nested dict** (`os`/`browser`/`tv`/`router` -> `icons` -> `{name,
  settings}`), not a list. Flatten it for the table: `TYPE, ICON, NAME`.

---

## access, proxy

| Command | Flags |
| --- | --- |
| `access list` | `--device <id>` *(required)* |
| `access add <ip>...` | `--device <id>` |
| `access remove <ip>...` | `--device <id>`, `--yes` |
| `proxy list` | `--country <cc>` |

**access table:** `IP, ADDED, ISP` — **JSON:** `{ip, added, country, city, isp, asn, as_name}`
*(geo fields null until learned; response key is `body.ips` — blank in the spec, verified live)*
**proxy table:** `CODE, CITY, COUNTRY` — **JSON:** `{code, city, country, country_name, lat, lon}`

- `--device` is a **query param**, not a path segment.
- `DELETE /access` is a **DELETE with a body**.
- `GET /access` is capped at **the latest 50 IPs**, with no pagination. Say so in `--help`; when 50 come
  back, print a note to stderr that the list may be truncated.
- `access add`/`access remove` take at most **50 IPs** per invocation (exit `2` above): the
  read-back window is those latest 50, so a larger add cannot be verified — and the form-var
  ceiling is **unprobed for `ips[]`**; the cap keeps it unreachable ([D11](decisions.md)). After
  `access add`, read back and assert every IP is present — added IPs are the newest, so they sit
  inside the window.
- `access remove` sends **one `DELETE` per IP**, mirroring `rule delete`. Read-back cannot prove a
  removal (an older IP may sit outside the 50-entry window), and the batch ack is `body: []` plus
  a human count that must never be parsed — so the per-request ack **is** the per-target outcome.
  A mixed batch reports each IP's result through the same [D4](decisions.md) `details` contract;
  `retry_argv` covers only the failures.
- **Proxy `code` (PK) is the legal `--via` value for `--action redirect`.** `proxy list` is therefore
  the discovery command for redirect targets — cross-reference it from `rule create --help`.
- Proxy key sets **vary between entries**; some fields are optional.

---

## account, billing, analytics, misc

> **`org` is deferred to a later release** ([D15](decisions.md)) — `cdctl` targets personal accounts.
> The `--org` flag arrives with the org commands; context-keyed config keeps them additive.
> Reachable today via `cdctl api`.

| Command | Table | JSON |
| --- | --- | --- |
| `account get` | `EMAIL, VERIFIED, 2FA, REGION` | `{id, email, verified, created, twofa, region, proxy_access}` — `GET /users`, body flat, **no controller key** |
| `billing products` | `PRODUCT, TYPE, EXPIRY` | `{id, name, type, expiry, proxy_access}` |
| `billing subscriptions` | `SUBSCRIPTION, PRODUCT, STATE, NEXT BILL` | `{id, product: {id, name, type}, state, method, amount, currency, started, ended, next_bill}` |
| `billing payments` | — | **Not in v1.** No probe account has payment history, so no schema can be verified — and a typed passthrough is exactly what [D2](decisions.md) forbids. Use `cdctl api /billing/payments`; the typed command ships once a real sanitized payload fixes the schema. |
| `analytics levels` | `LEVEL, TITLE` | `{level, title}` — upstream `PK` is an **int** (0/1/2) |
| `analytics regions` | `REGION, TITLE, COUNTRY` | `{region, title, country}` — upstream `PK` is a **string** (`america`/`europe`/`asia`) |
| `network` | `POP, CITY, COUNTRY, DNS, API, PROXY` | `{pops: [{pop, city, country, lat, lon, up: {dns, api, proxy}}], time, current_pop}` — `body` carries `time`/`current_pop` **siblings**; `up.*` are booleans (API sends 0/1) |
| `ip` | `IP, TYPE, ORG, COUNTRY, POP` | `{ip, type, org, asn, country, handler, pop}` — body flat, **no controller key** |

> **No `analytics query`.** No public query-log API exists. Do not add the command.

### Meta command output

| Command | stdout |
| --- | --- |
| `auth status` | `{authenticated, email, region, token_source}` — `token_source`: `env` \| `config`. Human mode: key/value lines. No token is the D4 envelope (`auth.missing_token`, exit `4`) — `authenticated: false` is never printed |
| `auth login` / `auth logout` | nothing — confirmation goes to stderr |
| `config get <k>` | the raw value (nothing when unset); `config list` -> `{context: {value, source}, token: {set, source}, default_profile: {value, source}}`, `source`: `env` \| `config` \| `default` \| `null` — the token's *value* never prints; `config path` -> the file path |
| `completions <shell>` / `reference` | the artifact itself (script / Markdown) — **not JSON**; an explicit `--json`/`--fields` is a usage error (exit 2), ambient `CONTROLD_OUTPUT=json` is ignored — an env-configured agent can still install completions |
| `api ...` | the upstream body **verbatim**, byte-for-byte with no added newline (the binary `/mobileconfig` response survives piping) — unstable by design ([D9](decisions.md)); errors still classify to standard exit codes, with the literal noun `resource` in slugs (`resource.not_found`) — the passthrough cannot know what it touched. An explicit `--json`/`--fields` is a usage error (exit `2`) like the artifact rows — neither can be honored on a verbatim body; ambient `CONTROLD_OUTPUT=json` shapes only error rendering. A 2xx whose body carries no error marker is **success, whatever the body's shape** — typed writes demand `success: true`, but the passthrough cannot impose the envelope (the binary `/mobileconfig` case); confirming a raw write's effect means re-fetching state |

### `cdctl api` request encoding

One exact encoding per input form — nothing is sniffed ([D16](decisions.md)):

| Input | Wire |
| --- | --- |
| `-F key=value` *(repeatable)* | `application/x-www-form-urlencoded` body. **Keys are sent literally** — write `-F 'hostnames[]=a' -F 'hostnames[]=b'` yourself; values are form-encoded, keys are never rewritten. Requires a non-GET `-X` (exit `2` otherwise — no documented GET takes a form body) |
| `--input -` | stdin sent **verbatim** with `Content-Type: application/json` — the API's one JSON write is `PUT .../filters`. No validation, no reshaping. Requires a non-GET `-X <method>` (exit `2` otherwise — a GET never carries a body here) |
| `-F` **and** `--input -` | mutually exclusive — exit `2` |
| Query parameters | ride in the path: `cdctl api '/access?device_id=abc'` — taken verbatim; [D9](decisions.md) origin rules still apply |
| `DELETE` with a body | `-X DELETE -F 'ips[]=1.2.3.4' -F device_id=abc` — covers `DELETE /access` |

---

## Name resolution

`--profile`, `--folder`, `--device`, and positional ids all accept **a name or an id**.

1. Exact id match wins.
2. Else exact (case-insensitive) name match (ASCII case folding — non-ASCII characters must
   match exactly).
3. **Multiple matches -> error, exit `2`.** Never guess. Folder names in particular are not unique.
4. No match -> exit `3`.

Resolution costs an extra `GET /profiles` (or `/devices`, `/groups`). Cache it for the process
lifetime; do not cache across runs.

---

## Confirmation matrix ([D8](decisions.md))

| Operation | TTY | Non-TTY |
| --- | --- | --- |
| `rule delete`, `access remove` | prompt | needs `--yes`, else exit `7` |
| `folder delete` (deletes contained rules) | prompt, **state the rule count** | needs `--yes` |
| `profile delete`, `device delete` | `--confirm=<name>` | `--confirm=<name>` — `--yes` alone is **not** enough |
| `org update` | `--confirm=<org>` + **"this is billable"** | same |
| `api` with a non-GET `-X` | needs `--yes`, else exit `7` (`confirmation.required`) — **never prompts**, the escape hatch is gated, not conversational ([D9](decisions.md)) | same |

`--yes` is **ignored when the target is implicit** — the profile came from the config file's
`default_profile`. `--profile` and `CONTROLD_PROFILE` both count as **explicit** ([D8](decisions.md)),
so env-configured agents keep `--yes`. Deleting the wrong profile because it was the default is the
failure mode worth designing out.
