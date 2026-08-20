# `cdctl` — Design

Command surface and contracts. Rationale lives in [decisions](decisions.md), verified API
behavior in [reference/read-verification](reference/read-verification.md).

> **`cdctl` is a client for the Control D REST API.** It is **not** [`ctrld`](https://github.com/Control-D-Inc/ctrld),
> Control D's DNS proxy daemon. Different tools that coexist.

---

## Shape

Noun-verb, singular nouns, shallow tree — the `gh`/`doctl`/`op` model.

```
cdctl <noun> <verb> [args] [flags]
```

```sh
cdctl profile list
cdctl rule create ads.example.com --action block
cdctl device list --json
cdctl filter enable ads_medium --profile Home   # v0.5
```

### Global flags

| Flag | Env | Notes |
| --- | --- | --- |
| `-p, --profile <PK\|name>` | `CONTROLD_PROFILE` | Takes a **name or a PK**. Names resolve via `/profiles`, ambiguity is an error. Falls back to `default_profile` in config. |
| `--json` | `CONTROLD_OUTPUT=json` | Full document. Boolean: an optional value would swallow the next positional (clap parses `--json <positional>` as the field list). |
| `--fields <a,b>` | | Select JSON fields, implies `--json`. |
| `--plain` | | Tables without borders/color, for `awk`/`cut`. |
| `-y, --yes` | | Skip confirmation. **Ignored when the target is implicit** ([D8](decisions.md#d8--tiered-confirmation)). |
| `-n, --dry-run` | | **Every typed remote mutation** (`rule import`/`restore` join in v0.4): resolve and validate everything, print the request/domain plan, persist nothing: no HTTP write, no config change, no file created. Exit `0`. Excluded from `cdctl api` ([D9](decisions.md#d9--cdctl-api-separately-gateable-get-by-default)). Contract in [commands](commands.md#dry-run). |
| `--no-retry` | | Disable automatic retries. |
| `--timeout <secs>` | | Per-request cap, default 30 s total, 10 s connect ([D12](decisions.md#d12--rate-limiting-reactive-not-predictive)). A hang is worse than a fast failure. |
| `--debug` | | Request/response trace to stderr, including `x-controld-pop`/`x-controld-srv`. Token always redacted. Upstream bytes are JSON-escaped, so control sequences never reach the terminal raw. |
| `-q, --quiet` | | Suppress `info:` advisories on stderr (the implicit-profile notice, retry backoff, post-write confirmations). Warnings and errors always print. Stdout is untouched. |

**No `--token` flag**: argv is world-readable ([D6](decisions.md#d6--auth-env-or-stdin-no---token-flag-no-keyring)). Use `CONTROLD_API_TOKEN` or `cdctl auth login --token-stdin`.

**No `-v` short flag**: the version/verbose ambiguity isn't worth it. `--version` and `--debug`
are explicit.

### An implicit profile prints to stderr

```console
$ cdctl rule list
info: using default profile "Home" (123456abcdefg) from config     # <- stderr, keeps implicit state honest
```

`-q, --quiet` drops these advisories for callers that find them noisy. Warnings and errors are
not gated.

---

## Endpoint coverage — 40 mapped for v1, 5 deferred (org), 1 deferred pending schema (billing payments)

> **Delivery is vertically sliced ([D17](decisions.md#d17--ship-in-vertical-slices-not-all-40-operations-at-once)):**
>
> - v0.1: `cdctl api`, profiles (list/get), rules, folders
> - v0.2: devices (list/get)
> - v0.3: `device update`
> - v0.4: `rule import`/`restore`
> - v0.5: profile writes/options/default, filters, services
> - v0.6: device create/delete/types, access, proxy, analytics, account, billing, network, ip
> - 1.0: the full surface
>
> Until a family's typed commands land, `cdctl api` reaches it.

Nouns are chosen once per resource, cross-referenced against the API name everywhere: *folder*
(the wire says `groups`, the spec's path parameter is literally `{folder}`) matches the dashboard;
*device* (the docs' prose says "Endpoints") follows the wire (`/devices`, `body.devices`,
`device_id`) and plain meaning — "endpoint" is already taken by API paths in a CLI that ships
`cdctl api`.

### profile

| Command | API |
| --- | --- |
| `profile list` | `GET /profiles` |
| `profile get <id>` | *client-side filter of* `GET /profiles`: **the API has no `GET /profiles/{id}`** |
| `profile create <name> [--clone <PK>]` | `POST /profiles` |
| `profile update <id> [--name ...]` | `PUT /profiles/{profile_id}` |
| `profile delete <id>` | `DELETE /profiles/{profile_id}` |
| `profile option list` | `GET /profiles/options` |
| `profile option set <name> [--value ...]` | `PUT /profiles/{profile_id}/options/{name}` |
| `profile default get` | `GET /profiles/{profile_id}/default` |
| `profile default set --action ...` | `PUT /profiles/{profile_id}/default` |

### rule

| Command | API |
| --- | --- |
| `rule list [--folder <id>]` | No `--folder`: **`GET /profiles/{id}/rules`** (segment **omitted**), returning *root rules only*, unioned client-side with one `GET .../rules/{folder_id}` per folder (ids from `GET /profiles/{id}/groups`), since the root listing alone omits every foldered rule. ⚠️ The docs also offer `folder_id=0` for root, but **that 404s**. With `--folder`, just `GET /profiles/{id}/rules/{folder_id}` |
| `rule create <hostname>... --action <a>` | `POST /profiles/{profile_id}/rules` |
| `rule update <hostname>...` | `PUT /profiles/{profile_id}/rules` |
| `rule delete <hostname>...` | `DELETE /profiles/{profile_id}/rules/{hostname}`: hostname may be a wildcard, so **percent-encode carefully** |
| `rule import <file>` | bulk, see below |
| `rule restore <manifest>` | `POST /profiles/{profile_id}/rules`: chunked recreate from a local restore manifest |

### folder (API calls these "groups")

| Command | API |
| --- | --- |
| `folder list` | `GET /profiles/{profile_id}/groups` |
| `folder create <name> [--action ...]` | `POST /profiles/{profile_id}/groups` |
| `folder update <id> ...` | `PUT /profiles/{profile_id}/groups/{folder}` |
| `folder delete <id>` | `DELETE /profiles/{profile_id}/groups/{folder}` |

### filter

| Command | API |
| --- | --- |
| `filter list [--external]` | `GET /profiles/{id}/filters`, `GET /profiles/{id}/filters/external` |
| `filter enable <name>` / `filter disable <name>` | `PUT /profiles/{id}/filters/filter/{filter}` |
| `filter set <name>=<on\|off>...` (batch) | `PUT /profiles/{id}/filters`: **the one `application/json` write** |

`<name>` is a **level name** from `levels[]` for leveled families (`ads_small`, `ads_medium`, `ads`,
`porn_strict`). A family with no levels is addressed by its **family id** (`noai`, `dating`, ...).
`filter list` shows the legal names. Third-party filter PKs are `x-`-prefixed.

### service

| Command | API |
| --- | --- |
| `service list` | `GET /profiles/{profile_id}/services` |
| `service set <service> --action ...` | `PUT /profiles/{profile_id}/services/{service}` |
| `service categories` | `GET /services/categories` |
| `service catalog <category>` | `GET /services/categories/{category}` |

### device (API calls these "endpoints")

| Command | API |
| --- | --- |
| `device list` | `GET /devices` |
| `device get <id>` | *client-side filter* over `GET /devices`: a `GET /devices/{id}` exists live but is undocumented (D16) |
| `device create <name> --profile ...` | `POST /devices` (only `name` + `profile_id` enforced live) |
| `device update <id> ...` | `PUT /devices/{device_id}` |
| `device delete <id>` | `DELETE /devices/{device_id}` |
| `device types` | `GET /devices/types`: nested dict (`os`/`browser`/`tv`/`router`), not a list |

### access, proxy, account, billing, analytics, misc

| Command | API |
| --- | --- |
| `access list --device <id>` | `GET /access?device_id=`: **query param, not a path segment** |
| `access add --device <id> --ip <ip>` | `POST /access` |
| `access remove --device <id> --ip <ip>` | `DELETE /access` (**a DELETE with a body**) |
| `proxy list` | `GET /proxies`: PKs are the legal `--via` values for `redirect` |
| `account get` | `GET /users`: body is the user object, **no controller key** |
| `billing products`, `billing subscriptions` | `GET /billing/products`, `GET /billing/subscriptions`: `billing payments` is **deferred** (no verifiable schema exists: [D2](decisions.md#d2--the-cli-is-the-stability-layer-own-the-output-schema)), reachable via `cdctl api` |
| `analytics levels`, `analytics regions` | `GET /analytics/levels`, `GET /analytics/endpoints` |
| `network`, `ip` | `GET /network`, `GET /ip` |

> **No `analytics query`.** No public query-log API exists. The dashboard has one. The API does not.
> Do not promise it.

> **`GET /mobileconfig/{device_id}`** is documented on the docs site but absent from the spec's 46
> operations (binary `.mobileconfig` response). No typed command. `cdctl api` reaches it.

### org — **deferred to a later release** ([D15](decisions.md#d15--personal-accounts-only-orgs-addable-without-breaking-changes))

`cdctl` targets **personal accounts**. The five organization operations are documented and specified
but **not implemented in v1**. They are a business feature, and a personal account cannot test them
(all five 404). Shipping untested commands is worse than shipping none.

| Future command | API |
| --- | --- |
| `org get`, `org members` | `GET /organizations/organization`, `GET /organizations/members` |
| `org suborg list`, `org suborg create <name>` | `GET /organizations/sub_organizations` (**underscored**), `POST /organizations/suborg` |
| `org update ...` | `PUT /organizations` (⚠️ **billable event**) |

**They must be addable without a breaking change.** Two things make that true and ship in v1:

1. **`org` is a single new top-level noun**: purely additive, cannot collide.
2. **The config schema is context-keyed from the start**, so an org context slots in without a migration.

The `--org` global flag (wired to the documented `X-Force-Org-Id` header) arrives **with** the org
commands: adding an optional flag later is not a breaking change, and its live effect cannot be
verified on a personal account today ([D15](decisions.md#d15--personal-accounts-only-orgs-addable-without-breaking-changes)).

Until then, `cdctl api /organizations/organization` reaches them.

### Meta

| Command | Notes |
| --- | --- |
| `auth login --token-stdin`, `auth status`, `auth logout` | Tokens are dashboard-issued only: **no OAuth/device flow is possible** |
| `config get/set/list/path` | `$XDG_CONFIG_HOME/cdctl/config.toml`, mode `0600` |
| `api <path> [-X <method>] [-F k=v]... [--input -]` | Raw passthrough ([D9](decisions.md#d9--cdctl-api-separately-gateable-get-by-default)). **GET by default**. Non-GET needs `-X <method>` **and** `--yes`. Both body forms (`-F`, `--input -`) require a non-GET `-X`. A distinct verb so sandboxes can deny `Bash(cdctl api:*)` while allowing `Bash(cdctl:*)`. Encoding is explicit, never sniffed: [commands](commands.md#cdctl-api-request-encoding) |
| `completions <shell>`, `reference` | Must work **without a token** ([D7](decisions.md#d7--resolve-auth-lazily)) |

**Coverage: 40/46 operations mapped for v1. The 5 `organizations/*` operations are deferred by scope
([D15](decisions.md#d15--personal-accounts-only-orgs-addable-without-breaking-changes)). `billing payments` is deferred until a real payload makes its schema
verifiable ([D2](decisions.md#d2--the-cli-is-the-stability-layer-own-the-output-schema)). All are reachable today via `cdctl api`.**

Plus one shape absent from the spec's `paths` object: **`GET /profiles/{id}/rules`** with the folder
segment omitted. Only the prose of the `folder_id` parameter description documents it, and that
prose offers `folder_id=0` as an equivalent. **It is not equivalent. `0` 404s.** `rule list` must use
this form, and a client generated from the spec alone would never find it.

---

## The shared action model

`do`/`status` recur across rules, folders, services, and the default rule. One flag group, reused
four times. **Users never type the magic integers** ([D10](decisions.md#d10--never-make-users-type-magic-integers)).

```
--action block|bypass|spoof|redirect     # do = 0|1|2|3
--via <ip|cname|PROXY>                   # spoof -> IP/CNAME;  redirect -> proxy PK (e.g. LHR)
--enabled | --disabled                   # status = 1|0  — orthogonal to --action
```

`--enabled/--disabled` is **independent** of `--action`: a disabled block rule (`do=0,status=0`) is
not an allow.

---

## Output contract

stdout carries **only** data. Diagnostics, prompts, and errors go to stderr. JSON is always
**pretty-printed**: 2-space indent, stable key order, identical piped or not ([D3](decisions.md#d3--no-tty-based-format-switching)). Tables escape
C0/DEL controls in server-supplied strings (`\x1b` -> `^[`), so a resource name must never rewrite the
terminal (the [D4](decisions.md#d4--errors-json-on-stderr-stable-slugs-explicit-retryable) rule, applied to success output).

```console
$ cdctl rule list --json
[
  {
    "hostname": "ads.example.com",
    "action": "block",
    "enabled": true,
    "folder": null,
    "folder_id": null,
    "via": null,
    "via6": null,
    "order": 1
  },
  {
    "hostname": "*.corp.internal",
    "action": "spoof",
    "enabled": true,
    "folder": "Work",
    "folder_id": 2,
    "via": "10.0.0.1",
    "via6": null,
    "order": 2
  }
]
```

Normalized, not passed through ([D2](decisions.md#d2--the-cli-is-the-stability-layer-own-the-output-schema)): `do:0` -> `"action":"block"`, `status:1` -> `"enabled":true`,
`group:0` -> `"folder":null`, `PK` -> `"hostname"`.

### Errors

```console
$ cdctl profile list --json          # bad token
# stdout: (empty)
# stderr:
{
  "error": {
    "code": "auth.invalid_token",
    "message": "Invalid session, please login again.",
    "upstream": {
      "code": 40001,
      "http_status": 400,
      "message": "Invalid session, please login again."
    },
    "retryable": false,
    "retry_after": null,
    "details": null,
    "hint": "Run `cdctl auth login`, or set CONTROLD_API_TOKEN."
  }
}
$ echo $?
4
```

**Exit code 8 is retryable. Everything else is terminal.** The full set: `0` ok, `1` generic,
`2` usage, `3` not found, `4` auth, `5` forbidden/plan, `6` conflict, `7` confirmation required,
`8` retryable, `130` SIGINT, `141` SIGPIPE (Unix). Rationale:
[decisions](decisions.md#d5--nine-exit-codes-exactly-one-retryable).

An empty result is **exit 0**, not an error. A read-scoped token used for a write **fails loudly**
with exit `5`, never a silently filtered result.

---

## Agent affordances

- `--json` everywhere, stdout is data-only.
- **Flags are the primary interface.** LLMs emit valid JSON that then dies on *shell escaping*:
  in [Microsoft's benchmark](https://developer.microsoft.com/blog/dont-rewrite-your-cli-for-agents),
  flags beat JSON payloads 5/5 on correctness and used 4-11x fewer tokens. `cdctl` takes JSON on
  **stdin** (`--input -`), never in argv.
- Structured errors with **stable slugs** and an explicit `retryable` boolean.
- **One retryable exit code (8)**, published.
- **No hidden prompts**: non-TTY without `--yes` fails with exit `7` rather than hanging.
- **No silent degradation**: insufficient scope errors, never filters.
- **`--dry-run` on every typed remote mutation**: resolve, validate, print the normalized plan,
  **persist nothing** (validation GETs run, mutations are withheld). Hallucination containment: a
  bad parameter fails locally, with the real error, before it reaches the API.
- **Inputs assumed adversarial**: control characters anywhere, and `?`/`#`/`%` in path-bound
  identifiers, are rejected with exit `2` before any request is built
  ([commands](commands.md#input-hardening)).
- `cdctl reference`: the entire command surface as one pipeable Markdown document. Works without
  a token.
- **No machine-readable command manifest in v1.** Command metadata stays centralized and derivable so
  a versioned spec can be generated post-v1 when a real consumer needs it ([D3](decisions.md#d3--no-tty-based-format-switching)).
- `AGENTS.md` at the repo root (the [agents.md](https://agents.md) convention, instructions any
  coding agent reads): the `ctrld` disambiguation, the exit-code contract and retryable set, the
  always-`--json` rule, the stdout/stderr split, the no-token-in-argv rule, the **read-only-token**
  preference (unless the task writes), and worked examples.
- `cdctl api`, so an agent is never blocked by a typed command lagging the API, while remaining
  **separately gateable** by a sandbox allowlist.

---

## Hazards & bulk import

Deserialization hazards: [reference/read-verification](reference/read-verification.md).
Write constraints (batch ceiling, no bulk delete, encoding): [reference/write-verification](reference/write-verification.md).
Phasing: [roadmap](roadmap.md).
