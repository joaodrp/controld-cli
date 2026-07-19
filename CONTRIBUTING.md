# Contributing

Thanks for helping out. This page is the practical part; the design docs under
[`docs/`](docs/) explain why things are the way they are.

## Setup

```sh
git clone https://github.com/joaodrp/controld-cli
cd controld-cli
cargo build
```

The toolchain and MSRV (1.85) come from `Cargo.toml`.

## Test gates

Every change must pass:

```sh
cargo test
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

`cargo test` never touches the network. Test layers and the fixture policy are in
[testing.md](docs/testing.md); new fixtures in `tests/fixtures/api/` must be sanitized (no
emails, device names, real domains, public IPs, or account PKs).

## The live suite

The opt-in live suite runs against the real API:

```sh
cp .env.example .env   # supplies CONTROLD_API_TOKEN
CONTROLD_LIVE_TESTS=1 cargo test --test live
```

> [!WARNING]
> Live tests confine their writes to a temporary `cdctl-test-*` profile they create and delete
> ([Live-test isolation](docs/testing.md#live-test-isolation)); manual probes mutate whatever
> you point them at. A free trial account is the safe default.

## The API spec

Control D publish no downloadable OpenAPI spec, but every rendered docs page embeds it.
[`scripts/fetch-spec.sh`](scripts/fetch-spec.sh) extracts it and asserts all 46 endpoint pages
ship a byte-identical copy:

```console
$ ./scripts/fetch-spec.sh
==> Fetching reference sidebar
==> Found 46 endpoint pages
==> Fetching all endpoint pages
==> Extracting and cross-verifying embedded spec
    all 46 pages agree (sha256 0404ebecf30b38f9)
    35 paths, 46 operations
==> Unchanged (matches docs/reference/controld-openapi.json)
```

The API is unversioned (*"[breaking changes can be introduced without
warning](https://docs.controld.com/reference/get-started)"*), so CI re-fetches and diffs weekly.
Where the spec came from and how far to trust it:
[spec-provenance.md](docs/reference/spec-provenance.md). Read
[hazards.md](docs/reference/hazards.md) before touching `src/api/` or any write path.

## Conventions

- [decisions.md](docs/decisions.md) is authoritative. Several decisions look wrong until you
  read the reasoning; changing one needs the maintainer's explicit approval, recorded by
  amending that file.
- Commit messages and PR titles follow [Conventional Commits](https://www.conventionalcommits.org/).
- Two contracts are public API and semver-major to break: the exit codes and the stdout-is-data
  rule. [AGENTS.md](AGENTS.md) states both.

## Bug reports

Open an [issue](https://github.com/joaodrp/controld-cli/issues) with `cdctl --version`, the
command you ran, and if possible a `--debug` trace (the token is always redacted).
