# Verification: the evidence layers and lint rules this target adds

This reference is owned by the next pass, the one that writes the
graphos-factory instructions, and it is not written yet. This page holds
the open questions so that pass has a place to answer them. Nothing below is
a decision, and an agent following this skill must not treat a question as
guidance.

The core keeps six evidence layers (`compose`, `connector_unit`,
`wiremock_e2e`, `conformance`, `lint`, `live`) and every lint rule about the
workspace contract and Connectors. A target adds evidence layers and lint
rules on top, and `testing.md` in the core says a target describes its own
in its own reference. This is that reference for this target.

## The `supergraph_check` evidence layer

The target's first added layer is named `supergraph_check`. Until the
mechanism is decided it is reported `not_run` with the reason, never as a
pass. The question is what it runs, and there are two candidates:

- `rover subgraph check` against a GraphOS variant, which needs a graph
  reference and credentials, and which compares the subgraph with the schema
  already published there.
- A local composition of this subgraph with the user's other subgraphs,
  through `rover supergraph compose`, which needs their schemas on disk and
  no network.

The question includes where the layer sits in the order, how its result maps
onto the per-operation table (one row per operation, or one result for the
whole subgraph), what `skipped` versus `not_run` mean when the user has no
other subgraphs, and what `compose.sh` and `ComposeConfig` supply as extra
inputs.

## Federation lint rules this target adds

Each candidate below is a question, not a rule. A rule needs a stated
condition, a severity, and a fixture that fails when the rule is reverted.

- Should a lint rule fire on an entity whose `@key` does not match the key
  that the owning subgraph declares, and how does the skill learn the owning
  subgraph's key when it only has this subgraph?
- Should a lint rule fire when `@shareable` is missing on a field that
  another subgraph also defines, and is that knowable without the other
  subgraphs' schemas?
- Should a lint rule fire on an `@external` field that no `@requires` or
  `@provides` uses?
- Should a lint rule fire on a `@requires` whose fields cannot be fetched by
  any connector in this subgraph?
- Should a lint rule fire on a `@provides` that names a field the connector
  response does not carry?
- Should the core's `entity-*` rules and these target rules share one
  family, and which of them become warnings when the user's other subgraphs
  are unavailable?

Rule severities and messages can be adjusted per target through the core's
`rule_overrides`, and every finding records whether the core or this target
raised it.
