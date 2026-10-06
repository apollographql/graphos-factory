# CLAUDE.md

This repository is a push-only mirror. Every commit is a snapshot published
from a private development repository (`SOURCE_COMMIT` names it), added on
top of the previous one; a commit's diff is what changed upstream. Only the
sync pushes here, so a change made here cannot land; report it as an issue
instead.

## Layout

| Path | What it is |
|---|---|
| `skills/graphos-factory/` | the skill: `SKILL.md`, its references, `scripts/bootstrap.sh`, `scripts/env.sh` (PATH for an agent with no hook), `scripts/session-start.sh` (the SessionStart hook `hooks/hooks.json` runs) and `graphos-factory-core/`, its own copy of the core with a `release.env` pin, so `npx skills` and `gh skill`, which copy this directory alone, install a working skill |
| `graphos-factory-core/` | the core skill material: `references/`, `scripts/` (every layer's bash wrappers), `SKILL-core.md` |
| `crate/` | cargo workspace: `graphos-factory-core` (lib) and `graphos-factory-targets` (the `graphos-factory` binary) |
| `schemas/` | JSON Schemas for the workspace contract, embedded into the binary |
| `pilots/graphos/` | validated example workspaces; CI runs their offline layers |

## Build and test

```bash
cargo fmt --manifest-path crate/Cargo.toml --all --check
cargo test --manifest-path crate/Cargo.toml --workspace --locked
cargo build --manifest-path crate/Cargo.toml --release --locked -p graphos-factory-targets   # target/release/graphos-factory
bash skills/graphos-factory/scripts/bootstrap.sh --build   # install it, and the graphos-factory-core link, from source
bash graphos-factory-core/scripts/toolchain.sh               # rover, the router and WireMock
graphos-factory evidence pilots/graphos/gitea --scripts graphos-factory-core/scripts
```

A pilot's `.factory/` files are checked artifacts: never edit a recorded
fixture to make a test pass, and treat `skipped` or `not_run` as not passed.
