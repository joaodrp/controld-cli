<div align="center">

![cdctl - Control D, from the command line](docs/assets/logo-dark.svg#gh-dark-mode-only)
![cdctl - Control D, from the command line](docs/assets/logo-light.svg#gh-light-mode-only)

[![CI](https://github.com/joaodrp/controld-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/joaodrp/controld-cli/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV 1.85](https://img.shields.io/badge/MSRV-1.85-orange.svg?logo=rust)](Cargo.toml)

CLI for the [Control D](https://controld.com) REST API. For humans, scripts, and AI agents.

</div>

> [!IMPORTANT]
> Not [`ctrld`](https://github.com/Control-D-Inc/ctrld): that daemon runs your DNS. `cdctl` manages your account.
> An independent project, not affiliated with Control D.

## A quick look

```console
$ cdctl rule create ads.example.com trackers.example.net --action block
┌──────────────────────┬────────┬─────┬─────────┬────────┐
│ HOSTNAME             ┆ ACTION ┆ VIA ┆ ENABLED ┆ FOLDER │
╞══════════════════════╪════════╪═════╪═════════╪════════╡
│ ads.example.com      ┆ block  ┆ -   ┆ true    ┆ -      │
│ trackers.example.net ┆ block  ┆ -   ┆ true    ┆ -      │
└──────────────────────┴────────┴─────┴─────────┴────────┘

$ cdctl rule create tv.example.com --action spoof --via 192.0.2.10
┌────────────────┬────────┬────────────┬─────────┬────────┐
│ HOSTNAME       ┆ ACTION ┆ VIA        ┆ ENABLED ┆ FOLDER │
╞════════════════╪════════╪════════════╪═════════╪════════╡
│ tv.example.com ┆ spoof  ┆ 192.0.2.10 ┆ true    ┆ -      │
└────────────────┴────────┴────────────┴─────────┴────────┘

$ cdctl rule list --fields hostname,action
[
  {
    "hostname": "ads.example.com",
    "action": "block"
  },
  {
    "hostname": "trackers.example.net",
    "action": "block"
  },
  {
    "hostname": "tv.example.com",
    "action": "spoof"
  }
]
```

## Highlights

- Verified writes. The API can acknowledge a write it did not apply, so every mutation is read
  back and compared before `cdctl` reports success.
- Safe by default. Every mutation takes `-n`/`--dry-run`, deletes ask for confirmation, and a
  failed write says whether retrying is safe.
- Built for scripts and agents. `--json` everywhere, stdout carries only data, errors have
  stable slugs, and the [exit codes](docs/decisions.md#d5--nine-exit-codes-exactly-one-retryable)
  are a documented contract: 8 means retry, nothing else does.
- Careful with the token. Read from the environment or a hidden prompt, never argv. Stored in a
  `0600` file, redacted in `--debug` traces.
- One static binary, with shell completions and man pages. Config follows XDG.
- The whole documented API is reachable: `cdctl api` sends raw requests through the same auth,
  retries, and error mapping as the typed commands.
- Personal accounts, documented API surface only. Organization endpoints are
  [deferred](docs/decisions.md#d15--personal-accounts-only-orgs-addable-without-breaking-changes),
  additively.

## Install

The package is `controld-cli`, and every method below installs the `cdctl` binary. Installer
script (Linux and macOS):

```sh
curl --proto '=https' --tlsv1.2 -LsSf \
    https://github.com/joaodrp/controld-cli/releases/latest/download/controld-cli-installer.sh | sh
```

Or a prebuilt archive from [Releases](https://github.com/joaodrp/controld-cli/releases):
Linux (gnu, musl), macOS (arm64, x64), Windows. Or Homebrew:

```sh
brew install joaodrp/tap/cdctl
```

Or from source:

```sh
cargo install controld-cli
```

Prebuilt archives ship completions and man pages. `cdctl completions <shell>` generates a script
for bash, elvish, fish, powershell, or zsh, and `cdctl completions --help` shows where to put it.

To uninstall, remove the binary the way it arrived: `brew uninstall cdctl`,
`cargo uninstall controld-cli`, or delete `cdctl` from where the installer put it
(`~/.cargo/bin` by default). The config file, at the path `cdctl config path` prints, holds your
token if you ran `auth login`. Delete it too.

## Quickstart

Create an API token in the [Control D dashboard](https://controld.com/dashboard/api), store it,
and check it reaches the API:

```sh
echo -n "$CONTROLD_API_TOKEN" | cdctl auth login --token-stdin
cdctl auth status
```

Most commands operate on one profile. Set a default once instead of passing `--profile` every
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

## Documentation

| | |
| --- | --- |
| [design.md](docs/design.md) | Command surface |
| [commands.md](docs/commands.md) | Per-command flags, columns, JSON fields |
| [decisions.md](docs/decisions.md) | What was decided, why, what it cost |
| [roadmap.md](docs/roadmap.md) | What ships next, and its test gates |
| [testing.md](docs/testing.md) | Test layers, fixture policy, live-test isolation |
| [reference/](docs/reference/) | OpenAPI spec + provenance, live/write verification, error codes |
| [AGENTS.md](AGENTS.md) | Instructions for coding agents |

## Contributing

Bug reports and pull requests are welcome. [CONTRIBUTING.md](CONTRIBUTING.md) covers setup, the
test gates, and the rules for running anything against the live API.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
