# Write Verification

Live `POST`/`PUT`/`DELETE` against a throwaway trial account (personal, plan "Try Control").
Probed across multiple sessions (latest 2026-07-11); the account is left empty after each.

## Array encoding — there was never a contradiction

`POST /profiles/{id}/rules`:

| Encoding | Result |
| --- | --- |
| `hostnames[]=a&hostnames[]=b` *(documented)* | :white_check_mark: 200 |
| `hostnames[0]=a&hostnames[1]=b` *(`ctrld-sync`)* | :white_check_mark: 200 |
| `hostnames=a&hostnames=b` *(bare repeat)* | :x: 400 `40003 hostnames must be an array` |
| JSON `{"hostnames":["a","b"]}` | :white_check_mark: 200 |

Both bracket and indexed work; the docs and `ctrld-sync` were each right. What fails is the **bare
repeat** — exactly what Go's `net/http` and Python's `parse_qs` emit by default.

**-> We send `hostnames[]`.**

## The backend is PHP

`do=9` returns a literal `print_r()` dump:

```
do must be one of Array
(
    [0] => 0
    [1] => 1
    ...
)
```

That explains the array encoding above: PHP's `parse_str` accepts both notations. **Upstream `message`
can be multi-line. Never parse it.**

## Content-Type is ignored

JSON body + form header -> 200. Form body + JSON header -> 200. The server sniffs the body.
**We don't rely on it** — send what the spec declares ([D16](../decisions.md)).

## :warning: The 1001-variable silent truncation

**The server parses at most ~1001 form variables and silently discards the rest.** The request
still returns 200. Measured on `POST /rules` (`do` + `status` + N `hostnames[]`):

| Body | Vars | Result |
| --- | --- | --- |
| params first + 998 hostnames | 1000 | :white_check_mark: 998 stored |
| params first + 999 hostnames | 1001 | :white_check_mark: 999 stored |
| params first + 1000 hostnames | 1002 | :warning: **200, 999 stored** — last hostname silently dropped |
| 1000 hostnames + `do` + `status=0` last | 1002 | :warning: **200, 1000 stored — with `status=1`**: the trailing `status=0` was dropped and *defaulted*, storing the opposite of what was sent |
| 1000 hostnames + `status` + `do` last | 1002 | :white_check_mark: 400, **0 stored** — a missing `do` is a hard error, never a default |

- **The response cannot reveal this.** `body.rules` on create/modify is a **one-entry summary**
  (`{do, status, via, group, order}` — no hostnames), however many rules the request carried.
- **Consequences for `cdctl`:** chunk imports at **500** (comfortably inside); cap typed
  multi-target writes client-side (**500** hostnames on `rule create/update`, **50** IPs on
  `access add/remove` — the `ips[]` ceiling is unprobed and the cap keeps it so); place scalar
  params *first* so any overflow drops trailing hostnames instead of `status` (undetectable
  corruption); **verify the full desired state after every multi-target write** — re-fetch and
  assert every written hostname is present with the intended action, enabled state, `via`, `via6`,
  and folder. A bare count check is weaker: a concurrent add can mask a dropped hostname.
- An earlier session recorded "1000 :white_check_mark: all stored" — wrong; it stored 999 and the diff went unchecked.

## Two more ceilings — these ones loud

| Probe | Result |
| --- | --- |
| 1500 / 2000 hostnames in one batch | :x: 400 `Failed to create or modify custom rule(s)`, **0 stored** |
| batch that would cross **10,000 rules/profile** (9,991 + 10) | :x: 400 `You have reached the maximum number of custom rules`, **0 stored** — no clipping |
| single rule under the cap (-> 9,992) | :white_check_mark: 200 |

The **10,000 custom-rule cap is real and enforced per profile**, atomically per batch.
`Failed to create or modify custom rule(s)` is the *generic* rule-write failure — it also fires for
a missing `do`. **No bulk delete exists.** N rules = N requests (or delete the profile).

## `PUT /rules` is a merge, not a replace

The spec marks `do` and `status` required. Live, both are optional and omitted fields are preserved:

| Sent (rule was `do=2, via=..., status=1`) | Stored |
| --- | --- |
| `status=0` only | `do=2, via` kept; `status=0` :white_check_mark: merge |
| `do=0` only | `do=0`, `status` kept — and **`via` cleared** (leaving spoof/redirect drops it) |
| `group=0` only | rule moved to the **root folder**, other fields kept :white_check_mark: |

No read-modify-write needed to toggle one field.

### `via_v6` cannot be cleared *(probed 2026-07-11, rule was `do=2, via=192.0.2.10, via_v6=2001:db8::1`)*

