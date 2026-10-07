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

Two things to know before you let it run. The composition plugin and the
Apollo Router are under the Elastic License v2
(https://www.elastic.co/licensing/elastic-license); the agent asks you once
before anything that needs them, as described under Install, and the
compose, unit, end-to-end and live layers report `not_run` until you have
accepted. And the rover installer adds `~/.rover/bin` to your `PATH` by
editing your shell profile (`~/.profile`, `~/.zshenv`).

## Install

The skill follows the [Agent Skills](https://agentskills.io/) format, so it
works with Claude Code, Codex, Cursor, GitHub Copilot, Gemini CLI and any
other agent that reads skills and can run shell commands. Install it one of
three ways.

**With the Skills CLI** ([skills.sh](https://skills.sh/)), for any agent:

```bash
npx skills add apollographql/graphos-factory
```

**With the GitHub CLI** (`gh skill`, in preview), for any agent:

```bash
gh skill install apollographql/graphos-factory graphos-factory --agent codex   # or claude-code, cursor, github-copilot, ...
```

**As a Claude Code plugin**, from Apollo's marketplace
(`apollographql/skills`):

```
/plugin marketplace add apollographql/skills
/plugin install graphos-factory@apollo-marketplace
```

The first two copy `skills/graphos-factory/` alone, which is why it carries
its own `graphos-factory-core/` (the shared references and the validation
scripts, plus `release.env`, the binary version it pins). The agent sets up
the binary itself, as `SKILL.md` tells it: `scripts/bootstrap.sh` downloads
`graphos-factory` from this repository's releases into
`~/.cache/graphos-factory-core/`, with a `graphos-factory-core` link beside
it, and `. scripts/env.sh` puts both, and rover, on `PATH` and sets
`GRAPHOS_FACTORY_CORE_SCRIPTS`. You can run the same two commands yourself
from the installed skill's directory.

The plugin does that at the start of each session instead: its hook
(`skills/graphos-factory/scripts/session-start.sh`) runs `bootstrap.sh` and
puts the binary on the session's `PATH`. The shared references write
`graphos-factory-core <command>`; here that is `graphos-factory`. To build
it from source instead, run
`bash skills/graphos-factory/scripts/bootstrap.sh --build` in a clone. Under
any install, the toolchain above is not installed up front: the agent runs
`toolchain.sh` as a step you see and approve, the first time a layer needs
it.

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
  commands; it never publishes. Run `rover subgraph publish` yourself, with
  your own `APOLLO_KEY`.
- **The live layer.** It calls the real API, so it needs the API's
  credential in your environment. Without one it is recorded `not_run`.
- **Composition with your other subgraphs.** The skill composes the subgraph
  alone. To check it against your graph, export `APOLLO_KEY` in the shell
  the agent runs in; the agent asks once per session which `graph@variant`
  to check against and passes it as `GRAPHOS_FACTORY_GRAPH_REF`. The
  `supergraph_check` layer then runs `rover subgraph check` (it sends the
  rendered schema to GraphOS and publishes nothing), and `export` refuses a
  subgraph that fails it. To have every run checked without being asked,
  set `GRAPHOS_FACTORY_SUPERGRAPH_CHECK=auto` beside `APOLLO_GRAPH_REF` in
  your shell profile; `APOLLO_GRAPH_REF` alone starts no check. Otherwise
  the layer is `not_run`. The key stays in your environment; the agent
  never asks for it and nothing records it.
- **Reading the evidence.** A `skipped` or `not_run` layer is never reported
  as a pass, and a workspace where a selected operation has no executed
  evidence is reported as not validated.

## Versions

A release is versioned by the product, `.claude-plugin/plugin.json`
(also the skill's `metadata.version`): each `vX.Y.Z` tag here is a release
of the skill and plugin, with the `graphos-factory` binary built from the
same commit attached. The binary keeps its own version (`crate/Cargo.toml`),
which moves only when the binary changes; the skill's
`graphos-factory-core/release.env` names both, and `bootstrap.sh` downloads
the binary from that release. `gh skill install` installs the latest
release, Apollo's marketplace pins one, and `npx skills add` and a clone read
`main`.

The hook only checks for the validation toolchain (rover and its supergraph
plugin, the Apollo Router, WireMock); the agent asks you before it runs
`graphos-factory-core/scripts/toolchain.sh` to install it, since that
downloads about 100 MB. The composition plugin and the Apollo Router are
under the Elastic License v2
(https://www.elastic.co/licensing/elastic-license): the agent shows you the
link and asks once; if you say yes it sets `APOLLO_ELV2_LICENSE=accept` for
the commands it runs in that session, and an `export` in your shell profile
makes it permanent. It never sets it unasked, and until it is set the
compose, unit, end-to-end and live layers report `not_run`. Java
17+ and `jq` are yours to install.

## Layout

- `skills/graphos-factory/`: the skill itself, its `SKILL.md`, its
  references, its `scripts/` (`bootstrap.sh`, `env.sh` and the plugin's
  `session-start.sh`) and its own copy of the core in
  `graphos-factory-core/`, so the directory works when it is installed on
  its own.
- `graphos-factory-core/`: the core the skill is built on: the shared
  references, the validation scripts, and `SKILL-core.md`. The copy inside
  the skill is made from it at every sync.
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
