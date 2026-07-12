# controld-cli

CLI for the [Control D](https://controld.com) **REST API** — profiles, rules, filters, services,
devices. For humans, scripts, and AI agents.

> **Not [`ctrld`](https://github.com/Control-D-Inc/ctrld)**, Control D's DNS proxy **daemon**. That runs
> DNS on your machine; this manages your Control D **account**. They coexist.
>
> Package `controld-cli`, binary **`cdctl`**

**Status: design complete, implementation not started.** No CLI for the Control D API existed — hence
this one. Delivery is sliced: rules and folders first (v0.1), `rule import`/`restore` next (v0.2) —
see [`docs/plan.md`](docs/plan.md).

```console
$ cdctl profile list
$ cdctl rule create ads.example.com --action block --profile Home
$ cdctl rule list --json | jq '.[] | select(.action == "block")'
$ curl -s https://.../blocklist.txt | cdctl rule import - --action block
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

## Docs

| | |
| --- | --- |
| [design.md](docs/design.md) | Command surface |
| [commands.md](docs/commands.md) | Per-command flags, columns, JSON fields |
| [decisions.md](docs/decisions.md) | What was decided, why, what it cost |
| [plan.md](docs/plan.md) | Implementation phases |
| [reference/](docs/reference/) | OpenAPI spec + provenance, live/write verification, error codes |

### The spec

Control D publish no downloadable OpenAPI spec, but every rendered docs page embeds it.
[`scripts/fetch-spec.sh`](scripts/fetch-spec.sh) extracts it and asserts all 46 endpoint pages ship a
byte-identical copy.

```console
$ ./scripts/fetch-spec.sh
    all 46 pages agree (sha256 0404ebecf30b38f9)
    35 paths, 46 operations
```

The Control D API is **unversioned** — *"[breaking changes can be introduced without warning](https://docs.controld.com/reference/get-started)"* — so re-run and diff.

## Development

```console
$ cp .env.example .env    # add a Control D API token
```

Live tests confine their writes to a temporary `cdctl-test-*` profile they create and delete
([plan.md](docs/plan.md)); manual probes mutate whatever you point them at. A free trial account is
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
