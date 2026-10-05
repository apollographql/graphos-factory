# GraphOS Factory

An agent skill for Claude Code that builds an Apollo Federation subgraph
from a REST API, wrapped with Apollo Connectors. The agent works in a
dedicated git workspace that records the API's inventory, the operations
and fields you selected, and every design decision, and it checks each
change with composition, connector unit tests, a mocked end-to-end run and,
when you give it a credential, a live run.

## Install

In Claude Code:

```
/plugin marketplace add apollographql/graphos-factory
/plugin install graphos-factory@graphos-factory
```

The skill installs the `graphos-factory` binary on first use from this
repository's releases (`skills/graphos-factory/scripts/bootstrap.sh`), with
a `graphos-factory-core` link beside it: the shared references write
`graphos-factory-core <command>`, and here that is `graphos-factory`. To
build it from source instead, run
`bash skills/graphos-factory/scripts/bootstrap.sh --build`.

## Layout

- `skills/graphos-factory/`: the skill itself, its `SKILL.md`, its
  references and its `scripts/bootstrap.sh`.
- `graphos-factory-core/`: the core the skill is built on: the shared
  references, the validation scripts, and `SKILL-core.md`.
- `crate/`: the Rust workspace: `graphos-factory-core`, the library, and
  `graphos-factory-targets`, whose one binary here is `graphos-factory`.
- `schemas/`: the JSON Schemas of the workspace contract.
- `pilots/graphos/`: fully validated example workspaces. CI re-runs their
  offline layers on every snapshot.

## This repository is a mirror

Each commit here is a snapshot published from a private development
repository; `SOURCE_COMMIT` names the commit it came from. History is not
kept, and nothing pushed here by hand survives the next snapshot.

Issues are welcome. Pull requests are closed without review, because a
change has to land upstream to reach the next snapshot; open an issue that
describes it instead.

## License

MIT. See `LICENSE`.