| Sent | Result |
| --- | --- |
| `do=2, via` — `via_v6` omitted | preserved :white_check_mark: merge applies to `via_v6` too |
| `via_v6=` (empty) | :x: 400 `40003` `Via_v6 must be a minimum of 1 characters` — atomic, nothing changed (fixture `err_via6_clear.json`) |
| `via_v6=0` | :x: 400 `40003` `Invalid rule action was provided` — atomic, nothing changed |
| `do=1` only | flipping the action away from spoof clears **both** `via` and `via_v6` |

**Conclusion: the API has no operation that clears `via_v6` while the spoof action persists.** The
only clearing mechanism is the action flip, whose intermediate state (a spoof rule momentarily
bypassing) is an unprotected window `cdctl` never enters implicitly. Consequence: a desired state of
`via6: null` against a live rule with `via_v6` set is **unconvergeable** — plans that require it
fail fast before any mutation ([commands.md](../commands.md#rule-import-semantics)).

## Folders

- `POST /groups` **without `do`** -> :white_check_mark: 200, an action-less folder (`{"action":{"status":1}}`).
  Even bare `name` works; `status` defaults to 1. The spec marks `do` required — it is not.
- `PUT /groups/{folder}` **merges** *(probed 2026-07-11 — the spec marks `do`+`status` required
  on PUT; they are not)*:

  | Sent (folder was `name=probeA, do=0, status=1`) | Stored |
  | --- | --- |
  | `name=` only | renamed; `do`/`status` preserved :white_check_mark: |
  | `status=0` only | disabled; name/`do` preserved :white_check_mark: |
  | `do=1` only | action changed; `status=0` preserved :white_check_mark: |
  | rename of an **action-less** folder | stays action-less — no `do` materializes :white_check_mark: |

  | spoof folder (`do=2, via=...`) -> `do=0` only | action changed and **`via` cleared** :white_check_mark: — same as rules; no stale remnant (fixture `write_folder_update.json`) |

  The PUT response carries the **full folder object** (the spec's empty response schema is wrong),
  and the `{folder}` path segment must be the **integer `PK`** — the folder *name* 404s
  (`40003 This folder does not exist`). In the create/update response, `group` holds the name and
  `PK` the id.
- `DELETE /groups/{folder}` **without a body** -> :white_check_mark: 200. The spec's four required body fields are a
  copy-paste artifact from the PUT page.

## Filters

- **Level names work on both write endpoints** — `PUT .../filters/filter/ads_medium` and the batch
  JSON `{"filter":"porn_strict"}`. The bare family PK (`ads`) is the base level.
- Enabling another level of an enabled family **swaps atomically** (`ads_medium` on -> enable
  `ads_small` -> medium off). Levels never stack.
- The write response is a **map keyed by family**, with `lvl` naming the active level:
  `{"filters":{"porn":{"do":0,"status":1,"lvl":"porn_strict"}}}`. The docs' array-of-strings
  example for the single endpoint is wrong live.
- :warning: **When zero filters remain enabled, that same key is `[]`** — PHP serializes an empty map as an
  array. `body.filters` is object-or-array *by emptiness*. Required test fixture.
- Invalid name -> **400** `40003 Invalid filter name` (not 404).

## Services

`PUT /services/{service}` -> 200; the response **echoes the action fields sent** (incl.
`via`/`via_v6` — fixture `write_service_v6.json`; the read-back GET carries the full service
object with nested `action` — fixture `p_service_v6.json`). `status=0` **persists the rule
disabled** — no removal exists. Invalid service -> 400 `40003 Invalid service was provided`.

`via_v6` on services behaves **exactly like rules** *(probed 2026-07-11, service was
`do=2, via=192.0.2.10, via_v6=2001:db8::1`)*:

| Sent | Result |
| --- | --- |
| `do=2, via` — `via_v6` omitted | preserved :white_check_mark: the PUT merges it |
| `via_v6=` (empty) | :x: 400 `40003` `Via_v6 must be a minimum of 1 characters` — atomic |
| `via_v6=0` | :x: 400 `40003` `Invalid service rule action was provided` — atomic |
| `do=1` only | flipping away from spoof clears **both** `via` and `via_v6` |

**No clear exists here either — and services cannot be deleted**, so the only route to
`via_v6: null` is the explicit two-step action flip, with its unprotected window.

## Devices & access

- `POST /devices` enforces **only `name` + `profile_id`** — the spec wrongly marks `icon` and
  `client_count` required (`client_count` defaults to 1). Missing `profile_id` -> 400 `40002`.
- Duplicate device name -> 400 `40003 A device with this name already exists` (a conflict -> exit 6).
- Response: device object **flat at `body`** + `message`, exactly as the spec says.
- `GET /access` -> **`body.ips`** (the key the spec leaves blank):
  `{ip, ts, country, city, isp, asn, as_name}` — geo fields null until learned.
- `POST /access` / `DELETE /access` ack with `body: []` + `"N IPs added"` / `"N IPs deleted"`.

## Profile `disable_ttl`

- `PUT /profiles/{id}` with `disable_ttl=<future unix ts>` disables the profile until then; `0`
  re-enables. There is **no boolean disable** — it's a deadline, and `POST /profiles` has no
  equivalent at all.
- Readback shape-shifts: enabled -> `"disable": null`; disabled -> the `disable` key is *replaced* by
  `"disable_ttl": <ts>`.
- The PUT response can echo a **stale** `disable_ttl` — confirm state with a fresh GET.
- `PUT`/`POST /profiles` return the full profile under `body.profiles[]` + `message` (the spec
  declares both responses empty).

## `groups/import` — exists, but crashes

| Payload | Result |
| --- | --- |
| `{}` | 400 `40002 config is required` |
| `{"config":{}}` | 400 `40003 Invalid import file` |
| any plausible `{"config":{group,rules}}` | **500, 0-byte body** |

It validates, then 500s on every shape tried. Undocumented *and* broken -> **unused** (D16).

> **Robustness case, now a fixture:** a response can send `content-type: application/json` with a
> **0-byte body**. Trusting the header and calling a JSON parser fails uselessly.

## Rule actions

| Probe | Result |
| --- | --- |
| `do=2` spoof, no `via` | :x: 400 — **`via` required for spoof** |
| `do=2` spoof, `via=192.0.2.5` | :white_check_mark: 200 |
| `do=3` redirect, `via=LHR` *(valid)* | :warning: **402** `40201 You need the Full Control plan` |
| `do=3` redirect, `via=ZZZ` *(invalid)* | :warning: **402** — *same*; plan check precedes validation |
| duplicate hostname | :x: **400** `40003 Custom Rule already exists` — **not 409** |
| missing `do` | :x: 400 `Failed to create or modify custom rule(s)` — never a default |

**Redirect is plan-gated.** Because the 402 fires before validation, we **cannot** tell whether the
API validates proxy codes at all. `cdctl` validates `--via` client-side against `GET /proxies`
regardless. `402` -> `plan.upgrade_required`, exit 5.

## `body: []` is not an error marker

Successful deletes (`profile`, `folder`, `device`), access writes, and all-disabled filter sets
return `body: []` with `success: true` (usually plus `message`). Only `success` and `error`
distinguish outcomes.

## Wildcard hostnames in DELETE paths

`*.wild.example.com` -> percent-encode `*` as `%2A` -> :white_check_mark: 200.

## Write responses

`POST /profiles` returns **`body.profiles[]` — an array**, not the flat object the spec claims for
`POST /devices`. `message` is present and human-facing. New profile's `da` is an object
(`{"do":1,"status":1}`), never `[]`.

**Profile PKs vary in length** (13 chars here, 12 on another account). Treat them as opaque strings.

## Corrections to earlier docs

| Earlier claim | Actual |
| --- | --- |
| Docs and `ctrld-sync` contradict on encoding | Both work |
| `groups/import` is a promising bulk endpoint | Exists, 500s, unusable |
| Duplicate -> `409` | `400` |
| `via` required for spoof | :white_check_mark: confirmed |
| Batch of 1000 -> "all stored" | **999 stored** — <= ~1001 form vars parsed, rest silently dropped |
| `PUT /rules` requires `do`+`status` (spec) | Merge; both optional |
| `POST /groups` requires `do` (spec) | Optional — omitting it makes an action-less folder |
| `DELETE /groups/{folder}` requires a body (spec) | None needed |
| Single-filter write returns array of names (docs) | Family-keyed map with `lvl`; `[]` when empty |
| `POST /devices` requires `icon`+`client_count` (spec) | Only `name`+`profile_id` enforced |

## Still unverified

- Proxy-code validation (masked by the 402 plan gate — needs Full Control).
- Any 429 / rate limit.
- `icon` on `PUT /devices/{id}`; `profile_id2` read-back; `lock_status` values.
- The `[]`-shaped `da` (never reproduced; tolerate both).
- The `ips[]` form-variable ceiling (`POST /access`) — the CLI's 50-IP cap keeps it unreachable.
- Level-less filter writes (`PUT /filters/filter/{family}` for families without `levels[]`,
  e.g. `noai`) — only leveled families were probed.
