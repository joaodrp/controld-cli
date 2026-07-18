<div align="center">

![cdctl — Control D, from the command line](docs/assets/logo-dark.svg#gh-dark-mode-only)
![cdctl — Control D, from the command line](docs/assets/logo-light.svg#gh-light-mode-only)

[![CI](https://github.com/joaodrp/controld-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/joaodrp/controld-cli/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV 1.85](https://img.shields.io/badge/MSRV-1.85-orange.svg?logo=rust)](Cargo.toml)

CLI for the [Control D](https://controld.com) **REST API** — profiles, rules, filters, services,
devices. For humans, scripts, and AI agents.

</div>

> [!IMPORTANT]
> **Not [`ctrld`](https://github.com/Control-D-Inc/ctrld)**, Control D's DNS proxy **daemon**. That runs
> DNS on your machine; this manages your Control D **account**. They coexist.

Package `controld-cli`, binary **`cdctl`**.

## Install

v0.1 is the first release; artifacts below land with it.

```sh
brew install joaodrp/tap/cdctl
```

Or the prebuilt-binary installer script (Linux and macOS):

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
    https://github.com/joaodrp/controld-cli/releases/latest/download/controld-cli-installer.sh | sh
```

Or grab a prebuilt archive from [Releases](https://github.com/joaodrp/controld-cli/releases) —
Linux (gnu, musl), macOS (arm64, x64), Windows.

```sh
cargo install controld-cli
```

> The musl binary reads OS CA certificates at runtime — in a certless image, install/mount them or set `SSL_CERT_FILE` ([D14](docs/decisions.md#d14--distribution)).

### Shell completions and man pages

Prebuilt archives ship generated completions and man pages. To generate a script yourself:

```sh
cdctl completions <shell>
```

The script goes to stdout; `cdctl completions --help` shows each shell's install path. Supported:
`bash`, `elvish`, `fish`, `powershell`, `zsh`.

## Quickstart

Create an API token in the [Control D dashboard](https://controld.com/dashboard/api), store it,
and check it reaches the API:

```sh
echo -n "$CONTROLD_API_TOKEN" | cdctl auth login --token-stdin
cdctl auth status
```

Most commands operate on one profile; set a default once instead of passing `--profile` every
time:

```sh
cdctl profile list
cdctl config set default_profile Home
```

Block a domain, list the rules, then delete it again. The delete prompts for confirmation
(`--yes` skips the prompt, but only alongside an explicit `--profile`):

```sh
cdctl rule create ads.example.com --action block
cdctl rule list
cdctl rule delete ads.example.com
```

From here, every command answers `--help`, `cdctl reference` prints the whole surface as one
document, and [commands.md](docs/commands.md) specifies each flag, output column, and exit code.
Scripts and agents get `--json` (stdout is data-only) and the
[documented exit codes](docs/decisions.md#d5--nine-exit-codes-exactly-one-retryable).

## For agents as well as humans

- `--json` everywhere; **stdout is data-only**, diagnostics on stderr.
- Structured errors with **stable slugs** and an explicit `retryable` flag.
- **Exit `8` is retryable; everything else is terminal.**
- No hidden prompts — a non-interactive run without `--yes` fails loudly.
- No silent degradation — a read-only token used for a write **errors**.
- `cdctl api` escape hatch, as a **separate verb** so a sandbox can allow `cdctl` but deny `cdctl api`.

## Scope

**Personal accounts.** Organization endpoints are deferred ([D15](docs/decisions.md#d15--personal-accounts-only-orgs-addable-without-breaking-changes)) — untestable on a
personal account. The design keeps them additive.

## Docs

| | |
| --- | --- |
| [design.md](docs/design.md) | Command surface |
| [commands.md](docs/commands.md) | Per-command flags, columns, JSON fields |
| [decisions.md](docs/decisions.md) | What was decided, why, what it cost |
| [roadmap.md](docs/roadmap.md) | What ships next, and its test gates |
| [testing.md](docs/testing.md) | Test layers, fixture policy, live-test isolation |
| [reference/](docs/reference/) | OpenAPI spec + provenance, live/write verification, error codes |
| [AGENTS.md](AGENTS.md) | Instructions for coding agents |

### The spec

Control D publish no downloadable OpenAPI spec, but every rendered docs page embeds it.
[`scripts/fetch-spec.sh`](scripts/fetch-spec.sh) extracts it and asserts all 46 endpoint pages ship a
byte-identical copy.

```console
$ ./scripts/fetch-spec.sh
    all 46 pages agree (sha256 0404ebecf30b38f9)
    35 paths, 46 operations
```

The Control D API is **unversioned** — *"[breaking changes can be introduced without warning](https://docs.controld.com/reference/get-started)"* — so it's worth re-running and diffing regularly; CI does exactly that every week.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check

cp .env.example .env
```

The token from `.env` is for the live suite and manual probes only — plain `cargo test` stays
network-free. Live tests confine their writes to a temporary `cdctl-test-*` profile they create and
delete ([docs/testing.md, Live-test isolation](docs/testing.md#live-test-isolation)); manual probes mutate whatever you point them at. A free
trial account is the safe default.

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
