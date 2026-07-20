# Control D Public API — Authoritative Contract

Source: every page under `https://docs.controld.com/reference/<slug>.md` (all 46 operation slugs +
`get-started`, `authentication`, `response-conventions`), cross-checked against the concept guides at
`https://docs.controld.com/docs/<slug>.md`. All 46 slugs returned HTTP 200 with real content. None 404'd.

Each reference page embeds a self-contained OpenAPI 3.0.1 fragment (`info.version: "1.0.1"`,
`servers[0].url: https://api.controld.com`). Everything below is transcribed from those fragments.

**Confidence markers used throughout:**
- **[SPEC]**: explicitly stated in the OpenAPI fragment or prose. Trust it.
- **[EXAMPLE]**: only visible in a response example, not in the declared schema. Likely right, but not contractual.
- **[GAP]**: the docs are silent. Must be probed against the live API.
- **[CONFLICT]**: the docs contradict themselves. Must be probed.

---

## 0. Global

### Base URL
```
https://api.controld.com
```
Declared identically in every page's `servers` block. No `/v1` or any other version segment. **[SPEC]**

### Authentication
Bearer token in the `Authorization` header. **[SPEC]**

```
Authorization: Bearer YOUR_FANCY_TOKEN
```

Verbatim from the Authentication page:
> Supply your API token in the `Authorization` header as a Bearer token for all authenticated API calls.

```bash
curl https://api.controld.com/users \
  --header 'authorization: Bearer YOUR_FANCY_TOKEN'
```

The OpenAPI `securitySchemes` confirms:
```json
"bearerAuth": { "type": "http", "scheme": "bearer" }
```

Token permission levels (set in the dashboard at `https://controld.com/dashboard/api`, not via API): **[SPEC]**
- **Read**: reads only.
- **Write**: full access (reads + writes).

Tokens can optionally carry an **Allowed IPs** restriction (comma-separated IPv4/IPv6 or CIDR).

There is **no API endpoint to create, list, or revoke tokens**: dashboard only. **[GAP]**

#### Endpoints declaring `security: []` (i.e. documented as *unauthenticated*)
- `GET /services/categories`
- `GET /services/categories/{category}`
- `GET /users`  <- **[CONFLICT]** the Authentication page uses `GET /users` as *the* example of an authenticated
  call ("If successful, you'll see your account details"), yet the `get_users` OpenAPI fragment sets
  `security: []`. The Authentication page is almost certainly correct. Treat `/users` as requiring a token.
- `GET /network`
- `GET /ip`

All other operations declare `security: [{ bearerAuth: [] }]`.

### Organization impersonation header
```
X-Force-Org-Id: <org_id>
```
Documented on `GET /profiles`, `GET /devices`, and `GET /access`. Verbatim: **[SPEC]**
> If you have a organization account (ignore this if you do not), you can "impersonate" an admin of a child
> sub-organization, by supplying `X-Force-Org-Id: org_id_goes_here` HTTP header along with all API calls
> within the Profiles scope. This will allow you to view, create and modify Profiles within the target
> sub-organization using the API token of the parent (main) Organization.

Scope is stated per-page as "within the Profiles scope" / "within this scope". Whether it applies to
Organization or Billing scopes is **[GAP]**.

### Success envelope **[SPEC]**
```json
{
  "body": {
    "examples": [
      { "PK": "123abc", "ts": 1665556443, "name": "Cheese" }
    ]
  },
  "success": true,
  "message": "Optional message goes here"
}
```
- `success: true` on success.
- Returned data lives under `body.<controllerName>`: the controller name is the resource collection key
  (`profiles`, `filters`, `rules`, `groups`, `services`, `devices`, `categories`, `members`,
  `organization`, `sub_organizations`, `options`, `default`, `types`).
- Every unique object has a primary key in the `PK` field.
- `message` is optional and only present on some writes.

Exceptions to the `body.<controllerName>` rule: the object sits directly at `body`:
- `POST /devices` -> device object at `body` (not `body.devices`). **[SPEC]**
- `PUT /devices/{device_id}` -> device object at `body`. **[SPEC]**
- `GET /ip` -> object at `body`. **[SPEC]**, **[LIVE-CONFIRMED]**
- `GET /users` -> object at `body`. **[SPEC]**, **[LIVE-CONFIRMED]**
- `DELETE /devices/{device_id}` -> `body` is an empty array `[]`. **[SPEC]**

A third shape exists that the spec does not describe: a controller key *plus sibling metadata*:
- `GET /network` -> `body` = `{"network": [...], "time": <int>, "current_pop": "<pop>"}`. **[LIVE ONLY]**

