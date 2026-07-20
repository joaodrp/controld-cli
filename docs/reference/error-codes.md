# Error Codes

Control D publish **no error-code catalogue**. This is ours, by observation.

## Classify on the prefix, not the code

Control D document the structure once: *"first 3 digits of the error code match the HTTP status."*

```
40401  ->  404 + subcode 01
```

**Key on the 3-digit prefix.** It's total: an unseen code still classifies correctly. The table below
is a refinement layer, not the mechanism.

| Prefix | Slug family | Exit | Retryable |
| --- | --- | --- | --- |
| `400` | `request.invalid`, :warning: except `40001`, and **also covers conflicts** | 1 | no |
| `401` / `403` | `auth.*` / `permission.denied` *(never observed)* | 4 / 5 | no |
| `402` | `plan.upgrade_required` | 5 | no |
| `404` | `*.not_found` | 3 | no |
| `429` | `ratelimit.exceeded` *(never observed)* | 8 | **yes** |
| `5xx` | `upstream.error` | 8 | **yes** |
| *(no response)* | `network.error` | 8 | **yes** |

**:warning: The `400` trap.** Auth runs *before* routing, so a missing/invalid token returns **400 / `40001`**,
not 401. Special-case it to `auth.*` (exit 4). Every other `400xx` is genuine validation.

**`409` is never used.** A duplicate rule returns **400 / `40003`**. Don't branch on `409`.

## Observed codes — the entire known universe

| Code | HTTP | Message | Slug | Exit |
| --- | --- | --- | --- | --- |
| `40001` | 400 | `No session token provided` | `auth.missing_token` | 4 |
| `40001` | 400 | `Invalid session, please login again.` | `auth.invalid_token` | 4 |
| `40002` | 400 | `config is required` | `request.invalid` | 1 |
| `40002` | 400 | `profile_id is required` | `request.invalid` | 1 |
| `40003` | 400 | `Invalid import file` *(`groups/import` with an empty `config`)* | `request.invalid` | 1 |
| `40003` | 400 | `hostnames must be an array` | `request.invalid` | 1 |
| `40003` | 400 | `Invalid rule action was provided` *(spoof without `via`)* | `request.invalid` | 1 |
| `40003` | 400 | `do must be one of Array\n(\n    [0] => 0\n...` (**multi-line PHP dump**) | `request.invalid` | 1 |
| `40003` | 400 | `Custom Rule already exists: ...` | `rule.conflict` | 6 |
| `40003` | 400 | `A device with this name already exists` | `device.conflict` | 6 |
| `40003` | 400 | `Name must be a minimum of 3 characters` | `request.invalid` | 1 |
| `40003` | 400 | `Failed to create or modify custom rule(s)` *(batch over ceiling, also missing `do`)* | `request.invalid` | 1 |
| `40003` | 400 | `Invalid filter name` *(batch variant appends `'foo' at index 0`)* | `request.invalid` | 1 |
| `40003` | 400 | `Invalid service was provided` | `request.invalid` | 1 |
| `40003` | 400 | `Via_v6 must be a minimum of 1 characters` *(empty `via_v6=`, there is no clear)* | `request.invalid` | 1 |
| `40003` | 400 | `Invalid service rule action was provided` *(`via_v6=0` on a service, a distinct wording from the rule variant)* | `request.invalid` | 1 |
| `40003` | 400 | `You have reached the maximum number of custom rules` | `request.invalid` | 1 |
| `40003` | 400 | `Custom Rule does not exist` *(`PUT /rules` targeting a hostname with no existing rule, no upsert)* | `request.invalid` | 1 |
| `40003` | 400 | `Invalid hostname was supplied` *(a hostname containing `%` or `?` at create)* | `request.invalid` | 1 |
| `40003` | 400 | `This folder does not exist` *(the `{folder}` path segment was a name, not the integer `PK`)* | `request.invalid` | 1 |
| `40201` | **402** | `You need the Full Control plan to perform this action.` | `plan.upgrade_required` | 5 |
| `40401` | 404 | `No such group exists.` | `folder.not_found` | 3 |
| `40401` | 404 | `Invalid category` | `category.not_found` | 3 |
| `40401` | 404 | `You have no organizations associated with your account` | `org.not_found` | 3 |

**Five distinct codes.** `40003` alone carries **every meaning in the table above, and every probe
session finds more**, exactly why the prefix rule carries the weight. The subcode is a coarse
bucket, not a unique error id.

## Messages can be multi-line

The backend is PHP and leaks. `do=9` returns a literal `print_r()` dump inside `message`:

```
do must be one of Array
(
    [0] => 0
    [1] => 1
)
```

**Never assume one line. Never parse it.** Human mode collapses CR/LF/TAB to spaces and **strips
every other C0/DEL control** (ANSI escapes included). The `error: ...` line stays single-line and
upstream bytes never rewrite the terminal ([D4](../decisions.md#d4--errors-json-on-stderr-stable-slugs-explicit-retryable)). JSON `upstream.message` carries the
verbatim text, and `--debug` renders it JSON-escaped, visible but never executable.

## Refining `400` by message — best-effort, by design

The *resource* half of a slug comes from the command that ran (`rule create` failing -> `rule.*`),
never from the message. `cdctl api` uses the literal noun `resource` (`resource.not_found`):
the passthrough cannot know what it touched. Only the conflict *classification* rides on a
message match:

| Message contains | Slug | Exit |
| --- | --- | --- |
| `already exists` | `<resource>.conflict` | 6 |
| *anything else* | `request.invalid` | 1 |

**This refinement is best-effort and documented as such.** The wire contract cannot support stable
fine-grained slugs (`40003` is one bucket for an open-ended set of meanings), so if upstream rewording breaks
the match, a conflict degrades to `request.invalid`/exit 1: still terminal, still correct not to
retry. Agents must treat exit 6 as a convenience, never as the only way conflicts present.

## Defensive parsing

| Response | Classification |
| --- | --- |
| `success: false`, `error.code` present | prefix rule (above) |
| `success: false`, `error` or `error.code` missing | classify on the HTTP status prefix |
| non-JSON or empty body, non-2xx | synthesize `upstream.error` from the HTTP status (5xx -> exit 8) |
| **2xx** with unparseable body | `upstream.error`, exit 8 (success cannot be confirmed) |
| `error.code` prefix != HTTP status | trust `error.code` ([D4b](../decisions.md#d4b--classify-on-the-prefix-of-errorcode-not-a-table)) and surface the mismatch under `--debug` |
| `error.code` with no plausible status prefix (100-599) | classify on the HTTP status |

## Empty / non-JSON bodies

`POST /profiles/{id}/groups/import` returns:

```
HTTP 500, content-type: application/json, body: 0 bytes
```

**A response can claim JSON and carry nothing.** Trusting the header and calling a parser fails
uselessly. Synthesize `upstream.error` from the status, exit 8. **Required test fixture.**

## Unknowns

- **429 never observed** (~10 req/s x 60). Whether `Retry-After` accompanies one is unverified.
- **`401`/`403` never observed**: those prefix rules are defensive.
- **Whether a read-scoped token 403s on a write** remains untested: it may be another `400`.
