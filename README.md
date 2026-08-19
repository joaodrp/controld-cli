<div align="center">

<picture>
  <source media="(prefers-color-scheme: dark)"
          srcset="https://raw.githubusercontent.com/joaodrp/controld-cli/main/docs/assets/logo-dark.svg">
  <img alt="cdctl - Control D, from the command line"
       src="https://raw.githubusercontent.com/joaodrp/controld-cli/main/docs/assets/logo-light.svg">
</picture>

[![CI](https://github.com/joaodrp/controld-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/joaodrp/controld-cli/actions/workflows/ci.yml)
[![License: MIT OR Apache-2.0](https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue.svg)](#license)
[![MSRV 1.85](https://img.shields.io/badge/MSRV-1.85-orange.svg?logo=rust)](Cargo.toml)

CLI for the [Control D](https://controld.com) REST API. For humans, scripts, and AI agents.

</div>

> [!IMPORTANT]
> Not [`ctrld`](https://github.com/Control-D-Inc/ctrld): that daemon runs your DNS. `cdctl` manages your account.
> An independent project, not affiliated with Control D.

## Demo

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

## Usage

```text
Usage: cdctl [OPTIONS] <COMMAND>

Commands:
  auth         Authenticate cdctl with an API token
  profile      Inspect the account's profiles
  rule         Manage a profile's DNS rules
  folder       Manage a profile's rule folders (API: groups)
  device       Inspect the account's devices (API: endpoints)
  config       Read and write cdctl's own configuration
  api          Raw request against the API origin (escape hatch, GET by default)
  completions  Shell completion script
  reference    The full command surface as one Markdown document
```

The account resources and what you can do to each:

| Resource | list | get | create | update | delete |
| --- | :-: | :-: | :-: | :-: | :-: |
| [profile](docs/commands.md#profile) | ✅ | ✅ | | | |
| [rule](docs/commands.md#rule) | ✅ | | ✅ | ✅ | ✅ |
| [folder](docs/commands.md#folder-api-groups) | ✅ | | ✅ | ✅ | ✅ |
| [device](docs/commands.md#device-api-endpoints) | ✅ | ✅ | | | |

Empty cells aren't shipped yet — see the [roadmap](docs/roadmap.md).

## Highlights

- 🔍 The API can report success for a change it never made, so `cdctl` reads every write back and checks it landed.
- 🛡️ `--dry-run` on every write, confirmation on deletes, and errors that say if a retry is safe.
- 🤖 Scripts and agents get `--json`, data-only stdout, and stable error codes.
- 🚦 [Exit codes](docs/decisions.md#d5--nine-exit-codes-exactly-one-retryable) are a contract: 8 means retry, nothing else does.
- 🔑 The token is never passed on the command line, lives in a file only you can read, and is hidden from `--debug` output.
- 📦 One static binary, with completions and man pages.
- 🚪 `cdctl api` reaches the whole documented API through the same auth, retries, and error handling.
- 👤 Personal accounts, with [orgs addable later](docs/decisions.md#d15--personal-accounts-only-orgs-addable-without-breaking-changes) and no breaking changes.

## Install

The package is `controld-cli`; the binary is `cdctl`.

| Channel | Platforms | Command |
| --- | --- | --- |
| Homebrew | macOS | `brew install joaodrp/tap/cdctl` |
| Prebuilt archive | macOS, Linux, Windows | from [Releases](https://github.com/joaodrp/controld-cli/releases), put `cdctl` on `PATH` |
| Cargo | any | `cargo install controld-cli` |

Archives cover aarch64 and x86_64, Linux in both gnu and musl, and ship completions and man
pages. `cdctl completions --help` generates one for any shell.

Installer scripts (no upgrade path, so re-run one to update):

```sh
# macOS and Linux
curl --proto '=https' --tlsv1.2 -LsSf \
    https://github.com/joaodrp/controld-cli/releases/latest/download/controld-cli-installer.sh | sh

# Windows
powershell -c "irm https://github.com/joaodrp/controld-cli/releases/latest/download/controld-cli-installer.ps1 | iex"
```

Swap `latest/download` for `download/<tag>` to pin a version.

### Uninstall

Removing the binary leaves the config file, which holds your token if you ran `auth login`.
`cdctl config path` prints where it is.

## Quickstart

Create an API token in the [Control D dashboard](https://controld.com/dashboard/api), store it,
and check it reaches the API:

```sh
cdctl auth login --token-stdin     # paste the token at the prompt, input stays hidden
cdctl auth status
```

Most commands operate on one profile. Set a default once instead of passing `--profile` every
time:

```sh
cdctl profile list
cdctl config set default_profile Home
```

Block a domain, list the rules, then delete it again. The delete asks you to confirm:

```sh
cdctl rule create ads.example.com --action block
cdctl rule list
cdctl rule delete ads.example.com
```

From here, every command answers `--help`, `cdctl reference` prints the whole surface as one
document, and [commands](docs/commands.md) specifies each flag, output column, and exit code.

## Configuration

### Environment variables

| Variable | Meaning |
| --- | --- |
| `CONTROLD_API_TOKEN` | API token (alternative to `cdctl auth login`) |
| `CONTROLD_PROFILE` | Profile to operate on, as `--profile` |
| `CONTROLD_OUTPUT` | `json` makes JSON the default output |

### Config file

Plain TOML at `~/.config/cdctl/config.toml` (`%APPDATA%\cdctl\config.toml` on Windows, or
wherever `$XDG_CONFIG_HOME` points):

```toml
current_context = "personal"

[contexts.personal]
token = "api.xxxx"
default_profile = "Home"
```

A context is a named token with its own default profile. One is enough unless you have several
accounts.

| Key | Meaning | Notes |
| --- | --- | --- |
| `current_context` | The context in use | Optional, defaults to `personal`. Set with `cdctl config set` |
| `token` | API token for that context | Written by `cdctl auth login`. Plain text, in a file only you can read |
| `default_profile` | Profile used when `--profile` is absent | Per context. Set with `cdctl config set` |

`cdctl config path` prints that path and `cdctl config list` shows each value with the source it
came from.

### Precedence

When a setting has more than one source, the first one that is set wins:

| Setting | Precedence |
| --- | --- |
| Profile | `--profile` > `CONTROLD_PROFILE` > `default_profile` |
| Token | `CONTROLD_API_TOKEN` > `token` |

## Documentation

| | |
| --- | --- |
| [design](docs/design.md) | Command surface |
| [commands](docs/commands.md) | Per-command flags, columns, JSON fields |
| [decisions](docs/decisions.md) | What was decided, why, what it cost |
| [roadmap](docs/roadmap.md) | What ships next, and its test gates |
| [testing](docs/testing.md) | Test layers, fixture policy, live-test isolation |
| [reference/](docs/reference/) | OpenAPI spec + provenance, live/write verification, error codes |
| [AGENTS](AGENTS.md) | Instructions for coding agents |

## Contributing

Bug reports and pull requests are welcome. [CONTRIBUTING](CONTRIBUTING.md) covers setup, the
test gates, and the rules for running anything against the live API.

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in
this work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without
any additional terms or conditions.
