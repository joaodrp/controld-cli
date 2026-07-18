# Testing

How the suite is layered, what it must protect, and the rules for running anything against the
real API.

## Test layers

| Layer | Tool | What |
| --- | --- | --- |
| Envelope/models | `serde` on fixtures | Every shape hazard, straight from **real captured payloads** — no HTTP server needed |
| Client behavior | `wiremock` | Retry (GET-only, exactly-once writes), origin rules, the 0-byte JSON 500, headers |
| Command contract | `assert_cmd` | Exit codes; stdout clean on error |
| Snapshots | `insta` | `--help`, JSON shapes |
| Live | opt-in suite | Runs only with `CONTROLD_LIVE_TESTS=1` (plus `CONTROLD_API_TOKEN`); isolated to a randomized profile (below) |

Exit codes and the retryable set are **public API** ([D5](decisions.md#d5--nine-exit-codes-exactly-one-retryable)). Test
them like it. Model types deserialize leniently in the binary; fixture tests use
`deny_unknown_fields`, so an API field addition fails tests instead of passing silently — the
drift tripwire for an unversioned API.

## Fixtures

`tests/fixtures/api/` holds **real, sanitized API responses** — prefer them to hand-written
mocks; they carry every hazard in the [API-hazards index](reference/hazards.md). Sanitize any
new fixture: no emails, device names, real domains, public IPs, or account PKs.

## Live-test isolation

The live suite (`tests/live.rs`) runs only with `CONTROLD_LIVE_TESTS=1` set (plus
`CONTROLD_API_TOKEN`) — plain `cargo test` and CI never run it, and it never runs unattended.

Profiles are the isolation boundary — rules, folders, the default rule, filters, services, and
options are all profile-scoped, so a run that stays inside its own profile is safe on **any**
account, not just a throwaway.

- **Setup:** every mutation happens inside a fresh `cdctl-test-<timestamp>-<nonce>` profile,
  created and deleted by the run; each run gets a fresh 10,000-rule quota. **Never touch
  pre-existing profiles** without the user saying so — the account behind the token may be
  someone's real account.
- **Teardown:** delete the profile; its contents go with it. Devices pointed at the test profile
  are deleted **before** the profile — deleting a profile out from under a device is unverified.
- **Leaks:** the prefix makes crashed-run leftovers identifiable; the suite sweeps stale
  `cdctl-test-*` profiles by name + age on start (plans cap profile counts, so accumulated leaks
  would eventually fail setup itself).
- **Limits:** account-scoped surfaces (devices, access, proxy, account, billing, network) cannot
  be profile-isolated; their tests (v0.4) use the same `cdctl-test-` naming on the resources they
  create.
