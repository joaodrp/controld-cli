# controld-cli

CLI for the [Control D](https://controld.com) **REST API** — profiles, rules, filters, services,
devices. For humans, scripts, and AI agents.

> **Not [`ctrld`](https://github.com/Control-D-Inc/ctrld)**, Control D's DNS proxy **daemon**. That runs
> DNS on your machine; this manages your Control D **account**. They coexist.
>
> Package `controld-cli`, binary **`cdctl`**

```console
$ cdctl profile list
$ cdctl rule create ads.example.com --action block --profile Home
$ cdctl rule list --profile Home --json | jq '.[] | select(.action == "block")'
```

## Install

v0.1 is the first release; artifacts below land with it.

```console
$ brew install joaodrp/tap/cdctl
```

(`joaodrp/tap` resolves to the [`joaodrp/homebrew-tap`](https://github.com/joaodrp/homebrew-tap) repo.)

```console
$ curl --proto '=https' --tlsv1.2 -LsSf \
    https://github.com/joaodrp/controld-cli/releases/latest/download/controld-cli-installer.sh | sh
```

Or grab a prebuilt archive from [Releases](https://github.com/joaodrp/controld-cli/releases) —
Linux (gnu, musl), macOS (arm64, x64), Windows.

```console
$ cargo install controld-cli
```

> **musl static binary in a certless container:** it reads OS certificates at runtime
> (`/etc/ssl/certs`), so a scratch/certless image needs CA certs installed or mounted, or
> `SSL_CERT_FILE`/`SSL_CERT_DIR` pointed at them ([D14](docs/decisions.md)).

### Shell completions and man pages

Prebuilt archives ship generated completions and man pages. To generate a completion script
yourself:

```console
$ cdctl completions zsh > _cdctl
```

Supported shells: `bash`, `elvish`, `fish`, `powershell`, `zsh`.

## Quickstart

Create an API token in the [Control D dashboard](https://controld.com/dashboard/api), then:

```console
$ echo -n "$CONTROLD_API_TOKEN" | cdctl auth login --token-stdin
$ cdctl profile list
$ cdctl rule create ads.example.com --action block --profile Home
$ cdctl rule list --profile Home
$ cdctl rule delete ads.example.com --profile Home --yes
```

## For agents as well as humans

- `--json` everywhere; **stdout is data-only**, diagnostics on stderr.
- Structured errors with **stable slugs** and an explicit `retryable` flag.
- **Exit `8` is retryable; everything else is terminal.**
- No hidden prompts — a non-interactive run without `--yes` fails loudly.
- No silent degradation — a read-only token used for a write **errors**.
- `cdctl api` escape hatch, as a **separate verb** so a sandbox can allow `cdctl` but deny `cdctl api`.

## Scope

**Personal accounts.** Organization endpoints are deferred ([D15](docs/decisions.md)) — untestable on a
personal account, and untested commands are worse than none. The design keeps them additive.

**v0.1** ships `profile list/get`, `rule`/`folder` CRUD, and `cdctl api` (the escape hatch for
everything else). `rule import`/`restore` land in v0.2; the full roadmap is in
[`docs/roadmap.md`](docs/roadmap.md).

## Docs

| | |
| --- | --- |
| [design.md](docs/design.md) | Command surface |
| [commands.md](docs/commands.md) | Per-command flags, columns, JSON fields |
| [decisions.md](docs/decisions.md) | What was decided, why, what it cost |
| [roadmap.md](docs/roadmap.md) | What ships next, and its test gates |
| [testing.md](docs/testing.md) | Test layers, fixture policy, live-test isolation |
| [reference/](docs/reference/) | OpenAPI spec + provenance, live/write verification, error codes |
| [AGENTS.md](AGENTS.md) | Instructions for coding agents (any agent, not just Claude Code) |

### The spec

Control D publish no downloadable OpenAPI spec, but every rendered docs page embeds it.
[`scripts/fetch-spec.sh`](scripts/fetch-spec.sh) extracts it and asserts all 46 endpoint pages ship a
byte-identical copy.

```console
$ ./scripts/fetch-spec.sh
    all 46 pages agree (sha256 0404ebecf30b38f9)
    35 paths, 46 operations
```

The Control D API is **unversioned** — *"[breaking changes can be introduced without warning](https://docs.controld.com/reference/get-started)"* — so re-run and diff. CI does this weekly.

## Development

```console
$ cargo test
$ cargo clippy --all-targets -- -D warnings
$ cargo fmt --check

$ cp .env.example .env    # API token — live suite and manual probes only; plain `cargo test` is network-free
```

Live tests confine their writes to a temporary `cdctl-test-*` profile they create and delete
([docs/testing.md](docs/testing.md)); manual probes mutate whatever you point them at. A free trial account is
the safe default.

`tests/fixtures/api/` holds **real API responses** (sanitized), covering every deserialization hazard
the live API throws — catalogued in [`docs/reference/`](docs/reference/).

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
