# GraphOS Factory

An agent skill for Claude Code that builds an Apollo Federation subgraph
from a REST API, wrapped with Apollo Connectors. The agent works in a
dedicated git workspace that records the API's inventory, the operations
and fields you selected, and every design decision, and it checks each
change with composition, connector unit tests, a mocked end-to-end run and,
when you give it a credential, a live run.

## Requirements

- macOS or Linux, on x86_64 or aarch64. On Windows, use WSL. The release
  binaries are built for Linux (musl) and macOS.
- `bash`, `curl`, `git` and `jq`.
- Java 17 or later, for the end-to-end and live layers (WireMock runs on
  it).
- Optional: Rust, only if you want to build the binary from source
  (`bootstrap.sh --build`). The plugin installs a prebuilt binary.

The validation layers also need rover, its supergraph plugin, the Apollo
Router and WireMock. `graphos-factory-core/scripts/toolchain.sh` installs
the versions pinned in it; today:

| Tool | Version | Installed to |
|---|---|---|
| rover | 0.41.0 | `~/.rover/bin` |
| supergraph plugin | 2.15.2 | `~/.rover/bin` |
| Apollo Router | 2.17.0 | `~/.cache/graphos-factory-core/apollographql-graphos-factory/` |
| WireMock | 3.13.2 | `~/.cache/graphos-factory-core/apollographql-graphos-factory/` |

Two things to know before you let it run. The supergraph composition
plugin is under the Elastic License v2
(https://www.elastic.co/licensing/elastic-license): read it, then set
`APOLLO_ELV2_LICENSE=accept` in your environment yourself; the scripts never
accept it for you, and the compose, unit, end-to-end and live layers report
`not_run` until you have. And the rover installer adds `~/.rover/bin` to
your `PATH` by editing your shell profile (`~/.profile`, `~/.zshenv`).

## Install

The plugin is listed in Apollo's Claude Code plugin marketplace
(`apollographql/skills`). In Claude Code:

```
/plugin marketplace add apollographql/skills
/plugin install graphos-factory@apollo-marketplace
```

The plugin installs the `graphos-factory` binary when a session starts,
from this repository's releases
(`skills/graphos-factory/scripts/bootstrap.sh`), with a
`graphos-factory-core` link beside it: the shared references write
`graphos-factory-core <command>`, and here that is `graphos-factory`. To
build it from source instead, run
`bash skills/graphos-factory/scripts/bootstrap.sh --build`. The toolchain
above is not installed at session start: the agent runs `toolchain.sh` as a
step you see and approve, the first time a layer needs it.

## Quick start

1. Install the plugin, as above.
2. Start Claude Code in the directory where the subgraph's workspace should
   live: an empty directory, or a folder in the repository that holds your
   other subgraphs.
3. Ask for the subgraph, and give the agent the API's description document
   if it has one. For example: "Wrap the Gitea REST API at
   https://gitea.example.com/api/v1 as a connectors subgraph; here is its
   swagger.json."
4. Answer the agent's questions. It asks which operations and fields to
   expose, and every design question the document does not settle, and
   records your answers; it does not answer them for you.
5. Approve the commands it asks to run: the toolchain install on first use,
   then the validation layers.

You end up with a git workspace holding the schema, the selection of
operations and fields, the decisions, the tests, and
`.factory/evidence/latest.json`, which records what each validation layer
did: passed, failed, skipped or not run.

## What is yours to do

- **Publishing to GraphOS.** The skill renders the schema and hands you the
  commands; it never runs them. Run `rover subgraph check` and
  `rover subgraph publish` yourself, with your own `APOLLO_KEY`.
- **The live layer.** It calls the real API, so it needs the API's
  credential in your environment. Without one it is recorded `not_run`.
- **Composition with your other subgraphs.** The skill composes the subgraph
  alone, and records `supergraph_check` as `not_run`.
  `rover subgraph check` against your graph is that check.
- **Reading the evidence.** A `skipped` or `not_run` layer is never reported
  as a pass, and a workspace where a selected operation has no executed
  evidence is reported as not validated.

## Versions

The plugin's version (`0.1.0`) is independent of the binary's:
`bootstrap.sh` installs the release matching the crate version pinned in
the plugin's own copy (`crate/Cargo.toml`).

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
repository; `SOURCE_COMMIT` names the commit it came from. Each snapshot is
added on top of the last, so a commit's diff is what changed upstream since
the previous one. Only the sync pushes here.

Issues are welcome. Pull requests are closed without review, because a
change has to land upstream to reach the next snapshot; open an issue that
describes it instead.

## License

MIT. See `LICENSE`.
