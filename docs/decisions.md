# Decisions

Verified behavior in [reference/](reference/). Each entry records its own reasoning and the cost
accepted.

## D1 — Crate `controld-cli`, binary `cdctl`

Control D's DNS daemon is already `ctrld`. Ours is a different tool and will sit
beside it on `PATH`.

- :x: `controld`: a trailing `-d` means *daemon* in Unix (`sshd`, `systemd`, `cloudflared`, `ctrld`).
  Two daemon-looking binaries neither a human nor an agent can tell apart. Also squats the name
  Control D would want.
- :x: `ctrld-cli`: names the *daemon* as its parent (but `ctrld` already has a CLI), and prefix-collides
  with `ctrld` in tab-completion.
- :white_check_mark: `cdctl`: `*ctl` (kubectl, systemctl) reads as *client tool*. No prefix collision.

**Cost accepted:** cryptic, unguessable. Discoverability rests on docs. Lead the README with the
`ctrld` disambiguation.

Top-level verbs that read as daemon ops (`status`, `start`, `log`) stay reserved: colliding with
`ctrld`'s surface would recreate the confusion this decision exists to avoid.

## D2 — The CLI is the stability layer; own the output schema

The API is unversioned ("breaking changes... without warning"), returns 400 for auth failures, encodes
actions as bare ints, and has three envelope shapes. **Never pass that through.** Unwrap, map ints to
names, rename `PK`, normalize timestamps to RFC-3339.

**Cost accepted:** new API fields need a release. D9 (`cdctl api`) is the pressure valve that makes this affordable.

## D3 — No TTY-based format switching

`--json` is explicit. Piping changes *rendering* (color, pager), never *representation*, as `gh` and
`kubectl` do. Agents use `--json` or `CONTROLD_OUTPUT=json`.

JSON output is always **pretty-printed** (2-space indent, stable key order, one trailing
newline), identical on a TTY, in a pipe, and in error documents on stderr. Compact output can
arrive later as a `--compact` flag. A formatting flag is additive.

Lists print in full: **no default `--limit`**. Silent truncation corrupts diff and sync flows.
Agents trim with `--fields` or `jq`. A limit flag stays additive if demand appears.

`--json` is **boolean**. Field selection is a separate `--fields <a,b>` (implies `--json`). An
optional-value `--json [fields]` swallows the next positional in clap. **No `--jq` in v1**: adding a
flag later is non-breaking, and shipping one means embedding a jq engine (`jaq`). Pipe to `jq` until
demand proves the dependency.

**No machine-readable command manifest in v1.** `--help`, `cdctl reference` (Markdown), and
`AGENTS.md` cover human and agent discovery without publishing another schema. Keep command metadata
centralized and derivable so a post-v1 release can generate a **versioned** machine-readable spec from
the same source as clap and `reference`. Add it only for a concrete consumer, such as a policy
validator, MCP adapter, or tool generator. Never maintain a second handwritten command contract.

## D4 — Errors: JSON on stderr, stable slugs, explicit `retryable`

stdout carries **only** data. Errors -> stderr. The envelope below is what **JSON mode** emits.
Human mode renders the same fields as a single `error: ...` line. Exit codes are identical in both.

```jsonc
{
  "error": {
    "code": "auth.invalid_token",       // stable, CLI-owned — agents match on this
    "message": "Invalid session...",    // ours, rendered
    "upstream": {                       // theirs, verbatim
      "code": 40001,
      "http_status": 400,
      "message": "Invalid session, please login again."
    },
    "retryable": false,
    "retry_after": null,                // seconds from Retry-After; null when absent or unparseable
    "details": null,                    // structured payload on aggregate errors
    "hint": "Run `cdctl auth login`."
  }
}
```

One field, `retryable`, removes a class of agent bugs. Its sibling `retry_after` carries the
parsed `Retry-After` in seconds (`null` when the response has none, or the header is
unparseable, in which case `--debug` says so). Agents never re-derive backoff.

Human mode stays **one line**, whatever upstream leaks: CR/LF/TAB collapse to spaces (whitespace
runs collapse to one, since stripping a tab would glue words together), and every other C0/DEL control
(ANSI escapes included) is **stripped** — a message never gets to rewrite the terminal. The verbatim text survives in JSON `upstream.message`. `--debug` renders it
**JSON-escaped after token redaction**: verbatim semantically, never raw terminal bytes.

