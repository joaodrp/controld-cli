# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0](https://github.com/joaodrp/controld-cli/releases/tag/v0.1.0) - 2026-07-18

### Added

- *(rule)* case-aware target resolution for update and delete
- *(rule)* rule CRUD and the multi-target write engine
- *(folder)* folder CRUD with shared action flags, dry-run, and confirmation
- *(profile)* profile list and get with normalized models
- *(cli)* add cdctl api, the D9 escape hatch
- *(api)* raw passthrough requests for the escape hatch
- *(api)* pin joined request URLs to the base origin
- implement Phase 1 core

### Fixed

- resolve the full-codebase review findings
- *(cli)* UX polish from the v0.1 help and error audit
- *(ci)* move dist's build-setup fragment out of .github/workflows
- *(rule)* foldered rules are invisible to the root listing
- *(rule)* never advertise an unconvergeable via6-clear retry
- *(confirm)* keep SIGINT live at the confirmation prompt
- *(build)* cap comfy-table below 7.2.2 to keep the 1.85 MSRV buildable
- *(api)* hand-build form bodies with literal bracket keys ([#3](https://github.com/joaodrp/controld-cli/pull/3))
- *(api)* reach public endpoints without a token ([#2](https://github.com/joaodrp/controld-cli/pull/2))
- *(api)* sharper diagnostics on the raw rejection paths
- *(api)* keep SIGINT live while reading the stdin body
- *(api)* reject an empty stdin body before any request
- *(api)* classify marker-less raw bodies on the HTTP status

### Other

- link text names the topic, not the file-and-section pair
- *(readme)* drop the musl cert note
- *(readme)* affiliation disclaimer; alerts where they earn it
- *(readme)* install section leads with the cross-platform script
- *(readme)* ctrld disambiguation as a GitHub alert
- *(readme)* drop the contextless teaser block
- direnv support and a sourceable .env.example
- drop `$ ` prompts from copy-pasteable samples
- *(readme)* shell-neutral completions example
- *(readme)* plain prose in the quickstart; completions delegate to --help
- *(readme)* quickstart guides setup, first operation, and discovery
- AGENTS.md becomes a leaf — shared content moves to canonical homes
- *(readme)* repair the hero image and label the installer
- *(readme)* hero with logo and badges
- release-readiness audit fixes
- trim noise and telegraphese from the reader-facing docs
- *(readme)* drop the release-contents paragraph from Scope
- *(readme)* introduce the API token before the quickstart block
- *(readme)* testing.md in the doc table; .env is for live tests only
- extract testing into docs/testing.md
- *(agents)* audit fixes — placement, generated-file trap, conventions
- single-source agent instructions, CLAUDE.md -> AGENTS.md symlink
- *(roadmap)* trim needless words from the release watch items
- retire plan.md in favor of roadmap.md
- *(release)* v0.1 machinery
- *(live)* opt-in live suite against the real API
- *(cli)* close the remaining api coverage gaps
- record the raw 2xx-is-success contract for cdctl api
- correct comment rot found in review
- add the missing module doc to completions
- one behavior per test in the config and error suites
- expect instead of allow where dead code is dead everywhere
- *(cli)* guard global flag ids against subcommand collisions
- *(error)* one constructor for stdout write failures
- *(api)* dedup the client pipelines and the origin policy
- record the cdctl api output and confirmation contracts
- *(cli)* prove the Phase 2 gate at the binary level
- *(api)* split request transport from response interpretation
- reconcile contracts with the Phase 1 implementation
- bootstrap cdctl design, verified API reference, and delivery plan
