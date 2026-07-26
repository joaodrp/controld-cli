# Write Verification

Live `POST`/`PUT`/`DELETE` against a throwaway trial account (personal, plan "Try Control").
Probed across multiple sessions (latest 2026-07-18). The account is left empty after each.

## Array encoding — there was never a contradiction

`POST /profiles/{id}/rules`:

| Encoding | Result |
| --- | --- |
| `hostnames[]=a&hostnames[]=b` *(documented)* | ✅ 200 |
| `hostnames[0]=a&hostnames[1]=b` *(`ctrld-sync`)* | ✅ 200 |
| `hostnames=a&hostnames=b` *(bare repeat)* | ❌ 400 `40003 hostnames must be an array` |
| JSON `{"hostnames":["a","b"]}` | ✅ 200 |

Both bracket and indexed work: the docs and `ctrld-sync` were each right. What fails is the **bare
repeat** (exactly what Go's `net/http` and Python's `parse_qs` emit by default).

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
**We don't rely on it**, sending what the spec declares ([D16](../decisions.md#d16--documented-surface-only)).

## The 1001-variable silent truncation

**The server parses at most ~1001 form variables and silently discards the rest.** The request
still returns 200. Measured on `POST /rules` (`do` + `status` + N `hostnames[]`):

| Body | Vars | Result |
| --- | --- | --- |
| params first + 998 hostnames | 1000 | ✅ 998 stored |
| params first + 999 hostnames | 1001 | ✅ 999 stored |
| params first + 1000 hostnames | 1002 | ⚠️ **200, 999 stored**, last hostname silently dropped |
| 1000 hostnames + `do` + `status=0` last | 1002 | ⚠️ **200, 1000 stored, with `status=1`**: the trailing `status=0` was dropped and *defaulted*, storing the opposite of what was sent |
| 1000 hostnames + `status` + `do` last | 1002 | ✅ 400, **0 stored**, a missing `do` is a hard error, never a default |

- **The response cannot reveal this.** `body.rules` on create/modify is a **one-entry summary**
  (`{do, status, via, group, order}`, no hostnames), however many rules the request carried.
- **Consequences for `cdctl`:** chunk imports at **500** (comfortably inside). Cap typed
  multi-target writes client-side: **500** hostnames on `rule create/update`, **50** IPs on
  `access add/remove` (the `ips[]` ceiling is unprobed, but the cap keeps it out of reach). Place
  scalar params *first*, so any overflow drops trailing hostnames instead of `status` (undetectable
  corruption). **Verify the full desired state after every multi-target write**: re-fetch and
  assert every written hostname is present with the intended action, enabled state, `via`, `via6`,
  and folder. A bare count check is weaker: a concurrent add can mask a dropped hostname.
- An earlier session recorded "1000 ✅ all stored", but that was wrong: it stored
  999 and the diff went unchecked.

## Two more ceilings — these ones loud

| Probe | Result |
| --- | --- |
| 1500 / 2000 hostnames in one batch | ❌ 400 `Failed to create or modify custom rule(s)`, **0 stored** |
| batch that would cross **10,000 rules/profile** (9,991 + 10) | ❌ 400 `You have reached the maximum number of custom rules`, **0 stored**, no clipping |
| single rule under the cap (-> 9,992) | ✅ 200 |

The **10,000 custom-rule cap is real and enforced per profile**, atomically per batch.
`Failed to create or modify custom rule(s)` is the *generic* rule-write failure. It also fires for
a missing `do`. **No bulk delete exists.** N rules = N requests (or delete the profile).

## `PUT /rules` is a merge, not a replace

The spec marks `do` and `status` required. Live, both are optional and omitted fields are preserved:

| Sent (rule was `do=2, via=..., status=1`) | Stored |
| --- | --- |
| `status=0` only | `do=2, via` kept, `status=0` ✅ merge |
| `do=0` only | `do=0`, `status` kept, and **`via` cleared** (leaving spoof/redirect drops it) |
| `group=0` only | rule moved to the **root folder**, other fields kept ✅ |

No read-modify-write needed to toggle one field.

### `via_v6` cannot be cleared *(probed 2026-07-11, rule was `do=2, via=192.0.2.10, via_v6=2001:db8::1`)*