**`upstream` is `null` or `{code, http_status, message}`**: `null` on client-side errors (bad
flag combination, unreadable file, plan refusals) *and at the top level of aggregate errors*,
whose target-specific upstream data sits with each target. Members are individually nullable (the
0-byte 500 yields `{"code": null, "http_status": 500, "message": null}`).

**`details` is `null` on ordinary errors** and a discriminated object on aggregate ones. The key
is always present, and inside each variant every key is too, `null` where not applicable. The
contract is the complete envelope, not fragments:

```jsonc
{
  "error": {
    "code": "write.partial_failure",    // stable aggregate slug
    "message": "1 of 2 rules failed",   // ours
    "upstream": null,                   // per-target upstream sits inside targets
    "retryable": true,                  // the aggregation verdict (true <=> exit 8)
    "retry_after": null,
    "details": {
      "kind": "multi_target",
      "targets": [                      // ordered; code is the stable CLI slug, NEVER the numeric upstream code
        {
          "target": "a.com",
          "outcome": "landed",
          "code": null,
          "retryable": null,
          "upstream": null
        },
        {
          "target": "b.com",
          "outcome": "failed",
          "code": "upstream.error",
          "retryable": true,
          "upstream": {
            "code": 50001,
            "http_status": 500,
            "message": "..."
          }
        }
      ],
      "retry_argv": [                   // resolved ids, never names/defaults
        "cdctl", "rule", "create", "b.com",
        "--action", "block",
        "--profile", "123456abcdefg"
      ]
    },
    "hint": "Re-run retry_argv; it covers only the failed targets."
  }
}

{
  "error": {
    "code": "rule.collision",
    "message": "2 hostnames exist in other folders",
    "upstream": null,
    "retryable": false,
    "retry_after": null,
    "details": {
      "kind": "collisions",             // nothing landed, nothing to retry — no retry_argv
      "collisions": [
        {"hostname": "x.example", "folder_id": 3, "folder": "Work"}
      ]
    },
    "hint": "Remove them from the file, or delete/move the live rules first."
  }
}

{
  "error": {
    "code": "rule.unconvergeable",
    "message": "1 rule cannot reach the desired state",
    "upstream": null,
    "retryable": false,                 // the API cannot express the transition
    "retry_after": null,
    "details": {
      "kind": "unconvergeable",         // every entry key always present
      "rules": [
        {
          "hostname": "v6.example",
          "reason": "via6-clear",
          "current_via6": "2001:db8::1",
          "desired_via6": null
        }
      ]
    },
    "hint": "command-specific — see rule-import semantics in commands.md"
  }
}
```

