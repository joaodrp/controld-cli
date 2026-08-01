# API hazards

The Control D API is unversioned. Everything below was verified live and contradicts their docs or
spec. Read this before touching `src/api/` or any write path. Evidence:
[`read-verification.md`](read-verification.md) (read path) and
[`write-verification.md`](write-verification.md) (mutations). **If the live API
ever contradicts this list, update the reference doc and this index.**

- **Missing `action.do` is an error, never a default**: defaulting it silently creates a BLOCK
  rule.
- **Auth failures return HTTP 400, not 401**: classify on `error.code`, never on HTTP status.
- **Three envelope shapes**, and `body` becomes `[]` on error — deserialize `body` as `Value`,
  unwrap per operation.
- **`GET /profiles/{id}/rules` must omit the folder segment**: the documented `folder_id=0`
  404s. The segment-less path returns **root rules only**, but a foldered rule is invisible on it.
  Seeing every rule in a profile needs one `GET /rules/{folder_id}` per folder on top of it.
- **`content-type: application/json` can carry a 0-byte body.**
- **Error messages can be multi-line dumps**: never parse them.
- **`error.code` is a coarse bucket**: classify on its 3-digit HTTP prefix
  ([`error-codes.md`](error-codes.md#classify-on-the-prefix-not-the-code)).
- **Filter level names cannot be constructed**: read `levels[]`.
- **Writes are form-encoded and the server silently drops form variables past ~1001** — chunk at
  500, scalars first, then **re-fetch and verify the full desired state** (action, enabled,
  `via`, `via6`, folder). Client caps: 500 hostnames, 50 IPs, and 10,000 rules per profile. There's
  no bulk delete, so percent-encode hostnames into DELETE paths (`*` -> `%2A`).
- **`body: []` is not an error marker**: successful deletes return it too. Branch on
  `success`/`error` only.
- **`DELETE` of a non-matching hostname returns `success: true`** (`"Custom rule(s) deleted"`):
  a silent no-op. Delete success proves nothing. Check existence before, not after.
- **`PUT /rules` never upserts**: an unknown hostname 400s `Custom Rule does not exist`, and
  target matching is case-sensitive while `PK`s store case-preserved, so pre-check existence and
  resolve case before writing.
- **Bad paths never 404.** An unknown top-level path returns *"This token does not have access to
  this endpoint"*, making a typo indistinguishable from a plan restriction. Worse, `/devices/{id}`
  **swallows trailing segments**: any `/devices/{id}/<anything>` returns 200 with the device, so a
  200 there proves nothing.