| Sent | Result |
| --- | --- |
| `do=2, via`, `via_v6` omitted | preserved ✅ merge applies to `via_v6` too |
| `via_v6=` (empty) | ❌ 400 `40003` `Via_v6 must be a minimum of 1 characters` (atomic, nothing changed, fixture `err_via6_clear.json`) |
| `via_v6=0` | ❌ 400 `40003` `Invalid rule action was provided` (atomic, nothing changed) |
| `do=1` only | flipping the action away from spoof clears **both** `via` and `via_v6` |

**Conclusion: the API has no operation that clears `via_v6` while the spoof action persists.** The
only clearing mechanism is the action flip, whose intermediate state (a spoof rule momentarily
bypassing) is an unprotected window `cdctl` never enters implicitly. Consequence: a desired state of
`via6: null` against a live rule with `via_v6` set is **unconvergeable**: plans that require it
fail fast before any mutation ([commands.md](../commands.md#rule-import-semantics-v02)).

## Folders

- `POST /groups` **without `do`** -> ✅ 200, an action-less folder (`{"action":{"status":1}}`).
  Even bare `name` works, and `status` defaults to 1. The spec marks `do` required, but it is not.
- `PUT /groups/{folder}` **merges** *(probed 2026-07-11: the spec marks `do`+`status` required
  on PUT, but they are not)*:

  | Sent (folder was `name=probeA, do=0, status=1`) | Stored |
  | --- | --- |
  | `name=` only | renamed, `do`/`status` preserved ✅ |
  | `status=0` only | disabled, name/`do` preserved ✅ |
  | `do=1` only | action changed, `status=0` preserved ✅ |
  | rename of an **action-less** folder | stays action-less, no `do` materializes ✅ |

  | spoof folder (`do=2, via=...`) -> `do=0` only | action changed and **`via` cleared** ✅, same as rules, no stale remnant (fixture `write_folder_update.json`) |

  The PUT response carries the **full folder object** (the spec's empty response schema is wrong),
  and the `{folder}` path segment must be the **integer `PK`**: the folder *name* is rejected
  (HTTP 400, `40003 This folder does not exist`). In the create/update response, `group` holds the
  name and `PK` the id.
- `DELETE /groups/{folder}` **without a body** -> ✅ 200. The spec's four required body fields are a
  copy-paste artifact from the PUT page.

## Filters

- **Level names work on both write endpoints**: `PUT .../filters/filter/ads_medium` and the batch
  JSON `{"filter":"porn_strict"}`. The bare family PK (`ads`) is the base level.
- Enabling another level of an enabled family **swaps atomically** (`ads_medium` on -> enable
  `ads_small` -> medium off). Levels never stack.
- The write response is a **map keyed by family**, with `lvl` naming the active level:
  `{"filters":{"porn":{"do":0,"status":1,"lvl":"porn_strict"}}}`. The docs' array-of-strings
  example for the single endpoint is wrong live.
- ⚠️ **When zero filters remain enabled, that same key is `[]`**: PHP serializes an empty map as an
  array. `body.filters` is object-or-array *by emptiness*. Required test fixture.
- Invalid name -> **400** `40003 Invalid filter name` (not 404).

## Services

`PUT /services/{service}` -> 200. The response **echoes the action fields sent** (incl.
`via`/`via_v6`, fixture `write_service_v6.json`). The read-back GET carries the full service
object with nested `action` (fixture `p_service_v6.json`). `status=0` **persists the rule
disabled**, no removal exists. Invalid service -> 400 `40003 Invalid service was provided`.

`via_v6` on services behaves **exactly like rules** *(probed 2026-07-11, service was
`do=2, via=192.0.2.10, via_v6=2001:db8::1`)*:

| Sent | Result |
| --- | --- |
| `do=2, via`, `via_v6` omitted | preserved ✅ the PUT merges it |
| `via_v6=` (empty) | ❌ 400 `40003` `Via_v6 must be a minimum of 1 characters` (atomic) |
| `via_v6=0` | ❌ 400 `40003` `Invalid service rule action was provided` (atomic) |
| `do=1` only | flipping away from spoof clears **both** `via` and `via_v6` |

**No clear exists here either, and services cannot be deleted**, so the only route to
`via_v6: null` is the explicit two-step action flip, with its unprotected window.

## Devices & access

- `POST /devices` enforces **only `name` + `profile_id`**: the spec wrongly marks `icon` and
  `client_count` required (`client_count` defaults to 1). Missing `profile_id` -> 400 `40002`.
- Duplicate device name -> 400 `40003 A device with this name already exists` (a conflict -> exit 6).
- Response: device object **flat at `body`** + `message`, exactly as the spec says.
- `GET /access` -> **`body.ips`** (the key the spec leaves blank):
  `{ip, ts, country, city, isp, asn, as_name}`, geo fields null until learned.
- `POST /access` / `DELETE /access` ack with `body: []` + `"N IPs added"` / `"N IPs deleted"`.

## Profile `disable_ttl`

- `PUT /profiles/{id}` with `disable_ttl=<future unix ts>` disables the profile until then, and `0`
  re-enables. There is **no boolean disable**: it's a deadline, and `POST /profiles` has no
  equivalent at all.
- Readback shape-shifts: enabled -> `"disable": null`. Disabled -> the `disable` key is *replaced* by
  `"disable_ttl": <ts>`.
- The PUT response can echo a **stale** `disable_ttl`. Confirm state with a fresh GET.
- `PUT`/`POST /profiles` return the full profile under `body.profiles[]` + `message` (the spec
  declares both responses empty).

## `groups/import` — exists, but crashes

| Payload | Result |
| --- | --- |
| `{}` | 400 `40002 config is required` |
| `{"config":{}}` | 400 `40003 Invalid import file` |
| any plausible `{"config":{group,rules}}` | **500, 0-byte body** |

It validates, then 500s on every shape tried. Undocumented *and* broken -> **unused** ([D16](../decisions.md#d16--documented-surface-only)).

> **Robustness case, now a fixture:** a response can send `content-type: application/json` with a
> **0-byte body**. Trusting the header and calling a JSON parser fails uselessly.

## Rule actions

| Probe | Result |
| --- | --- |
| `do=2` spoof, no `via` | ❌ 400: **`via` required for spoof** |
| `do=2` spoof, `via=192.0.2.5` | ✅ 200 |
| `do=3` redirect, `via=LHR` *(valid)* | ⚠️ **402** `40201 You need the Full Control plan` |
| `do=3` redirect, `via=ZZZ` *(invalid)* | ⚠️ **402**, same as above (plan check precedes validation) |
| duplicate hostname | ❌ **400** `40003 Custom Rule already exists`, **not 409** |
| missing `do` | ❌ 400 `Failed to create or modify custom rule(s)`, never a default |

**Redirect is plan-gated.** Because the 402 fires before validation, we **cannot** tell whether the
API validates proxy codes at all. `cdctl` validates `--via` client-side against `GET /proxies`
regardless. `402` -> `plan.upgrade_required`, exit 5.

## `body: []` is not an error marker

Successful deletes (`profile`, `folder`, `device`), access writes, and all-disabled filter sets
return `body: []` with `success: true` (usually plus `message`). Only `success` and `error`
distinguish outcomes.

## Wildcard hostnames in DELETE paths

`*.wild.example.com` -> percent-encode `*` as `%2A` -> ✅ 200.

## Write responses

`POST /profiles` returns **`body.profiles[]`, an array**, not the flat object the spec claims for
`POST /devices`. `message` is present and human-facing. New profile's `da` is an object
(`{"do":1,"status":1}`), never `[]`.

`POST /profiles` rejects names longer than **32 characters** (400,
`Name must be a maximum of 32 characters`), undocumented in the spec.

**Profile PKs vary in length** (13 chars here, 12 on another account). Treat them as opaque strings.

## Corrections to earlier docs

| Earlier claim | Actual |
| --- | --- |
| Docs and `ctrld-sync` contradict on encoding | Both work |
| `groups/import` is a promising bulk endpoint | Exists, 500s, unusable |
| Duplicate -> `409` | `400` |
| `via` required for spoof | ✅ confirmed |
| Batch of 1000 -> "all stored" | **999 stored**: <= ~1001 form vars parsed, rest silently dropped |
| `PUT /rules` requires `do`+`status` (spec) | Merge, both optional |
| `POST /groups` requires `do` (spec) | Optional: omitting it makes an action-less folder |
| `DELETE /groups/{folder}` requires a body (spec) | None needed |
| Single-filter write returns array of names (docs) | Family-keyed map with `lvl`, `[]` when empty |
| `POST /devices` requires `icon`+`client_count` (spec) | Only `name`+`profile_id` enforced |

## `PUT /rules` does not upsert; `via` case is preserved; `%`/`?` reject at create *(probed 2026-07-18)*

| Probe | Result |
| --- | --- |
| `PUT /profiles/{id}/rules` targeting a hostname with no existing rule | ❌ 400 `Custom Rule does not exist`, **nothing created**, confirming "`PUT /rules` is a merge, not a replace" above never upserts |
| `POST /profiles/{id}/rules` with `via=MiXeD-CaSe.Example.COM` | ✅ 200, read back **byte-for-byte identical**: case is preserved verbatim |
| `POST /profiles/{id}/rules` with a hostname containing `%` or `?` | ❌ 400 `Invalid hostname was supplied` |
| `POST /profiles/{id}/rules` with `hostnames[]=MiXeD.Example.COM` | ✅ 200, read back `PK: "MiXeD.Example.COM"`, **case-preserved** exactly like `via` above |
| `PUT /profiles/{id}/rules` with `hostnames[]=mixed.example.com` against that same rule | ❌ 400 `Custom Rule does not exist`, **target matching is case-SENSITIVE**, not just storage |
| `DELETE /profiles/{id}/rules/{hostname}` with a hostname matching no rule | ✅ 200 `success: true`, `"Custom rule(s) deleted"`, a **silent no-op**: the rule set is unchanged |
| `POST /profiles/{id}/rules` with `hostnames[]=MiXeD.Example.COM`, then again with `hostnames[]=mixed.example.com` | ✅ both 200: **case variants coexist as two distinct rules** in the same profile |

**Consequences for `cdctl`:** `rule update` cannot create a missing rule, so a typo'd hostname must
be caught before the write: a pre-write check against a fresh profile-wide read-back, not
whatever generic error the failed `PUT` happens to produce. `rule restore` (commands.md) must
route a manifest hostname absent from the profile through `POST`, never `PUT`, for the same reason.
The spoof `via`/`via6` read-back comparison ([rule](../commands.md#rule)) is case-insensitive
despite this result: DNS names are case-insensitive and the API is unversioned, so relying on
today's case-preserving behavior continuing would be fragile. A redirect `via` is a proxy PK (an
exact identifier) and compares case-sensitively.

The *hostname* pre-write check is more than existence, because of the last row above: since case
variants can coexist as distinct rules, and the server's own `PUT`/`DELETE` target matching is
case-sensitive, `cdctl` cannot simply fold a target's case to find *a* match: it must resolve to
the *right* one. `rule update`/`rule delete` therefore **keep their targets case-preserved**
(`create` still lowercases, for canonical creation) and resolve each one against the pre-write
listing. An exact case-sensitive match wins outright (this is also how a user disambiguates two
coexisting variants). Otherwise, a *unique* case-insensitive match is accepted, with an `info:` line
naming the substitution (never silent, since the write would otherwise land on a hostname other
than the literal input). 2+ case-insensitive matches with no exact one is refused (exit `2`, naming
every variant) rather than guessed. `rule delete`'s own ack is equally uninformative: since a
non-matching `DELETE` acks success too, `cdctl` runs the same resolution before every delete, never
trusting the ack alone to mean something was actually removed.

## Still unverified

- Proxy-code validation (masked by the 402 plan gate, needs Full Control).
- Any 429 / rate limit.
- `icon` on `PUT /devices/{id}`, `profile_id2` read-back, `lock_status` values.
- The `[]`-shaped `da` (never reproduced, tolerate both).
- The `ips[]` form-variable ceiling (`POST /access`): the CLI's 50-IP cap keeps it unreachable.
- Level-less filter writes (`PUT /filters/filter/{family}` for families without `levels[]`,
  e.g. `noai`). Only leveled families were probed.