> :warning: So `body` has **three** shapes, not two. The unwrap key must be per-operation configuration
> supporting *keyed*, *flat*, and *keyed-plus-siblings*. And on **error**, `body` is `[]`, an
> **array**, regardless of the success shape. See
> [read-verification.md section 1](read-verification.md#the-envelope-has-three-shapes-not-one).

### Error envelope **[SPEC]**
```json
{
  "body": [],
  "success": false,
  "error": {
    "message": "Error message goes here",
    "code": _ERROR_CODE_
  }
}
```
- `success: false`.
- `body` is an **empty array**, not an object. (A Rust deserializer must tolerate `body` being either an
  object or `[]`.)
- `error` contains `message` (string) and `code` (integer).
- **"First 3 digits of the error code will match the HTTP status code."** So `40003` => HTTP 400.

Real error example from the `PUT /profiles/{profile_id}/filters` page. Note it carries an **undocumented
`date` field**: **[EXAMPLE]**
```json
{
  "body": [],
  "success": false,
  "error": {
    "date": "Wed, 11 Mar 2026 21:52:24 -0400",
    "message": "Invalid filter name 'foo' at index 0",
    "code": 40003
  }
}
```
Only one concrete error code (`40003`) appears anywhere in the docs. There is **no error-code table**. **[GAP]**

### Versioning **[SPEC]**
Verbatim from Get Started:
> **Usage Warning** — Currently the API has no versioning, and things can change at any time. Usually these
> changes should be non-breaking, however breaking changes can be introduced without warning.

### Rate limiting
**Nothing is documented.** No rate-limit section, no `X-RateLimit-*` headers, no 429 mention anywhere in
the reference or guides. **[GAP]** Must be probed.
(Note: `docs.controld.com` itself rate-limits scrapers with 429, but that is the docs host, not the API.)

### Pagination
**Nothing is documented.** No `page`, `limit`, `offset`, `cursor`, or `next` anywhere. Every list endpoint
returns a bare array. The only bound stated anywhere is `GET /access`: "List up to latest **50** IPs".
Assume list endpoints are unpaginated. **[GAP]**

### Content types for writes — the important bit
**Almost every write is `application/x-www-form-urlencoded`, not JSON.**

| Operation | Request content type |
|---|---|
| `POST /profiles` | `application/x-www-form-urlencoded` **[SPEC]** |
| `PUT /profiles/{profile_id}` | `application/x-www-form-urlencoded` **[SPEC]** |
| `DELETE /profiles/{profile_id}` | *(no body declared)* |
| `PUT /profiles/{profile_id}/options/{name}` | `application/x-www-form-urlencoded` **[SPEC]** |
| `PUT /profiles/{profile_id}/default` | `application/x-www-form-urlencoded` **[SPEC]** |
| **`PUT /profiles/{profile_id}/filters`** | **`application/json`** <- the one JSON write **[SPEC]** |
| `PUT /profiles/{profile_id}/filters/filter/{filter}` | `application/x-www-form-urlencoded` **[SPEC]** |
| `POST /profiles/{profile_id}/groups` | `application/x-www-form-urlencoded` **[SPEC]** |
| `PUT /profiles/{profile_id}/groups/{folder}` | `application/x-www-form-urlencoded` **[SPEC]** |
| `DELETE /profiles/{profile_id}/groups/{folder}` | `application/x-www-form-urlencoded` (body declared, see **[CONFLICT]** below) |
| `POST /profiles/{profile_id}/rules` | `application/x-www-form-urlencoded` **[SPEC]** |
| `PUT /profiles/{profile_id}/rules` | `application/x-www-form-urlencoded` **[SPEC]** |
| `DELETE /profiles/{profile_id}/rules/{hostname}` | `application/x-www-form-urlencoded` (empty body schema) |
| `PUT /profiles/{profile_id}/services/{service}` | `application/x-www-form-urlencoded` **[SPEC]** |
| `POST /devices` | `application/x-www-form-urlencoded` **[SPEC]** |
| `PUT /devices/{device_id}` | `application/x-www-form-urlencoded` **[SPEC]** |
| `DELETE /devices/{device_id}` | *(no body declared)* |
| `POST /access` | `application/x-www-form-urlencoded` **[SPEC]** |
| `DELETE /access` | `application/x-www-form-urlencoded` **[SPEC]** (a DELETE **with a body**) |
| `POST /organizations/suborg` | `application/x-www-form-urlencoded` **[SPEC]** |
| `PUT /organizations` | `application/x-www-form-urlencoded` **[SPEC]** |

Responses are always JSON ("Responses are always going to be json, unless specified otherwise.").

### Array-style form params
Three exist. In form-encoded bodies the **literal bracket suffix `[]` is part of the field name** as declared:

| Param | Used by | Declared example |
|---|---|---|
| `hostnames[]` | `POST /profiles/{profile_id}/rules`, `PUT /profiles/{profile_id}/rules` | `["domain1.com","domain2.com"]` |
| `ips[]` | `POST /access`, `DELETE /access` | `["1.2.3.4","a:b:c:d:e:f::"]` |
| `filters` (JSON array of objects) | `PUT /profiles/{profile_id}/filters` | see section Filters |

**[GAP]** The docs do not say whether repeated keys (`hostnames[]=a&hostnames[]=b`) or indexed keys
(`hostnames[0]=a&hostnames[1]=b`) are expected. PHP-style backends (which this looks like) accept repeated
`name[]=` keys. Probe this. It is the single highest-risk encoding decision in the whole client.

`do` and `status` are **NOT** arrays anywhere. They are scalar integers on every operation that takes them.

---

## 1. Domain enums

### `do` — rule action. **This is the central enum.** **[SPEC]**
Stated verbatim and *identically* on `PUT /profiles/{profile_id}/default`, `POST /profiles/{profile_id}/rules`,
`PUT /profiles/{profile_id}/rules`, `POST /profiles/{profile_id}/groups`, and
`PUT /profiles/{profile_id}/services/{service}`:

> Rule type. 0 = BLOCK. 1 = BYPASS, 2 = SPOOF, 3 = REDIRECT

| Value | Name | Meaning | `via` semantics |
|---|---|---|---|
| `0` | BLOCK | Prevent the domain from resolving | unused |
| `1` | BYPASS | Resolve to true IP via authoritative DNS (overrides Filters/Services/Default) | unused |
| `2` | SPOOF | Answer with an IP or CNAME you supply | `via` = IPv4 address **or** hostname (A / CNAME). `via_v6` = IPv6 (AAAA) |
| `3` | REDIRECT | Route through a Control D proxy | `via` = 3-letter IATA proxy identifier (e.g. `JFK`, `YYZ`, `SYD`) from `GET /proxies`. `via_v6` has no effect |

They are **integers**, not strings.

The `PUT /profiles/{profile_id}/services/{service}` page restates it as an explicit list:
> ## do
> * `0` - Block service domains
> * `1` - Bypass service domains (to override Filter block)
> * `2` - Spoof service domains to IP
> * `3` - redirect service domains via proxy
>
> ## via
> * In `do=2` mode, this arg supplies the `A` record or `CNAME`.
> * In `do=3` mode, this arg supplies a 3 letter IATA code proxy identifier.
>
> ## via_v6
> * In `do=2` mode, this arg supplies the `AAAA` record
> * No effect when `do=3`

:warning: **The concept guides describe only THREE actions** (Block / Bypass / Redirect) because the web UI folds
SPOOF and REDIRECT into one "Redirect" control with a "Proxies vs IP or Hostname" sub-choice. The **API has
four values**. Do not let the guide prose mislead the implementation. (`custom-rules.md`: "one of 3 rule types".
`default-rule.md`: "one of 3 actions".)

`via` is **never marked required** on any operation, even when `do=2` or `do=3`. The conditional requirement
(`do in {2,3}` => `via` needed) is not expressed in the schema. **[GAP]** Enforce it client-side.

### `status` — rule/filter/option enabled flag **[SPEC]**
`0` = disabled, `1` = enabled. Integer. Consistent everywhere (rules, folders, services, filters, options).
Declared with `enum: [0,1], minimum: 0, maximum: 1` on `PUT /profiles/{profile_id}/filters`.

:warning: Do **not** confuse this with `status` on `PUT /devices/{device_id}`, which is a **4-value device lifecycle
enum** (see section Devices).

### Filter identifiers (`PK`) **[SPEC]**
From the `filters` guide's "Filter Name to Filter PK Mapping" table:

| PK | Name |
|---|---|
| `ads` | Ads & Trackers |
| `porn` | Adult Content |
| `noai` | Artificial Intelligence |
| `fakenews` | Clickbait |
| `cryptominers` | Crypto |
| `dating` | Dating |
| `drugs` | Drugs |
| `ddns` | Dynamic DNS |
| `filehost` | File Hosting |
| `gambling` | Gambling |
| `games` | Games |
| `gov` | Government Sites |
| `iot` | IoT Telemetry |
| `malware` | Malware |
| `nrd` | New Domains |
| `typo` | Phishing |
| `social` | Social |
| `torrents` | Torrents & Privacy |
| `urlshort` | URL Shorteners |
| `dnsvpn` | VPN & DNS |

:warning: **[CONFLICT]** The `GET /profiles/{profile_id}/filters` response example lists only 15 filters and omits
`noai`, `ddns`, `filehost`, `games`, `urlshort`. The guide table (20 entries) is newer. **Do not hardcode
this list**: fetch it from `GET /profiles/{profile_id}/filters` at runtime.

:warning: **[CONFLICT] Filter "mode" suffixes.** The `PUT /profiles/{profile_id}/filters` batch example sends
`porn_strict` and its success response contains `ads_small`. Neither is in the PK table, and neither is
returned by the List endpoint. So a filter name may be a bare PK (`ads`) or a suffixed variant
(`ads_small`, `porn_strict`). The docs **never define the suffix grammar or enumerate the variants**.
This is a real hole. Probe it. The `additional` field on List Native describes "Relaxed Mode"/"Strict Mode"
per filter but gives no wire values.

### Filter sub-option types **[EXAMPLE]**
Each native filter may carry an `options[]` array. Seen `type` values, with their `name`:
- `counterfilter`: `cbl_tracking` (on `ads`: "Allow Email Tracking and Affiliate Links")
- `service`: `safesearch` (on `porn`: "Enable Safe Search")
- `ipfilter`: `ip_malware` (on `malware`: "Block Malicious IPs")

**[GAP]** There is **no documented endpoint to toggle these sub-options.** Not in the given operation list,
and no reference page exists for it. Probe.

### Profile option PKs **[EXAMPLE]** (from `GET /profiles/options`)
| PK | Title | `type` | `default_value` |
|---|---|---|---|
| `block_rfc1918` | DNS Rebind Protection | `toggle` | `0` |
| `spoof_ipv6` | IPv4/IPv6 Compatibility Mode | `toggle` | `0` |
| `no_dnssec` | Disable DNSSEC | `toggle` | `0` |
| `ml_filter` | AI Malware Filter | `toggle` | `0` |
| `ttl_blck` | Block TTL (seconds) | `field` | `10` |
| `ttl_spff` | Redirect TTL (seconds) | `field` | `20` |

`type` is `toggle` (use `status` only) or `field` (use `status` + `value`). The list is from an example, so it
may be stale. Fetch it at runtime.

### Device analytics level (`stats`) **[SPEC]**
`0` = off, `1` = basic ("Some Analytics", counts only), `2` = full ("Full Analytics", queries + metadata).
Declared on `POST /devices` and `PUT /devices/{device_id}`: "Set analytics level on device. 0 = off, 1 = basic, 2 = full".
`GET /analytics/levels` returns these at runtime, but its response schema is empty in the docs. **[GAP]**

### Device lifecycle `status` **[SPEC]** (`PUT /devices/{device_id}` only)
> Update device status. 0 - pending, 1 - active, 2 - soft disabled, 3 - hard disabled

| Value | Name | Meaning (from `device-status` guide) |
|---|---|---|
| `0` | Pending | Never used, flips to Active on first query |
| `1` | Active | Currently or previously used |
| `2` | Soft disabled | Profile no longer enforced, acts as a plain resolver |
| `3` | Hard disabled | Serves no DNS at all |

:warning: `status` is **not** an accepted field on `POST /devices`, only on `PUT`. **[SPEC]**

### Device/endpoint types and icons **[EXAMPLE]** (from `GET /devices/types`)
Response is `body.types`, an **object keyed by category**, not an array. The `icon` value you pass to
`POST /devices` is one of the *keys* of a category's `icons` map.

| Category | `name` | Icon keys |
|---|---|---|
| `os` | Desktop & Mobile | `mobile-ios`, `mobile-android`, `desktop-windows`, `desktop-mac`, `desktop-linux` |
| `browser` | Browsers | `browser-chrome`, `browser-firefox`, `browser-edge`, `browser-brave`, `browser-other` |
| `tv` | TV & Media | `tv`, `tv-apple`, `tv-android`, `tv-firetv`, `tv-samsung` |
| `router` | Routers | `router`, `router-openwrt`, `router-ubiquiti`, `router-asus`, `router-ddwrt` (+ `setup_url`) |

:warning: **[GAP]** The router category has an extra `setup_url` key, but the others don't. Deserialize defensively.

### Service categories **[EXAMPLE]**
`audio`, `gaming`, `shop`, `social`, `tools`, `video`. Fetch via `GET /services/categories`.

### Organization member permission levels **[EXAMPLE]** — only two values appear, in an example:
`100` -> `"Owner"`, `1` -> `"Viewer"`. The response ships both `permission.level` (int) and
`permission.printable` (string). **The full ladder is undocumented.** **[GAP]** Read `printable`, don't
map `level` yourself.

### Proxy identifiers
3-letter IATA codes. Examples across the docs: `JFK`, `YYZ`, `BRU`, `SYD`, `LAX`, `ADL`, plus
non-IATA-looking values `LOCAL` (Default Rule example) and `RES_ORD` (Hulu residential proxy). So the
"3 letter IATA code" description is **not strictly true**: `RES_*` residential proxies and `LOCAL` exist.
`GET /proxies` returns the real list. **Its response schema is empty in the docs.** **[GAP]**

### Analytics storage regions **[EXAMPLE]**
`stats_endpoint` values seen: `america`, `jfk-org01`, `ams-org01`. Fetch via `GET /analytics/endpoints`,
whose response schema is empty in the docs. **[GAP]**

---

## 2. Profiles

### `GET /profiles` — Profiles - List
List all profiles associated with an account.
- Params: none. Honors `X-Force-Org-Id`.
- Response key: **`body.profiles`** (array)

| Field | Type | Notes |
|---|---|---|
| `PK` | string | **primary key**, e.g. `52wtfl4k`. Required on every profile-scoped call |
| `updated` | integer | unix ts |
| `name` | string | |
| `stats` | integer | |
| `profile` | object | counts summary |
| `profile.flt.count` | integer | native filters |
| `profile.cflt.count` | integer | counter-filters |
| `profile.ipflt.count` | integer | IP filters |
| `profile.rule.count` | integer | custom rules |
| `profile.svc.count` | integer | services |
| `profile.grp.count` | integer | folders |
| `profile.opt.count` / `profile.opt.data[]` | integer / array | each `{PK, value}` |
| `profile.da` | object **or** array | the Default Rule: `{do, via, status}` |

:warning: **[CONFLICT]** `profile.da` is declared in the schema as `"type": "array"` but the examples show it as
either `[]` (empty, when unset) **or** a bare object `{"do":3,"via":"YYZ","status":1}`. Deserialize as
`Option`-ish / untagged-enum. Same trap as the top-level `body`.

### `POST /profiles` — Profiles - Create
Create a new blank profile, or clone an existing one.
- Content-Type: `application/x-www-form-urlencoded` (declared as a **required header param**)

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | **yes** | e.g. `My Profile Name` |
| `clone_profile_id` | string | no | PK of profile to clone. Omit => blank profile |

- Response: schema is empty `{}` in the docs. **[GAP]** Presumably `body.profiles` with the new PK. Probe.

### `PUT /profiles/{profile_id}` — Profiles - Modify
- Path: `profile_id` (string, required): profile PK
- Content-Type: `application/x-www-form-urlencoded` (required header param)
- **No field is marked required** (the schema has no `required` array).

| Field | Type | Notes |
|---|---|---|
| `name` | string | rename |
| `disable_ttl` | integer | "Disable profile until specified unix timestamp. **ttl = 0 disables previous deactivation.**" |
| `lock_status` | integer | lock/unlock profile from being edited |
| `lock_message` | string | message to error with when a locked profile is modified |
| `password` | string | **account password**, required when *unlocking* a profile |

:warning: **[GAP]** `lock_status` values are never enumerated. `0`/`1` is the obvious guess but unstated.

- Response: empty schema `{}`. **[GAP]**

### `DELETE /profiles/{profile_id}` — Profiles - Delete
- Path: `profile_id` (string, required)
- No request body.
- Constraint **[SPEC]**: "Profile cannot be enforced by a device to be deleted successfully (must be orphaned profile)."
- Response: empty schema `{}`. **[GAP]**

### `GET /profiles/options` — Profiles - List Options
Get all available profile options (the catalogue, not a profile's current values).
- :warning: **[CONFLICT]** Declares a **required** `Content-Type: application/x-www-form-urlencoded` *header* on a
  GET with no body. Almost certainly a docs artifact from copy-paste. Harmless to send, probably ignorable.
- Response key: **`body.options`** (array): `PK`, `title`, `description`, `type` (`toggle`|`field`),
  `default_value` (integer), `info_url`. All required.
- > **[LIVE 2026-07-11]** The live catalogue has **13** options (the docs' example shows 6) and a
  > third type the docs never mention: **`dropdown`**, with `default_value` as an object map
  > (`ai_malware`: `{"0.9": "Minimal", ...}`, `b_resp`) or a **bare label array** (`ecs_subnet`).
  > Model `type` as an open enum and `default_value` as raw JSON. Captured in
  > [profile_options.json](../../tests/fixtures/api/profile_options.json).

### `PUT /profiles/{profile_id}/options/{name}` — Profiles - Modify Options
Set an option on a profile.
- Path: `profile_id` (string, required), `name` (string, required: the option PK, e.g. `block_rfc1918`)
- Content-Type: `application/x-www-form-urlencoded` (required header param)

| Field | Type | Required | Notes |
|---|---|---|---|
| `status` | integer | **yes** | 1 = enable, 0 = disable |
| `value` | **string** | no | "Optional value of the option to set" (example: `"something"`) |

:warning: Note `value` is typed **string** here, but `GET /profiles/options` returns `default_value` as an
**integer**, and `body.profile.opt.data[].value` is an **integer**. Send it as a string in the form body
(everything in a form body is a string anyway), expect an integer back.

- Response: empty schema `{}`. **[GAP]**

---

## 3. Default Rule

### `GET /profiles/{profile_id}/default` — Default Rule - List
- Path: `profile_id` (string, required)
- Response key: **`body.default`** (object): `do` (int), `via` (string), `status` (int). All three required.
- Example: `{"do": 3, "via": "LOCAL", "status": 1}`

### `PUT /profiles/{profile_id}/default` — Default Rule - Modify
(The page's `description` erroneously reads "Returns status of the Default Rule.", a copy-paste from the GET.)
- Path: `profile_id` (string, required)
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `do` | integer | **yes** | 0=BLOCK, 1=BYPASS, 2=SPOOF, 3=REDIRECT |
| `via` | string | no | "If SPOOF, this can be an IP or hostname. If REDIRECT, this must be a valid proxy identifier." |
| `status` | integer | **yes** | |

- Response key: **`body.default`** (`{do, via, status}`).
- :warning: No `via_v6` here, unlike custom rules and services. **[GAP]** Can the default rule spoof AAAA? Undocumented.

---

## 4. Filters

### `GET /profiles/{profile_id}/filters` — Filters - List Native
- Path: `profile_id` (string, required)
- Response key: **`body.filters`** (array)

| Field | Type | Required | Notes |
|---|---|---|---|
| `PK` | string | yes | **filter identifier**, e.g. `ads`, `malware` |
| `name` | string | yes | human name |
| `description` | string | yes | |
| `additional` | string | yes* | HTML blob describing Relaxed/Strict modes. *Declared required but **absent** from several example entries (`fakenews`, `dating`, ...). Treat as optional.* |
| `sources` | array of string | yes | empty `[]` for native filters |
| `options` | array | yes* | *Also declared required but absent from most example entries. Treat as optional.* Each: `{title, description, type, name, status}` |
| `status` | integer | yes | 0/1: whether the filter is on for this profile |

### `GET /profiles/{profile_id}/filters/external` — Filters - List 3rd Party
- Path: `profile_id` (string, required)
- Response schema is **empty `{}`** in the docs. **[GAP]** Presumably `body.filters` with the same shape
  plus a populated `sources[]`. Probe.

### `PUT /profiles/{profile_id}/filters` — Filters - Batch Modify
**The only JSON-body write in the entire API.**
- Path: `profile_id` (string, required)
- Content-Type: **`application/json`**

```json
{
  "filters": [
    { "filter": "ads",         "status": 1 },
    { "filter": "malware",     "status": 1 },
    { "filter": "porn_strict", "status": 1 },
    { "filter": "gambling",    "status": 1 }
  ]
}
```
| Field | Type | Required | Notes |
|---|---|---|---|
| `filters` | array of object | **yes** | |
| `filters[].filter` | string | **yes** | "Filter name from the List Filters endpoint" |
| `filters[].status` | integer | **yes** | `enum: [0,1]`, `minimum: 0`, `maximum: 1` |

- :warning: **[CONFLICT]** The declared response schema says `body.filters` is an **array of string**. The success
  *example* shows `body.filters` as an **object/map**:
```json
{ "body": { "filters": {
    "malware":     {"do": 0, "status": 1},
    "gambling":    {"do": 0, "status": 1},
    "fakenews":    {"do": 0, "status": 1},
    "ads_small":   {"do": 0, "status": 1},
    "porn_strict": {"do": 0, "status": 1}
}}, "success": true }
```
  Meanwhile `PUT .../filters/filter/{filter}` (single) shows `body.filters` as a **plain array of strings**
  (`["malware","ads"]`). **Three different shapes for one key.** Probe both endpoints. Do not share a
  response type between them without checking.
- Errors are per-index: `"Invalid filter name 'foo' at index 0"`, code `40003`.

### `PUT /profiles/{profile_id}/filters/filter/{filter}` — Filters - Modify (single)
- Path: `profile_id` (string, required), `filter` (string, required, e.g. `ads`)
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `status` | integer | **yes** | 1 = enable, 0 = disable |

- Response key: **`body.filters`**. Example: an array of filter-name strings `["malware","ads"]`
  (apparently the full set of now-enabled filters).

---

## 5. Rule Folders (a.k.a. "groups")

The URL segment is **`groups`**. The docs title them "Rule Folders". Folder IDs are **integers**.

### `GET /profiles/{profile_id}/groups` — Rule Folders - List
- Path: `profile_id` (string, required)
- Response key: **`body.groups`** (array)

| Field | Type | Required | Notes |
|---|---|---|---|
| `PK` | **integer** | yes | **folder ID** (integer, unlike profile/rule/device PKs, which are strings) |
| `group` | string | yes | folder name |
| `action` | object | yes | `{do, status}`: schema marks **both** `do` and `status` required... |
| `count` | integer | yes | number of rules inside |

:warning: **[CONFLICT]** `action` declares `required: ["do","status"]`, but the very example on the same page shows
a folder with `"action": {"status": 1}`, **no `do`** (a plain, action-less folder). So `do` is genuinely
optional in `action`. Model it as `Option<i64>`.

:warning: `action.via` is **not** in the List schema, but **is** in the Create response schema. **[CONFLICT]** —
expect it to be present for `do=2`/`do=3` folders.

### `POST /profiles/{profile_id}/groups` — Rule Folders - Create
- Path: `profile_id` (string, required)
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | **yes** | folder name |
| `do` | integer | **yes** | 0=BLOCK, 1=BYPASS, 2=SPOOF, 3=REDIRECT. "All rules inside will inherit rule type" |
| `via` | string | no | "Add spoof IP or hostname, or proxy identifier if do=2 or do=3" |
| `status` | integer | **yes** | "Status of the folder and all rules inside" |

:warning: **[CONFLICT]** `do` is marked **required** here, yet the List example proves action-less folders exist
(`{"action": {"status": 1}}`), and the guide says a folder "can be **optionally** assigned an action".
Either the API accepts a sentinel (`do=`? `do=-1`?) for "no action", or action-less folders can only be made
via the UI. **Unresolvable from the docs. Probe.**

- Response key: **`body.groups`** (array, one element): `{PK: int, group: string, action: {do, via, status}, count: int}`

### `PUT /profiles/{profile_id}/groups/{folder}` — Rule Folders - Modify
- Path: `profile_id` (string, required), **`folder`** (string, required, the folder ID, example `"2"`)
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | no | rename |
| `do` | integer | **yes** | |
| `via` | string | no | |
| `status` | integer | **yes** | |

- Response: empty schema `{}`. **[GAP]**

### `DELETE /profiles/{profile_id}/groups/{folder}` — Rule Folders - Delete
Delete folder **and all custom rules inside it**.
- Path: `profile_id` (string, required), `folder` (string, required)
- :warning: **[CONFLICT] The docs declare a form-encoded request body with `name`, `do`, `via`, `status`, all four
  marked REQUIRED, on a DELETE.** This is plainly a copy-paste of the PUT page (the field descriptions even
  say "(Optional) Rename the folder"). A DELETE almost certainly needs no body. **Send no body. Probe if it
  400s.** Note the types here are also `string` where PUT used `integer`, more evidence of a bad copy-paste.
- Response: empty schema `{}`. **[GAP]**

---

## 6. Custom Rules

### `GET /profiles/{profile_id}/rules/{folder_id}` — Custom Rules - List
Return custom rules in a folder. **For the root folder, OMIT the folder ID.**
> :warning: **The docs also say you may pass `0`. You may not: it returns `404 "No such group exists."`**
> Verified live. `0` is not a real folder. See [read-verification.md section 3](read-verification.md#listing-root-rules-the-docs-offer-two-ways-one-is-false--and-the-working-one-isnt-all-rules).
- Path: `profile_id` (string, required), `folder_id` (string, required in the schema, but the description
  says "Folder ID (**0 or omit for root**)" and the operation description says "For root folder, omit the
  folder ID"). So `GET /profiles/{profile_id}/rules` (no trailing segment) is valid. **[SPEC]**
- Response key: **`body.rules`** (array)

| Field | Type | Required | Notes |
|---|---|---|---|
| `PK` | string | yes | **the hostname** (e.g. `ipinfo.io`). This is the rule's primary key |
| `order` | integer | yes | |
| `group` | integer | yes | folder ID. `0` is a **sentinel meaning "not in a folder"**, a legal field *value* but **not a listable folder id** |
| `action` | object | yes | `{do, status}` required, `via` optional |

Examples showing all four `do` values in the wild:
```json
{"PK":"ipinfo.io",     "order":8,  "group":0, "action":{"do":3,"via":"BRU","status":1}}          // REDIRECT via proxy
{"PK":"wtfismyip.com", "order":9,  "group":0, "action":{"do":2,"via":"www.google.com","status":1}} // SPOOF to CNAME
{"PK":"derp123.com",   "order":12, "group":0, "action":{"do":2,"via":"1.2.3.4","status":1}}      // SPOOF to IP
{"PK":"test.com",      "order":10, "group":0, "action":{"do":0,"status":1}}                       // BLOCK
{"PK":"ads.com",       "order":13, "group":0, "action":{"do":1,"status":1}}                       // BYPASS
```

### `POST /profiles/{profile_id}/rules` — Custom Rules - Create
Create **one or more** custom rules.
- Path: `profile_id` (string, required)
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `do` | integer | **yes** | 0=BLOCK, 1=BYPASS, 2=SPOOF, 3=REDIRECT |
| `status` | integer | **yes** | |
| `via` | string | no | SPOOF => IPv4 or hostname. REDIRECT => proxy identifier |
| `via_v6` | string | no | SPOOF only: IPv6 / AAAA record. Example `a:b:c:d:e:f::` |
| `group` | integer | no | folder ID to create the rule in, **root folder if omitted** |
| `hostnames[]` | array | **yes** | array of hostnames. Example `["domain1.com","domain2.com"]` |

Hostname/wildcard grammar (from the `custom-rules` guide): **[SPEC]**
- `domain.com` -> matches `domain.com` **and all subdomains**
- `*.domain.com` -> matches all subdomains but **NOT** `domain.com` itself
- `server-*.domain.com` -> prefix wildcard within a label
- Most specific rule wins. Limit: **10,000 custom rules**.

- Response key: **`body.rules`** (array): `{do, status, via, group, order}`. **Note: no `PK`/hostname in the
  response example.** **[GAP]**

### `PUT /profiles/{profile_id}/rules` — Custom Rules - Modify
Modify an existing custom rule. Same path (no `{hostname}` segment): the target is identified by
`hostnames[]` in the body.
- Path: `profile_id` (string, required)
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `do` | integer | **yes** | |
| `status` | integer | **yes** | |
| `via` | string | no | |
| `via_v6` | string | no | |
| `group` | integer | no | "ID of the folder to create this rule in" (i.e. **moving a rule between folders**) |
| `hostnames[]` | array | **yes** | example here is the bare string `"domain1.com"`, not an array (docs sloppiness). Declared `type: array` |

- Response: empty schema `{}`. **[GAP]**
- :warning: **[GAP]** Whether PUT is a full replace or a partial merge is undocumented. Since `do` and `status` are
  both required, it behaves like a replace: you must resend the full action even to change one field.

### `DELETE /profiles/{profile_id}/rules/{hostname}` — Custom Rules - Delete
Description says "Delete one or **more** custom rules", but the path takes a **single** hostname and the
declared body is an **empty object**. **[CONFLICT]** There is no documented way to bulk-delete. Probe
whether a `hostnames[]` body is accepted.
- Path: `profile_id` (string, required), `hostname` (string, required, e.g. `domain.com`)
- Content-Type: `application/x-www-form-urlencoded` with an empty schema.
- :warning: Hostnames with wildcards (`*.domain.com`) must be **URL-encoded** in the path. Not mentioned in the docs. **[GAP]**
- Response: empty schema `{}`. **[GAP]**

---

## 7. Services

### `GET /profiles/{profile_id}/services` — Services - List
"This returns services that have any kind of rule associated with it."
- Path: `profile_id` (string, required)
- Response key: **`body.services`** (array)

| Field | Type | Required | Notes |
|---|---|---|---|
| `PK` | string | yes | **service identifier**, e.g. `zoom`, `disney`, `blizzard` |
| `name` | string | yes | display name, e.g. `Battle.net` |
| `category` | string | yes | `video`, `social`, `gaming`, `tools`, `shop`, `audio` |
| `unlock_location` | string | yes | e.g. `JFK`, `SYD`, `RES_ORD` |
| `locations` | array of string | yes* | *declared required but absent from most example entries. Treat as optional* |
| `warning` | string | no | user-facing caveat |
| `action` | object | yes | `{do, status}` required, `via` optional |

### `PUT /profiles/{profile_id}/services/{service}` — Services - Modify
Create or modify a rule for a service in a profile.
- Path: `profile_id` (string, required), `service` (string, required, e.g. `zoom`)
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `do` | integer | **yes** | 0=BLOCK, 1=BYPASS, 2=SPOOF, 3=REDIRECT |
| `status` | integer | **yes** | 0 = disabled, 1 = enabled |
| `via` | string | no | `do=2` => A record or CNAME. `do=3` => 3-letter IATA proxy identifier |
| `via_v6` | string | no | `do=2` => AAAA record. No effect when `do=3` |

- Response key: **`body.services`** (array): `{do, via, status}`
- :warning: **[GAP]** No documented way to **delete** a service rule. Presumably `status=0` disables it. Whether the
  rule can be removed entirely is unstated.

### `GET /services/categories` — List Service Categories
- `security: []` (unauthenticated). No params.
- Response key: **`body.categories`** (array): `PK`, `name`, `description`, `count`. All required.

### `GET /services/categories/{category}` — List All Services
- `security: []` (unauthenticated).
- Path: `category` (string, required, e.g. `tools`)
- Response key: **`body.services`** (array): `PK`, `name`, `category`, `unlock_location` (all required).
  `locations` (array of string) and `warning` (string) are optional.
- Note: **no `action` object** here. This is the global catalogue, not a profile's service rules.

---

## 8. Endpoints / Devices

The URL segment is **`devices`**. The docs call them "Endpoints".

### `GET /devices` — List All Endpoints
- Params: none declared. Honors `X-Force-Org-Id`.
- **Undeclared sub-paths and query params (prose only)** **[SPEC]**:
  - Append **`/users`** -> `GET /devices/users`: only User-type devices.
  - Append **`/routers`** -> `GET /devices/routers`: only Router-type devices.
  - Query param **`last_activity=1`**: required to keep receiving `last_activity` and `clients` fields.
  Verbatim deprecation notice:
  > We're currently updating this API to streamline behavior and improve query performance. As part of this
  > change, the `last_activity` and `clients` fields will be removed from the response soon. In the meantime,
  > if you still need these two fields, please include the query parameter `last_activity=1` in your requests.
  :warning: Neither `last_activity` nor `clients` appears in the declared response schema at all. **[GAP]** Their
  shapes are undocumented. Don't rely on them.

- Response key: **`body.devices`** (array), plus a sibling **`body.activity`** (boolean, required).

| Field | Type | Required | Notes |
|---|---|---|---|
| `PK` | string | yes | **device primary key**, e.g. `2bmen1byrpr` |
| `device_id` | string | yes | same value as `PK` in every example |
| `ts` | integer | yes | unix ts |
| `name` | string | yes | |
| `status` | integer | yes | 0=pending, 1=active, 2=soft disabled, 3=hard disabled |
| `learn_ip` | integer | yes | 0/1 |
| `stats` | integer | no | analytics level 0/1/2 |
| `restricted` | integer | no | 0/1 |
| `desc` | string | no | |
| `icon` | string | no | e.g. `desktop-windows` |
| `ddns` | object | no | `{status, subdomain, hostname, record}`, all four required if present |
| `ddns_ext` | object | no | `{status, host}`, both required if present |
| `resolvers` | object | **yes** | `{uid, doh, dot}` required, `v4`/`v6` (arrays of string) optional |
| `legacy_ipv4` | object | no | `{resolver, status}` |
| `profile` | object | **yes** | `{PK, updated, name}`, the enforced profile |

:warning: `profile_id2` (second enforced profile) is a **write-only** field: accepted on POST/PUT but **never
appears in any response schema or example**. **[GAP]** How do you read back the second profile?

### `POST /devices` — Create Endpoint
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | **yes** | e.g. `Bobs-Phone` |
| `client_count` | **string** | **yes** | "Number of devices using this Endpoint". Typed string, example `"10"` |
| `profile_id` | string | **yes** | PK of main profile to enforce |
| `icon` | string | **yes** | device type/icon key from `GET /devices/types`, e.g. `mobile-android` |
| `profile_id2` | string | no | PK of a second profile to enforce |
| `stats` | integer | no | analytics level: 0=off, 1=basic, 2=full |
| `legacy_ipv4_status` | integer | no | 1 => generate a legacy IPv4 (and IPv6) resolver |
| `learn_ip` | integer | no | 0/1: automatic IP learning + logging |
| `restricted` | integer | no | 1 => only previously authorized IPs may query it |
| `desc` | string | no | free-text comment |
| `ddns_status` | integer | no | status of the DDNS endpoint that exposes the last-used IP |
| `ddns_subdomain` | string | no | subdomain to expose the IP on |
| `ddns_ext_status` | integer | no | status of DDNS-based IP learning |
| `ddns_ext_host` | string | no | DDNS hostname to poll for new IPs |
| `remap_device_id` | string | no | remap source device + client ID to a new device |
| `remap_client_id` | string | no | e.g. `hostname-01` |

:warning: `status` and `bump_tls` and `ctrld_custom_config` are **not** accepted on POST. PUT only.

- Response: the device object sits **directly at `body`** (not `body.devices`). **[SPEC]**
  Required: `PK`, `ts`, `name`, `stats`, `device_id`, `status`, `icon`, `restricted`, `learn_ip`, `bump_tls`,
  `desc`, `resolvers` (`uid`,`doh`,`dot`,`v4`,`v6` all required), `legacy_ipv4`, `profile`.
  Plus top-level `message` (e.g. `"Device has been added"`).
  Note the created device comes back with `status: 0` (pending).

### `PUT /devices/{device_id}` — Modify Endpoint
- Path: `device_id` (string, required): "Device/Resolver ID"
- Content-Type: `application/x-www-form-urlencoded`
- **No field is required** (no `required` array).

| Field | Type | Notes |
|---|---|---|
| `name` | string | |
| `client_count` | **string** | |
| `profile_id` | string | |
| `profile_id2` | string | **"-1 to remove"**: sentinel to detach the second profile |
| `stats` | integer | 0=off, 1=basic, 2=full |
| `legacy_ipv4_status` | integer | 1 = generate, **0 = remove existing one** |
| `learn_ip` | integer | 0/1 |
| `restricted` | integer | 0/1 |
| `bump_tls` | integer | experimental ECH support and TLS bumping. **PUT-only** |
| `desc` | string | |
| `ddns_status` | integer | 1 = enabled, 0 = disable |
| `ddns_subdomain` | string | |
| `ddns_ext_host` | string | |
| `ddns_ext_status` | integer | 0/1 |
| `status` | integer | **0 = pending, 1 = active, 2 = soft disabled, 3 = hard disabled.** PUT-only |
| `ctrld_custom_config` | string | a `ctrld` `.toml` config file to deploy. **PUT-only** |

:warning: `icon` is **not** listed as a modifiable field on PUT, though it's required on POST. **[GAP]** Can you
change a device's icon? Probe.

- Response: device object **directly at `body`** + `message` (e.g. `"Device has been updated"`). Required
  fields include `ddns` and `ddns_ext` (unlike the POST response, which requires `icon` and `bump_tls` instead).

### `DELETE /devices/{device_id}` — Delete Endpoint
- Path: `device_id` (string, required)
- No request body.
- Response: `{"body": [], "success": true, "message": "Device has been deleted"}`. `body` is an **empty array**. **[SPEC]**
- Warning **[SPEC]**: "This will break DNS on any physical gadget that uses this Device's unique DNS resolvers."

### `GET /devices/types` — List Endpoint Types
- No params.
- Response key: **`body.types`**, an **object keyed by category** (`os`, `browser`, `tv`, `router`), each
  `{name: string, icons: {<icon_key>: <label>}}`. The `router` entry additionally has `setup_url`.
  See section 1 for the full icon key list.

---

## 9. Access (Known / Authorized IPs)

All three operations live on the **same path** `/access`, differentiated by method. Honors `X-Force-Org-Id`.

### `GET /access` — List Known IPs
"List up to latest **50** IPs that were used to query against a Device (resolver)."
- **Query param** `device_id` (string, **required**): "(Required) Primary key of the device."
- Response schema is **empty `{}`**. **[GAP]** Presumably `body.access` or `body.ips`. **Probe this. The
  response key is unknown.**

### `POST /access` — Learn New IP
"Supply an array of IPs to authorize on the device. These IPs will be able to use the Legacy DNS IPv4 resolver
and have access to proxies. If this is a restricted device, then only these IPs will be able to communicate with it."
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `device_id` | string | **yes** | device PK: **in the body**, not the path or query |
| `ips[]` | array | **yes** | IPv4 or IPv6. Example `["1.2.3.4","a:b:c:d:e:f::"]` |

- Response: empty schema `{}`. **[GAP]**

### `DELETE /access` — Delete Learned IP
"Deauthorize an IP from a device. Only useful for restricted devices, or devices that have Legacy Resolvers."
- Content-Type: `application/x-www-form-urlencoded` (**a DELETE with a request body**). Some HTTP clients
  strip DELETE bodies by default. Ensure the Rust client (e.g. `reqwest`) sends it.

| Field | Type | Required | Notes |
|---|---|---|---|
| `device_id` | string | **yes** | |
| `ips[]` | array | **yes** | example `["1.2.3.4"]` |

- Response: empty schema `{}`. **[GAP]**

---

## 10. Analytics

Both are catalogue endpoints. **Both have completely empty response schemas in the docs.** **[GAP]**

### `GET /analytics/endpoints` — List Storage Regions
"Returns Analytics st[o]rage regions that can be set on the account or organization."
- No params. Feeds the `stats_endpoint` field of `POST /organizations/suborg` and `PUT /organizations`.
- Response key unknown, probably `body.endpoints`. **Probe.**
- Known values from other examples: `america`, `jfk-org01`, `ams-org01`.

### `GET /analytics/levels` — List Log Levels
"Returns Analytics log levels which can be enabled on Devices."
- No params. Feeds the `stats` field of `POST`/`PUT /devices` (0 / 1 / 2).
- Response key unknown, probably `body.levels`. **Probe.**

:warning: **[GAP]** There is **no endpoint to query analytics data itself** (query logs, counts, reports) in the
public API. These two only expose the *configuration* catalogues. If the CLI is meant to show DNS activity,
that capability is not in the documented public API.

---

## 11. Billing

All three: no params, `security: [bearerAuth]`, and **completely empty response schemas**. **[GAP]** —
every field and response key here is unknown. Probe all three.

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/billing/payments` | "Returns billing history of all payments made." |
| `GET` | `/billing/products` | "Returns all products currently activated on an account." |
| `GET` | `/billing/subscriptions` | "Returns all active and canceled subscriptions associated with an account." |

Likely response keys by convention: `body.payments`, `body.products`, `body.subscriptions`.

---

## 12. Organizations

### RESOLVED: the sub-organizations path
The `get_organizations-sub-organizations` **doc slug uses a hyphen**, but the **HTTP path it documents uses an
underscore**. Verbatim from the OpenAPI fragment:

```json
"paths": { "/organizations/sub_organizations": { "get": { ... } } }
```

> ### :white_check_mark: `GET /organizations/sub_organizations` — **underscore**, not hyphen. Definitive.

The response body key is likewise **`sub_organizations`** (underscore). Note the *create* path is a
**different, shorter word**: `POST /organizations/suborg`. Three different spellings across the API surface:
- doc slug: `sub-organizations` (hyphen)
- GET path + response key: `sub_organizations` (underscore)
- POST path: `suborg`

### `GET /organizations/organization` — View Organization Info
Note the doubled segment: the path really is `/organizations/organization`.
- No params.
- Response key: **`body.organization`** (object). All fields required:

| Field | Type |
|---|---|
| `PK` | string, org primary key, e.g. `77dvawsb` |
| `name` | string |
| `website` | string |
| `address` | string |
| `contact_email` | string |
| `date` | string, `YYYY-MM-DD` |
| `status` | integer |
| `stats_endpoint` | string, analytics storage region |
| `max_profiles` | integer |
| `max_users` | integer |
| `max_routers` | integer |
| `max_legacy_resolvers` | integer |
| `max_sub_orgs` | integer |
| `price_users` | integer |
| `price_routers` | integer |
| `members` | object `{count}` |
| `profiles` | object `{count, max}` |
| `users` | object `{count, max, price}` |
| `routers` | object `{count, max, price}` |
| `sub_organizations` | object `{count, max}` |

### `GET /organizations/members` — View Members
- No params.
- Response key: **`body.members`** (array). All fields required:

| Field | Type | Notes |
|---|---|---|
| `PK` | string | member primary key |
| `email` | string | |
| `last_active` | integer | unix ts |
| `twofa` | integer | 0/1 |
| `status` | integer | |
| `permission` | object | `{level: integer, printable: string}` (e.g. `{100, "Owner"}`, `{1, "Viewer"}`) |

:warning: **[GAP]** No endpoints to **invite, modify, or remove** members. Read-only.

### `GET /organizations/sub_organizations` — View Sub-Organizations
- No params.
- Response key: **`body.sub_organizations`** (array). Same shape as `organization` **minus**
  `max_sub_orgs`/`price_*`, **plus**:

| Extra field | Type | Notes |
|---|---|---|
| `parent_org` | object | `{name, PK}`, **required** |
| `parent_profile` | object | `{PK, updated, name}`, **optional** (absent in 2 of 3 examples). The shared/global profile |
| `twofa_req` | integer | 0/1: is 2FA required for members |
| `contact_name` | string | required |

Required per schema: `contact_name`, `stats_endpoint`, `max_legacy_resolvers`, `max_profiles`, `max_routers`,
`max_users`, `parent_org`, `twofa_req`, `contact_email`, `status`, `date`, `name`, `PK`, `members`,
`profiles`, `users`, `routers`, `sub_organizations`.
Optional: `parent_profile`, `website`, `address`.

### `POST /organizations/suborg` — Create Sub-Organization
- Content-Type: `application/x-www-form-urlencoded`

| Field | Type | Required | Notes |
|---|---|---|---|
| `name` | string | **yes** | |
| `contact_email` | string | **yes** | |
| `twofa_req` | integer | **yes** | 0 = no, 1 = yes |
| `stats_endpoint` | string | **yes** | PK of storage region: "See GET /analytics/endpoints" |
| `address` | string | no | |
| `website` | string | no | |
| `contact_name` | string | no | |
| `contact_phone` | string | no | |
| `parent_profile` | string | no | Global Profile PK to enforce on all created Devices |

- Response key: **`body.organization`** + top-level `message` (`"Sub-organization has been created."`)
- :warning: `max_users` / `max_routers` / `max_profiles` are **not** settable at creation: the new org comes back
  with server-chosen defaults (example: `max_users: 10`, `max_routers: 1`, `max_profiles: 100`). Use
  `PUT /organizations` to change seats, except see the conflict below.

### `PUT /organizations` — Modify Organization
- **No path parameter.** The path is bare `/organizations`. **[GAP]** *Which* org does this modify? The
  page title says "Modify **Sub**-Organization", but there is no ID anywhere. Presumably it targets the org
  tied to the token, or the one named by `X-Force-Org-Id`. **This is unresolvable from the docs and must be
  probed before implementing.**
- Content-Type: `application/x-www-form-urlencoded`
- **No field is required.**

| Field | Type | Notes |
|---|---|---|
| `name` | string | |
| `contact_email` | string | |
| `twofa_req` | integer | 0/1 |
| `stats_endpoint` | string | |
| `address` | string | |
| `website` | string | |
| `contact_name` | string | |
| `contact_phone` | string | |
| `parent_profile` | string | |

- :warning: **[CONFLICT] Seats.** The page's prose is explicitly about seats:
  > **Billable Events** — Modifying `max_users` and `max_routers` is a billable event. If you increase your
  > commitment, you will be charged a prorated difference from your last commitment to the new one. New amount
  > will be rebilled subsequently. Reducing the commitment will update your future rebill amount.

  ...but **`max_users` and `max_routers` are absent from the declared request schema.** The summary even says
  "including seats". Either the schema is incomplete (most likely) or seats can't be set here. **Probe. Be
  careful: getting this wrong triggers real billing.**

- Response key: **`body.organization`** + `message` (`"Organization has been updated."`)
- :warning: **[GAP]** No endpoint to **delete** a sub-organization.

---

## 13. Account / Misc

### `GET /users` — User Data
"Returns all relevant account information of a Control account."
- No params. Declares `security: []` but see the **[CONFLICT]** in section 0: it needs a token.
- Response: object **directly at `body`** (not `body.users`). All fields required:

| Field | Type | Example |
|---|---|---|
| `PK` | string | `4034dvb354` |
| `email` | string | `username@email.com` |
| `date` | string | `2022-11-27` (`YYYY-MM-DD`) |
| `status` | integer | `1` |
| `email_status` | integer | `1` |
| `last_active` | integer | `1669595046` (unix ts) |
| `proxy_access` | integer | `1` |
| `twofa` | integer | `0` |

This is the natural "am I authenticated?" / `whoami` endpoint for the CLI.

### `GET /network` — Network Stats
"Returns network stats on available services in different POPs."
- `security: []` (unauthenticated). No params.
- Response schema **empty `{}`**. **[GAP]** Probe.

### `GET /ip` — Return IP
"Returns current IP and datacenter that was used to handle the API request."
- `security: []` (unauthenticated). No params.
- Response: object **directly at `body`**. All required:

| Field | Type | Example |
|---|---|---|
| `ip` | string | `66.207.0.0` |
| `type` | string | `v4` (presumably `v6` too) |
| `org` | string | `Beanfield Technologies` |
| `country` | string | `CA` |
| `handler` | string | `dva-h01` |

### `GET /proxies` — List Proxies
"Returns list of usable proxies that traffic can be redirected through."
- Requires auth. No params. Tagged `Profiles` (not `Misc`).
- Response schema **empty `{}`**. **[GAP]** **This is a high-priority probe**: it is the authoritative
  source for every legal `via` value when `do=3`. Response key is probably `body.proxies`. Known values from
  examples elsewhere: `JFK`, `YYZ`, `BRU`, `SYD`, `LAX`, `ADL`, `RES_ORD`, `LOCAL`.

---

## 14. Complete operation index (46)

| # | Method | Path | Response key |
|---|---|---|---|
| 1 | GET | `/profiles` | `body.profiles` |
| 2 | POST | `/profiles` | *(empty schema)* |
| 3 | PUT | `/profiles/{profile_id}` | *(empty schema)* |
| 4 | DELETE | `/profiles/{profile_id}` | *(empty schema)* |
| 5 | GET | `/profiles/options` | `body.options` |
| 6 | PUT | `/profiles/{profile_id}/options/{name}` | *(empty schema)* |
| 7 | GET | `/profiles/{profile_id}/default` | `body.default` |
| 8 | PUT | `/profiles/{profile_id}/default` | `body.default` |
| 9 | GET | `/profiles/{profile_id}/filters` | `body.filters` |
| 10 | GET | `/profiles/{profile_id}/filters/external` | *(empty schema)* |
| 11 | PUT | `/profiles/{profile_id}/filters` | `body.filters` (JSON body!) |
| 12 | PUT | `/profiles/{profile_id}/filters/filter/{filter}` | `body.filters` |
| 13 | GET | `/profiles/{profile_id}/groups` | `body.groups` |
| 14 | POST | `/profiles/{profile_id}/groups` | `body.groups` |
| 15 | PUT | `/profiles/{profile_id}/groups/{folder}` | *(empty schema)* |
| 16 | DELETE | `/profiles/{profile_id}/groups/{folder}` | *(empty schema)* |
| 17 | GET | `/profiles/{profile_id}/rules/{folder_id}` | `body.rules` |
| 18 | POST | `/profiles/{profile_id}/rules` | `body.rules` |
| 19 | PUT | `/profiles/{profile_id}/rules` | *(empty schema)* |
| 20 | DELETE | `/profiles/{profile_id}/rules/{hostname}` | *(empty schema)* |
| 21 | GET | `/profiles/{profile_id}/services` | `body.services` |
| 22 | PUT | `/profiles/{profile_id}/services/{service}` | `body.services` |
| 23 | GET | `/services/categories` | `body.categories` |
| 24 | GET | `/services/categories/{category}` | `body.services` |
| 25 | GET | `/devices` | `body.devices` + `body.activity` |
| 26 | POST | `/devices` | `body` (object) |
| 27 | PUT | `/devices/{device_id}` | `body` (object) |
| 28 | DELETE | `/devices/{device_id}` | `body` = `[]` |
| 29 | GET | `/devices/types` | `body.types` (map) |
| 30 | GET | `/access` | *(empty schema)* |
| 31 | POST | `/access` | *(empty schema)* |
| 32 | DELETE | `/access` | *(empty schema)* |
| 33 | GET | `/analytics/endpoints` | *(empty schema)* |
| 34 | GET | `/analytics/levels` | *(empty schema)* |
| 35 | GET | `/billing/payments` | *(empty schema)* |
| 36 | GET | `/billing/products` | *(empty schema)* |
| 37 | GET | `/billing/subscriptions` | *(empty schema)* |
| 38 | GET | `/organizations/organization` | `body.organization` |
| 39 | GET | `/organizations/members` | `body.members` |
| 40 | GET | `/organizations/sub_organizations` | `body.sub_organizations` |
| 41 | POST | `/organizations/suborg` | `body.organization` |
| 42 | PUT | `/organizations` | `body.organization` |
| 43 | GET | `/users` | `body` (object) |
| 44 | GET | `/network` | *(empty schema)* |
| 45 | GET | `/ip` | `body` (object) |
| 46 | GET | `/proxies` | *(empty schema)* |

Undocumented-but-mentioned extras: `GET /devices/users`, `GET /devices/routers`, `GET /devices?last_activity=1`.

---

## 15. Probe list — what to verify against the live API before shipping

Ordered by risk.

> **Status 2026-07-11: resolved**, except #6 (org seats, untestable on a personal account, :warning:
> billable), #14 (no 429 ever observed), and the low-stakes #15/#16/#18. Findings live in
> [read-verification.md](read-verification.md) and [write-verification.md](write-verification.md),
> including two hazards this list never anticipated: the server silently drops form variables past
> ~1001 (a 1000-hostname batch 200s and stores 999), and the 10,000 rules/profile cap rejects
> crossing batches atomically.

1. **Array form encoding.** `hostnames[]=a&hostnames[]=b` (repeated) vs `hostnames[0]=a` (indexed). Blocks
   all rule creation and all `/access` writes.
2. **`GET /proxies` response shape.** Authoritative `via` values for `do=3`. Response key unknown.
3. **`GET /access` response shape.** Response key unknown. Only bound is "up to 50".
4. **Filter mode suffixes.** Is `porn_strict` / `ads_small` a real wire format? What's the grammar? What does
   `GET /profiles/{id}/filters` actually return for a filter in strict mode: `status: 2`? A different `PK`?
5. **`PUT /profiles/{id}/filters` response shape**: array-of-string (schema) or map (example)?
6. **`PUT /organizations` targeting + seats.** No org ID in the path. Are `max_users` / `max_routers`
   accepted? :warning: **Billable: test on a throwaway sub-org.**
7. **`DELETE /profiles/{id}/groups/{folder}` body.** Does it really require `name`/`do`/`via`/`status`, or is
   the docs' body a copy-paste artifact? Send none first.
8. **Action-less folders.** `POST /groups` marks `do` required, but action-less folders demonstrably exist.
   What value means "no action"?
9. **Empty-schema write responses** (POST/PUT/DELETE profiles, options, rules, groups, access). Capture the
   real bodies, especially `POST /profiles`, which must return the new PK.
10. **Billing endpoint shapes** (3 endpoints, zero documented fields).
11. **`GET /analytics/endpoints` and `/analytics/levels` shapes.**
12. **`GET /network` shape.**
13. **`GET /users` auth.** Confirm it 401s without a token (schema says `security: []`, prose says otherwise).
14. **Rate limits.** Watch for `429` and any `X-RateLimit-*` / `Retry-After` headers. Wholly undocumented.
15. **`icon` on `PUT /devices/{id}`**: accepted or not?
16. **`profile_id2` read-back**: where does the second enforced profile appear in responses?
17. **Wildcard hostname URL-encoding** in `DELETE /profiles/{id}/rules/{hostname}` (`*.domain.com`).
18. **`lock_status` values** on `PUT /profiles/{id}`.
19. **`GET /profiles/{id}/filters/external` shape.**
20. **Error code table.** Only `40003` is documented. Collect real codes as you go.