The `unconvergeable` variant covers desired states the API cannot reach
([write-verification](reference/write-verification.md#via_v6-cannot-be-cleared-probed-2026-07-11-rule-was-do2-via1920210-via_v62001db81)). `reason` is a stable slug and the
`current_*`/`desired_*` members are `null` when a future reason doesn't use them.

`outcome` is `"landed" | "failed" | "skipped"` (skipped = not attempted after an abort).

`retry_argv` is built so the retry cannot go astray:

- **An argv array, not a string**: JSON stays shell-neutral. No single quoting scheme works for
  POSIX, PowerShell, *and* `cmd.exe`, and agents should not parse shell syntax. Human mode may
  render a convenience string beside it.
- **Resolved opaque ids, never names or ambient defaults**: `--profile <id>` (plus
  `--device <id>` where applicable) is always present, even when the original run used a name or
  the config default. A changed default must not redirect the retry (the D8 hazard).
- **Deletes carry `--yes`** so the retry stays non-interactive. Credentials never appear.

**Aggregation:** exit `8` only when *every* failed target is retryable. A batch containing any
terminal failure exits with the shared terminal code, or `1` when mixed. The per-target fields
stay authoritative either way. stdout stays empty. JSON mode emits exactly **one** document on
stderr.

## D4b — Classify on the *prefix* of `error.code`, not a table

Only **five** distinct codes are known (`40001`, `40002`, `40003`, `40201`, `40401`), and `40003` alone
carries **a still-growing list of unrelated meanings**, since every probe session adds to the observed
table in [error-codes.md](reference/error-codes.md#observed-codes--the-entire-known-universe). A lookup table would be worthless.

Control D document the structure: *"first 3 digits match the HTTP status."* So classify on the prefix:
it's total, and a code we've never seen still resolves correctly.

**The trap:** `400` is overloaded. Auth runs before routing, so a bad token is `400`/`40001`, not 401.
Special-case it. Full table: [reference/error-codes.md](reference/error-codes.md#observed-codes--the-entire-known-universe).

## D5 — Nine exit codes, exactly **one** retryable

| | | | |
| --- | --- | --- | --- |
| `0` success *(incl. empty results)* | `1` generic | `2` usage *(free from clap)* | `3` not found |
| `4` auth *(matches `gh`)* | `5` forbidden / plan / scope | `6` conflict | `7` confirmation required |
| `8` **retryable** — rate limit, 5xx, network | `130` SIGINT | `141` SIGPIPE *(Unix)* | |

An earlier draft split rate-limit/5xx/network into three codes. That's **diagnostic, not actionable**:
an agent does the same thing for all three. An exit status is one byte and can't carry a message. The
reason belongs in the error envelope.

**The contract: exit 8 is retryable, everything else is terminal.** Codes 9-19 reserved, append-only.

`141` is Unix's SIGPIPE death: the default disposition is restored at startup, so
`cdctl reference | head` dies quietly the traditional way. Rust's default (a write-error panic,
exit 101) would leak an undocumented code. Sockets are unaffected (`MSG_NOSIGNAL`).

**Scope violations fail loudly** (exit 5): never silently filter results, as `flyctl` does.

## D6 — Auth: env or stdin. No `--token` flag. No keyring.

`CONTROLD_API_TOKEN` or `cdctl auth login --token-stdin`. **No `--token` flag**: argv is world-readable
via `ps`. Config at `$XDG_CONFIG_HOME/cdctl/config.toml`, mode `0600`, token wrapped in `secrecy`.
The dir matches the binary (`gh` -> `gh/`), and D1's no-squatting argument applies to `controld/` too.

**No keyring:** breaks headless/CI/agent use and drags dbus into musl builds. Agents are a primary
audience, so it's the wrong trade. (`gh`/`aws`/`stripe` all do env + file.)

**The persistence contract (v1):**

```toml
# $XDG_CONFIG_HOME/cdctl/config.toml (Windows: %APPDATA%\cdctl\config.toml)
current_context = "personal"  # optional — "personal" is the default

[contexts.personal]
token = "api.xxxx"           # plaintext — `secrecy` guards logs and debug output, NOT the disk
default_profile = "Home"   # optional

[contexts.acme]            # an org context slots in later (D15) with no migration
token = "api.xxxx"
org = "<org-id>"
```

Precedence: `CONTROLD_API_TOKEN` > active context's `token` > error. Writes go through a
same-directory tempfile + atomic rename. The directory is created `0700`, a symlinked config file is
refused, and an existing group/world-readable file draws a warning. Windows relies on `%APPDATA%`'s
user-scoped ACLs: no Unix mode bits are attempted. **The token is not encrypted at rest**. The file
mode is the guard, and the docs must never imply otherwise.

Tokens are **dashboard-issued only** — no token API exists — so `auth login` cannot do an OAuth flow.

**Secret managers integrate with zero code.** Env beats config, so 1Password's
`op run` injects `CONTROLD_API_TOKEN` from an `op://` reference per invocation, and
`op read ... | cdctl auth login --token-stdin` covers persistent setup. The same works for any
manager that can export an env var. This is the designed path: no keyring, no plugin API.

## D7 — Resolve auth lazily

Resolving the token at the top of `run()` makes `completions` and `reference` fail with "not
authenticated," so you can't install completions without a token. Resolve inside the handlers that
make requests.

The four operations the spec marks `security: []` (`service categories`, `service catalog`,
`network`, `ip`) are **attempted without a token** when none is configured: no `Authorization`
header at all. If the API answers `40001` anyway, it classifies as auth (exit 4) like anywhere else.
With a token configured, it is sent.

## D8 — Tiered confirmation

Prompt on TTY. **Fail loudly** (exit 7) in non-TTY without `--yes`. Severe ops (`profile`/`device
delete`) need `--confirm=<name>`. **`--yes` is ignored when the target is implicit.** Explicit means
the profile came from `--profile` **or `CONTROLD_PROFILE`**, both deliberate, per-invocation or
per-session choices. Only the config file's `default_profile` is implicit: it's ambient state the
caller may have forgotten, and deleting into the wrong profile because it was the default is the
failure mode worth designing out. Agents set the env once and `--yes` works normally.

`--force` never bypasses a prompt: bypassing is `--yes`'s job alone. A `--force*` flag accepts one
named risk (`--force-delete-first` covers the protection gap), and future flags keep that split.

## D9 — `cdctl api`, separately gateable, GET by default

The escape hatch. Without it the CLI *becomes the blocker* when the API moves, and users fall back to
`curl` with the token in argv.

**But a raw passthrough defeats agent allowlists**: permit `cdctl` and you've permitted every DELETE.
So: a **distinct verb** (sandboxes can deny `Bash(cdctl api:*)` while allowing `Bash(cdctl:*)`),
**GET by default**, non-GET requires `-X` **and** `--yes`, raw output documented as unstable.

**Origin rules: the token never leaves the pinned origin.** `cdctl api` accepts **relative paths
only**: absolute URLs, scheme-relative forms, and authority/userinfo components are rejected at
parse time. Redirects are followed same-origin only. `Authorization` is never forwarded across
origins. The base-URL override (`CONTROLD_API_URL`) serves tests and path-routing gateways: it
must be an http(s) URL with a host (rejected at construction otherwise), a path prefix on it is
part of the pinned target (preserved on every request, with dot-segment escapes rejected), and
when it points anywhere but the real origin, `cdctl` refuses to attach a stored token unless
`CONTROLD_UNSAFE_BASE_URL=1` is also set. Non-2xx responses classify through the standard error
rules. GET retries apply, writes never retry.

**Encoding is explicit, never sniffed** (D16). Repeated `-F k=v` builds a form body with **literal
keys** (`hostnames[]=` works as typed). `--input -` sends stdin verbatim as JSON. **Both body
forms require a non-GET `-X <method>`** (a GET never carries a body here), and the two are mutually
exclusive. Query strings ride in the path. Grammar:
[commands.md](commands.md#cdctl-api-request-encoding).

## D9b — Flags primary; JSON only via stdin

Microsoft [benchmarked flags vs JSON payloads for LLM callers](https://developer.microsoft.com/blog/dont-rewrite-your-cli-for-agents)
(July 2026): **flags won 5/5 on correctness** and used **4-11x fewer tokens**. Models emit valid
JSON that then dies on *shell escaping*.

**Never JSON-in-argv.** `--input -` for stdin.

## D10 — Never make users type magic integers

One flag group, reused by `rule`, `folder`, `service`, `profile default`:

```
--action block|bypass|spoof|redirect     # not --do 0
--via <ip|cname|PROXY>
--enabled | --disabled                   # not --status 1
```

`--enabled/--disabled` is **orthogonal** to `--action`. The CLI's biggest ergonomic win over `curl`.

## D11 — Form-encoded, `hostnames[]`, resolved live

Send what the spec declares: form for the 18 writes, JSON for `PUT .../filters`. Arrays as
**`hostnames[]=a&hostnames[]=b`**.

**The "contradiction" never existed**: bracket and indexed both work. The *bare repeat* fails, and
that's what Go/Python emit by default. Root cause: **the backend is PHP** (it leaked a `print_r()`
dump into an error). Details: [reference/write-verification.md](reference/write-verification.md#array-encoding--there-was-never-a-contradiction).

Consequences:

- Upstream messages can be **multi-line**: never parse them.
- **The server parses at most ~1001 form variables and silently drops the rest**: a 1000-hostname
  batch returns 200 and stores 999, or stores all 1000 with a *defaulted* `status`.
- So: chunk imports at **500**, put scalar params first, and **verify the full desired state**:
  re-fetch and assert every written hostname is present with the intended action, enabled state,
  `via`, `via6`, and folder. The write response is a one-entry summary that can't reveal partials,
  and a bare count check can be masked by concurrent changes.
- Over-sized batches and batches crossing the **10,000 rules/profile cap** fail atomically.
- The ceiling binds **every** variadic write, not just import. Typed multi-target commands are
  capped client-side: **500 hostnames** (`rule create/update`), **50 IPs** (`access add/remove`,
  the read-back window is 50 and the `ips[]` ceiling is unprobed).
- Every multi-target write yields a **per-target outcome**: read-back where the window allows
  (rules, `access add`), one request per target where it doesn't (`rule delete`, `access remove`,
  the 50-entry `GET /access` window cannot prove an older IP's deletion).
- **No bulk delete exists.** Percent-encode hostnames into DELETE paths (`*` -> `%2A`).

## D12 — Rate limiting: reactive, not predictive

No headers, no 429 ever observed (~10 req/s x 60). Can't pre-limit against an unverified number.
Honour `Retry-After` if present, exponential backoff with jitter, **auto-retry idempotent GETs only,
never writes**.

**The retry loop is ours, not reqwest's.** The client is built with `retry::never()`. `cdctl` owns a
GET-only loop: `Retry-After` (integer *and* HTTP-date), full-jitter exponential backoff, caps of
3 attempts / 30 s elapsed, `--no-retry` disables. reqwest 0.13's built-in retry replays immediately —
no backoff, no `Retry-After` — and its default policy is not GET-only, so it cannot implement this
contract. Tests assert every write endpoint receives **exactly one** request under 429/5xx/timeout.

Every request runs under a timeout (30 s total, 10 s connect, `--timeout` overrides) because a
hang is worse than a fast failure. Each retry attempt logs to stderr. A silent backoff is
indistinguishable from a hang. A **write that times out may still have landed**: it is never
retried, the error says so, and the hint directs to re-fetch state (multi-target writes resolve
this themselves via read-back).

## D13 — Rust stack *(compile-verified July 2026)*

`clap` + `clap_complete`, `reqwest` (**async**, rustls, the `form` feature, no longer a default
feature, and every write needs it), `tokio`, `serde`/`serde_json`, `thiserror`, `comfy-table`,
`etcetera` (XDG on macOS too), `secrecy`, `toml`, `httpdate` (`Retry-After` HTTP-dates),
`fastrand` (backoff jitter), `tempfile` (atomic config writes). `anstream` + `owo-colors` arrive
with the first colored output.
Dev: `wiremock`, `assert_cmd`, `insta`, `predicates`, `nix` (the SIGINT test).

Latest stable versions at implementation time. Exact pins live in `Cargo.toml`. Edition 2024,
MSRV 1.85, the first release with edition-2024 support. Development uses latest stable.

`reqwest::blocking` is *not* tokio-free (it spawns a runtime thread), so "blocking to avoid tokio" is a
myth, hence async.

:warning: **reqwest's built-in retries are disabled** (`retry::never()`). They replay immediately with no
backoff and no `Retry-After`, and the default policy is not GET-only: a retried `POST /rules`
duplicates rules, a retried `DELETE` double-deletes. D12's loop is hand-rolled and owned by `cdctl`.

## D14 — Distribution

`cargo-dist` (linux gnu/musl, macOS arm64/x64, Windows) + Homebrew tap. `release-plz` -> crates.io.

Two traps, each sprung exactly once, at first release:

- `release-plz` and dist both create the GitHub Release by default and collide. `release-plz` owns
  crates.io and the tag, with `git_release_enable = false`. dist owns the Release.
- The tool is `dist`, but install `cargo-dist`: the `dist` crate name is an unrelated placeholder.

The platform verifier reads OS certs from disk, so the static musl binary fails TLS in a certless
(scratch) container: document `/etc/ssl/certs` as required, or bundle webpki roots.

## D15 — Personal accounts only; orgs addable without breaking changes

Org endpoints are **out of scope for v1**: a business feature, untestable on a personal account (all
five 404), and untested commands are worse than none. The community can add them.

**Two things ship in v1 so orgs stay purely additive:**

1. **`org` is one new top-level noun**: cannot collide.
2. **Config is context-keyed from the start**: no migration needed.

**The `--org` flag is deferred with the rest.** Adding an optional flag later is not a breaking
change, and a personal account cannot verify the `X-Force-Org-Id` header's live effect. Shipping
it untested would break this decision's own rule. It arrives with the org noun, tested.

**Gate:** adding orgs must require no breaking change to any command, flag, output field, or exit code.
Org endpoints remain reachable today via `cdctl api`.

## D16 — Documented surface only

Undocumented behavior isn't a contract on an API that breaks without warning.

| Behavior | Verdict |
| --- | --- |
| `Content-Type` ignored / body sniffed | Don't rely on it |
| Indexed `hostnames[0]=` | Don't use — `hostnames[]` is documented and works |
| `POST .../groups/import` | Dropped — undocumented *and* 500s |
| Analytics host, `ipflt`, `cflt` | Out of scope |

**One apparent exception, which isn't:** `GET /profiles/{id}/rules` (folder segment omitted) *is*
documented (in the `folder_id` param prose), just absent from the spec's `paths`. We must use it
because the other documented option (`folder_id=0`) **is broken**.

`cdctl api` remains the escape hatch for anything else.

## D17 — Ship in vertical slices, not all 40 operations at once

Four releases, each independently useful. Anything not yet typed is reachable via `cdctl api`
(D9) from day one, so deferral never locks anyone out:

| Release | Group |
| --- | --- |
| **v0.1** | Core + `cdctl api` + `profile list/get` + `rule`/`folder` CRUD + release machinery |
| **v0.2** | `rule import` + `rule restore` — the flagship, isolated so v0.1 ships sooner |
| **v0.3** | Protection: `profile` writes/options/default, `filter *`, `service *` |
| **v0.4** | Fleet & account: `device *`, `access *`, `proxy`, `analytics`, `account`, `billing`, `network`, `ip` |
| **1.0** | = the full mapped surface, org (D15) and `billing payments` (D2) stay deferred |

**What makes this safe** is the same mechanism as D15's org deferral:

- Every global contract (exit codes, output schema, error envelope, shared flags, config format)
  ships complete in v0.1 and is **treated as frozen from v0.1**, 0.x semver notwithstanding.
  Later groups only add nouns and verbs.
- Folders ride with rules in v0.1 because rules are folder-scoped.
- Group-specific live probes gate their own group (dropdown/level-less filters -> v0.3), not
  earlier releases.

**Cost accepted:** four release cycles instead of one. Users of deferred families type raw
`cdctl api` paths for a while. The delivery table above must stay in sync with
[roadmap.md](roadmap.md).

## D18 — Single crate, binary-only, one module per noun

No `lib.rs`. Our public API is the CLI contract (exit codes, JSON schema, per D2 and D5), not Rust
types. A lib target on crates.io would create semver obligations nobody asked for (fd does the
same, while bat's lib is deliberate and ours would be accidental). If a reusable client ever emerges, it
becomes a separate `controld-client` crate in a workspace, a non-breaking migration.

Layout follows cargo's own binary: `src/cli.rs` holds the root parser (global flags + one enum
variant per noun). `src/commands/<noun>.rs` holds that noun's `Subcommand` enum **and** its
handlers, colocated. Errors are `thiserror`-typed all the way up (no `anyhow`): the D4 envelope
and nine exit codes *are* structure, and erasing them at the binary boundary would forfeit it.

**Cost accepted:** integration tests can't import crate internals: they drive the binary
(`assert_cmd` + wiremock), and fixture deserialization is tested in-module.

## D19 — No bundled MCP server

The MCP niche has incumbents, including a Control D one. `cdctl` differentiates on a CLI agents
call directly. A wrapper can be third-party — the D3 machine-readable spec, if it ever ships, is
its integration point.

**Cost accepted:** MCP-first users go elsewhere.

---

## Open

1. **Proxy-code validation**: masked by the 402 plan gate, needs a Full Control account.
2. **Rate limits**: never observed.
3. **The `[]`-shaped default rule**: never reproduced. Spec says it can happen. Tolerate both.
4. **`icon` on `PUT /devices`, `profile_id2` read-back, `lock_status` values**: unprobed, low-stakes.
5. **`dropdown` option writes** (`PUT /options/{name}` with `value`): unprobed, verify before v0.3.
6. **`billing payments` schema**: needs one real sanitized payload, the typed command waits on it.
7. **Level-less filter writes** (`PUT /filters/filter/{family}` for families without `levels[]`,
   e.g. `noai`): unprobed, verify before v0.3.
8. **The `ips[]` form-variable ceiling** (`POST /access`): unprobed, the 50-IP cap keeps it
   unreachable.
9. **Percent-encoded bracket keys** :white_check_mark: *resolved* — form bodies are hand-built:
   keys verbatim, values percent-encoded. Every write (typed and `cdctl api -F`) sends the literal
   `hostnames[]=` form live verification proved, so the probe of the `%5B%5D` form is moot. (The
   original note claimed `cdctl api -F` already sent literal keys, but it did not: reqwest's form
   encoder emitted `hostnames%5B%5D=` there too, and an earlier `cdctl api -F` wire test had pinned the encoded
   form under a test name that said otherwise.)
