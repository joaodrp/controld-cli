# OpenAPI Spec: Provenance & Validation

**[`controld-openapi.json`](controld-openapi.json)**: Control D Public API 1.0.1, OpenAPI 3.0.1.
35 paths, 46 operations. Refresh: `./scripts/fetch-spec.sh`.

## Where it came from

Control D publish no downloadable spec. The file their docs load is permission-gated:

```
/controld/api-next/v2/branches/1.0/apis/control-d.json  -> 403
```

But **every rendered reference page embeds the complete spec** in its payload, at
`.data.api.schema`. `fetch-spec.sh` enumerates the endpoint pages from the docs sidebar and extracts it.

**Why we trust it:** all **46 endpoint pages ship a byte-identical spec** (SHA-256 over canonicalised
JSON). The script asserts this every run: a stale page fails the fetch instead of corrupting the spec.

**Rejected sources:** `controld-mcp`'s spec (third-party *reconstruction*), `llms.txt` (lists 5 of 46),
`sitemap.xml` (lists 6).

## Validation

`npx @redocly/cli lint` -> **2 errors, 146 warnings**. Structurally valid, with real defects.

| Error | Meaning |
| --- | --- |
| `no-identical-paths`: `/rules/{folder_id}` (GET) vs `/rules/{hostname}` (DELETE) | Cosmetic, but the same URL slot means *folder id* when reading and *hostname* when deleting. |
| `no-schema-type-mismatch`: `profiles[].profile.da` | **Load-bearing.** `da` is typed `array` *and* carries object `properties`, Control D's own spec encoding a real wart: the default rule is `[]` until modified, then an object. **Tolerate both.** |

Warnings worth knowing:

- **All 46 operations lack an `operationId`**: blocks clean codegen, and we assign our own.
- **No operation documents any error response**: the error envelope exists only in prose.
- 40 examples fail to validate against their own schemas. Two are signal:
  - *"`action` requires `do`"*: the spec marks `do` required, yet real responses **omit it**.
    `controld-go` reads a missing `do` as `0` = **BLOCK**. Treat missing `do` as an error.
  - *"type must be array"*: the `da` wart again.

## Content types

The spec settles this: **18 writes are `application/x-www-form-urlencoded`, and exactly one
(`PUT /profiles/{id}/filters`) is `application/json`.**

Live, the API **ignores `Content-Type` and sniffs the body**, undocumented, so we send what the spec
declares ([D16](../decisions.md#d16--documented-surface-only)).

## What the spec gets wrong or omits

Verified against the live API: see [read-verification](read-verification.md) and
[write-verification](write-verification.md):

- `body` has **three** shapes, not one (`/users` and `/ip` are flat, and `/network` has siblings).
- Auth failures are **HTTP 400**, not 401.
- `GET /profiles/{id}/rules` (folder omitted) is **absent from `paths`**, and it's the form you need,
  because the documented `folder_id=0` **404s**.
- 12 operations have an **empty response schema**, including `GET /proxies`, the authoritative list
  of legal REDIRECT targets.
- **Rate limits and pagination: entirely undocumented.**
- **Live endpoints exist outside the 35 paths.** `/endpointschedules` (full CRUD) and `/schedules`
  are absent from the spec and from the API reference, yet both answer on a personal account. The
  spec is a subset of the API, not a description of it.
- Documented request fields can have **undocumented response counterparts**: `profile_id2` is
  accepted on device writes and returned as `profile2`, which appears in no schema or example.

### Finding what the spec omits

The dashboard is a Next.js app whose chunks embed every API path it calls. Fetch
`https://controld.com/dashboard`, download the `/_next/static/chunks/*.js` it references, and grep
for path literals. Undocumented endpoints appear directly, method and body shape attached.
`/endpointschedules` turned up this way. Hand-probing cannot match it, because the API defeats
guesswork — see
[bad paths never say "not found"](read-verification.md#bad-paths-never-say-not-found-2026-08-01).

Bundle-derived paths are **evidence, not license**: undocumented surface stays out of typed
commands ([D16](../decisions.md#d16--documented-surface-only)).

## Versioning

> *"The API has no versioning... breaking changes can be introduced without warning."*

A design input, not a footnote. It justifies `cdctl api` ([D9](../decisions.md#d9--cdctl-api-separately-gateable-get-by-default)) and CI that re-fetches the spec and
fails on drift.
