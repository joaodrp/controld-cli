# Live Verification (reads)

Read-only `GET` probes against a real account. Where this contradicts Control D's spec or docs, **this
wins**. Writes: [write-verification](write-verification.md). Provenance is per section below: most
of this file is from the initial 2026-07-11 pass, and sections that say otherwise are later.

## The envelope has three shapes, not one

Docs say data lives under `body.<controllerName>`. Not always:

| Shape | Endpoints | `body` is |
| --- | --- | --- |
| **keyed** *(documented)* | most | `{"profiles": [...]}` |
| **flat (no key)** | `/users`, `/ip` | the resource itself |
| **keyed + siblings** | `/network` | `{"network": [...], "time": ..., "current_pop": ...}` |

**On error, `body` is `[]` (an array)**, whatever the success shape. And `[]` is **not an error
marker**: successful deletes, access writes, and all-disabled filter sets return `body: []` with
`success: true`. Deserialize as `Value`, unwrap per-operation, branch only on `success`/`error`.
A generic `Envelope<T>` with one mandatory key is wrong.

`error` also carries an undocumented **`date`** field.

## Auth failures are HTTP 400, not 401

Auth runs *before* routing, so an unauthenticated request to a bogus path returns the **auth** error,
not a 404. All of these are `400` / `40001`:

- no token, bad token, nonexistent path with no token

**Classify on `error.code`, never on the HTTP status.** See [error-codes](error-codes.md#classify-on-the-prefix-not-the-code).

## Listing root rules: the docs offer two ways, one is false — and the working one isn't "all rules"

The `folder_id` param says *"0 or omit for root"*. **Only `omit` works.**

```
GET /profiles/{id}/rules/0   -> 404  "No such group exists."
GET /profiles/{id}/rules     -> 200  root rules only
```

`folder_id=0` never resolves. The working form is **absent from the spec's `paths`** (documented only
in the param prose), so a spec-generated client never finds it, and `0` is the more natural guess.

**The root listing is not profile-wide: it omits every foldered rule outright.** Verified 2026-07-14
on a fresh profile with one folder (id 1) containing one rule:

```
GET /profiles/{pk}/rules     -> {"body":{"rules":[]},"success":true}              # the rule is absent
GET /profiles/{pk}/rules/1   -> {"body":{"rules":[{"PK":"probe.cdctl-smoke.example.com",
                                  "order":1,"group":1,"action":{"do":0,"status":1}}]},"success":true}
```

A foldered rule is visible only through its own folder's listing, `GET /profiles/{id}/rules/{folder_id}`
(folder ids from `GET /profiles/{id}/groups`). A client that wants every rule in a profile must fetch
the root listing plus one such GET per folder. `cdctl` does this (`rule.rs`'s `fetch_all_rules`).

`group: 0` *inside a rule* means something else: a sentinel for **"not in a folder"**. Not a listable id.

**An empty folder is not an error, but a deleted one is.** Verified 2026-07-14. This probe was not
read-only, since telling the two apart needed a folder deleted mid-test:

```
GET /profiles/{pk}/rules/{empty_folder_id}    -> {"body":{"rules":[]},"success":true}  # still exists
GET /profiles/{pk}/rules/{deleted_folder_id}  -> 404  "No such group exists."
```

The empty case does not trigger the `body: []`-on-error flip: `success` stays `true`. `cdctl`
relies on this: the folder ids driving `fetch_all_rules`'s per-folder GETs come from a `/groups` fetch
moments earlier, so a 404 there can only mean the folder was deleted in the race between the two calls.
Its rules died with it, so contributing zero rules is the true state, not a failure to tolerate.

## The action model (live data)

```jsonc
{"PK":"host.example.com", "order":1, "group":0, "action":{"do":2,"status":1,"via":"192.0.2.10"}}
{"PK":"other.example.com","order":5, "group":0, "action":{"do":1,"status":1}}
```

- `do`: `0`=BLOCK `1`=BYPASS `2`=SPOOF `3`=REDIRECT (integers).
- `status`: `0`/`1`, **orthogonal to `do`**. `do=0,status=0` is a *disabled block*, not an allow.
- `via`: present for SPOOF/REDIRECT, absent otherwise.
- `order` has **gaps** (1,2,3,5...).
- Rule `PK` is the hostname, **wildcards included** (`*.example.com`) -> percent-encode into DELETE paths.

**A missing `action.do` is an error, never a default.** `controld-go` reads it as `0` = BLOCK, the
worst possible default for a DNS tool.

## Filter "mode suffixes" are discoverable

Each filter's `levels[]` enumerates its own legal names:

```
ads  -> ads_small (Relaxed), ads_medium (Balanced), ads (Strict)
porn -> porn (Relaxed), porn_strict (Strict)
```

**`PK` is the family, and `levels[].name` is what you enable.** No derivable suffix rule: strict is bare
`ads` for one family and `porn_strict` for another. **Read `levels[]`. Never construct names.**

20 native filters. 14 third-party, PKs prefixed **`x-`**.

## `GET /proxies` — spec has no schema; reality is the redirect target list

103 entries, `PK` = IATA-style code (`TIA`, `SYD`, `LHR`...). **These are the legal `via` values for
REDIRECT.** Key sets vary between entries: some fields optional.

## Shapes that break naive clients

| Endpoint | Trap |
| --- | --- |
| `/devices/types` | `body.types` is a **nested dict** (`os`/`browser`/`tv`/`router` -> `icons`), not a list |
| `/devices` | Optional: `ctrld`, `icon`, `stats`. `ctrld` reports the daemon's version |
| `/profiles/{id}/services` | Optional: `warning`, `locations` |
| `/analytics/levels` | `PK` is an **int** |
| `/analytics/endpoints` | `PK` is a **string** |

**`PK`'s type varies by endpoint**: string (profiles, devices, rules, proxies), int (folders,
analytics levels). Never assume.

**Summary counts != list lengths.** `profile.svc.count` said 16, but the list returned 18 (16 enabled + 2
disabled). Counts track **enabled** items only.

**`da` / default rule:** every account returned an **object**. The `[]`-when-unset form was never
reproduced, but the spec types it as an array: **tolerate both**.

## Rate limits: none observed

40 sequential (~3.4 req/s) and 60 concurrent (~10 req/s) -> all 200, **no 429, no rate-limit headers**.
The only figure anywhere is a `controld-go` comment: 1200 req/5min, a **budget, not an rps cap**.
Handle 429 reactively. Don't predict it.

Headers do expose `x-controld-srv` / `x-controld-pop` (worth echoing under `--debug`).

## Misc

- `stats_endpoint` (from `/users`) = e.g. `europe` -> implies a second host,
  `https://{stats_endpoint}.analytics.controld.com`. Undocumented, out of scope.
- `GET /organizations/organization` -> **404 on personal accounts**, normal, not a failure.
- `GET /services/categories/{c}` needs a real PK: `audio career finance gaming hosting news
  recreation shop social tools vendors video`.
- `GET /access?device_id=`: device scoping is a **query param**, not a path segment. Response key
  (blank in the spec) is **`body.ips`**: `{ip, ts, country, city, isp, asn, as_name}`, geo nullable.
- `profile.*` hints at undocumented surface: `cflt` (custom filters), `ipflt` (IP filters, no
  endpoint at all).
