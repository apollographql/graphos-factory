# Federation subgraph: entities and composition

This reference is owned by the next pass, the one that writes the
graphos-factory instructions, and it is not written yet. This page holds
the open questions so that pass has a place to answer them. Nothing below is
a decision, and an agent following this skill must not treat a question as
guidance.

## Which operations become entities, and which directives apply

The question has two halves. First, which operations become `@key` entities
by default: every GET-by-id that the inventory finds, only those a person
selects, or only those another selected type refers to. Second, when each of
`@shareable`, `@external`, `@requires` and `@provides` applies to a field
the skill generates, and what in a workspace tells the skill to apply it.
This touches `connectors-language.md § Entities` (in the shared core), which
today describes the entity form (`@key` plus a type-level `@connect`) and the
`resolvable_key` rule, and it touches the `entity-*` lint rules, which assume
a subgraph that owns each entity it declares.

## A relationship field whose target type lives in another subgraph

Today the `links:` block in `selection.yaml` proposes a field that points at
a type this same subgraph owns, and `links apply --dry-run` prints the
field-level `{$this.<fk>}` connector for it. The question is how a link
differs when the target type is defined in another subgraph: whether the
field returns a stub entity carrying only the key, whether the target type
is declared here with `@key(resolvable: false)`, and whether the host-type
and credential checks in the `link-*` lint rules still apply when no
by-id connector exists in this subgraph. The `links:` block, its lint rules
and the `candidate_entity_link` fact in the inventory are all in the core.

## More than one `@source` in a workspace

The core models one host and one credential per workspace, and its lint
reports a second `@source` as a finding whose severity a target sets. A
Federation subgraph can legitimately wrap several upstream hosts, so the
question is whether this target models multiple sources at all, and if so
what changes: the workspace contract's single `source` entry, the
`{{BASE_URL}}`-style variables per source, the per-source credential, and the
unit and end-to-end fixtures that name a source. Until this is answered the
target reports a second `@source` as a warning (`multiple-sources`, downgraded
from the core's error) and turns `commented-source` off: unmodelled, not
forbidden.

## Directive imports in `ComposeConfig`

A related open point is which directives the generated `@link` must import
for this target, and whether the Federation spec version is pinned or taken
from the user's existing graph. `ComposeConfig` supplies both, together with
any extra compose inputs, and the core's `federation-drift` lint checks the
import list against it. A second `@source` would also change what compose
has to be given.
