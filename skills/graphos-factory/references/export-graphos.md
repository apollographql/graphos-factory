# Export to a GraphOS graph

This reference is owned by the next pass, the one that writes the
graphos-factory instructions, and it is not written yet. This page holds
the open questions so that pass has a place to answer them. Nothing below is
a decision, and an agent following this skill must not treat a question as
guidance.

## How a finished subgraph reaches a GraphOS graph

Three mechanisms are candidates, and none is chosen:

- `rover subgraph publish` against a named graph and variant.
- A pull request to the user's own schema repository.
- A file drop, where the skill writes the schema and its connector
  configuration to a directory the user names and stops.

The question is which of these the skill offers, whether it offers more than
one, and which of them needs credentials the skill must never handle. The
core binary never fetches from or posts to a network, so any mechanism that
talks to GraphOS is a script under this target's `scripts/`, not a
subcommand. This touches the core's export boundary, which is a projection
of a workspace into a directory plus a script that moves it.

## The publication gate

Whatever the mechanism, something decides when a subgraph may be published.
The question is what that gate checks: only that every offline evidence
layer passed and that no selected operation lacks executed evidence, or also
a composition check against the target graph (see `verification.md`). A
zero-case unit layer and a skipped live layer have to be reported, never
counted as a pass, and the gate has to say so in the same terms the
`evidence/latest.json` report uses.

## Whether `evidence` records publication

The open point is whether publication is itself an evidence layer, a field
on the report, or outside the report entirely. Recording it makes the
validation table show it automatically, but it also puts a network-dependent
step into a report that the core otherwise produces offline and that CI
re-checks against the lock.

## Placeholders: keep them, or author literals

The core renders `{{BASE_URL}}` and `{{AUTH_EXPR}}` from `template.yaml` so
that every layer can run against a local value. The question is whether this
target keeps those placeholders and renders literal values at export, or
authors literals directly and declares no placeholders. The second choice
removes `render` from this target's path, and it changes what `template.yaml`
is for: still the local value the layers run against, or not required at all.
This touches the core's placeholder set, the `unknown-placeholder` and
`missing-template` lint rules, and the lock's list of output files, which
the target supplies.
