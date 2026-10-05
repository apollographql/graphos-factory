# Contract schemas

JSON Schemas for the nine machine-readable files in a service workspace's
`.factory/` directory. They are also the **interface between the skill and
any host UI**: such a UI writes `selection.yaml` and reads `inventory.json` and `evidence/latest.json`, and
the skill refuses to act on a file that does not satisfy its schema.

Each schema's `$id` is `https://github.com/apollographql/graphos-factory/schemas/<file>`:
the public repository the core is mirrored to, so the identifier is the same
in every tree that carries the schema. An `$id` is a
name; nothing fetches it.

| Schema | File it governs | Written by |
|---|---|---|
| [`workspace.schema.json`](workspace.schema.json) | `.factory/workspace.yaml` | the skill, at `init` |
| [`inventory.schema.json`](inventory.schema.json) | `.factory/inventory.json` | `graphos-factory-core inventory`, at `discover` |
| [`selection.schema.json`](selection.schema.json) | `.factory/selection.yaml` | the user (by hand or through a host UI), or the skill on the user's instruction |
| [`evidence.schema.json`](evidence.schema.json) | `.factory/evidence/latest.json` | the skill, at `validate` |
| [`applied-lock.schema.json`](applied-lock.schema.json) | `.factory/applied.lock.yaml` | `graphos-factory-core lock` at the end of every `apply`. New locks include optional skill, binary, toolchain, input, and output provenance. |
| [`sources-lock.schema.json`](sources-lock.schema.json) | `.factory/sources.lock.yaml` | the skill at `init` and `discover` (docs/probe entries); `graphos-factory-core sources pin` (kind, version, upstream, hashes) and `codify --source` (`patches`) for pinned documents |
| [`context.schema.json`](context.schema.json) | `.factory/context.yaml` (optional; present only when an assessment has requirements to track — the generic/specialized decision itself is `workspace.yaml`'s `context_mode`) | the agent during intake and context reassessment; `context capture` records local artifacts; `context check` and lint validate them |
| [`decisions.schema.json`](decisions.schema.json) | `.factory/decisions.json` (the decision log: judgement calls recorded and open questions awaiting the user, as tracked records). Decisions only: each carries its alternative, a `question` or two or more `choices` | `graphos-factory-core decisions` (its only writer); a headless agent and any UI record through the same command |
| [`findings.schema.json`](findings.schema.json) | `.factory/findings.json` (facts a reference or the wire settled, which an instrument reads or the next session needs: `omits` with reason `consumed` or `not-applicable`, `affects`, a `cites`; ids `F-nnnn`, `current` until superseded) | `graphos-factory-core findings` (its only writer); `codify --expressed --context` and `sources refresh` / `sources pin --force` write through the same module |

A target embeds the contracts of the files it adds to a workspace beside
these (`Target::embedded_schemas`); they live with the target's
code, not here, and `lint` checks them with the same validator.

The `.factory/` contracts use `contract_version: 1`, except
`evidence/latest.json` (2) and `selection.yaml`, which is 2:
an override or waiver may carry `context`, and its `decision:` is optional.
The schema reads a version-1 selection too, as long as it carries no
`context`; every writer that adds one writes 2, and `decisions migrate
--split` upgrades the file. `sources.lock.yaml` gained an optional `context`
on a `patches[]` entry and stays at 1. Existing applied locks remain valid
without the optional provenance block.

For a breaking change, update the version in the schema and every writer. The version is
what lets a UI refuse a workspace it does not understand rather than
misread one.

## Validating

`graphos-factory-core inventory build` checks its own output before writing it, and
`graphos-factory-core lint` checks the files in a workspace (a target's own files with
its own embedded schemas):

```bash
graphos-factory-core inventory validate
graphos-factory-core lint .
```

The nine schemas are embedded into the binary at build time (`crate/graphos-factory-core/src/schemas.rs`),
so a workspace never has to find this directory; `--schemas DIR` on lint,
inventory, infer, context, decisions, findings and evidence overrides them for development. The validator
(`crate/graphos-factory-core/src/jsonschema.rs`) implements the subset of JSON Schema these files
use — `$ref` to local `#/$defs`, `type`, `enum`, `const`, `required`,
`properties`, `patternProperties`, `additionalProperties`, `items`,
`pattern`, `minLength`, `format: date-time`, `minimum`/`maximum`,
`minItems`/`uniqueItems`, `minProperties`/`maxProperties`,
`propertyNames`, `oneOf`/`anyOf`/`allOf`/`not`, `if`/`then`/`else`.

Keep the schemas within that subset, or teach the validator the keyword you
need and test it in `crate/graphos-factory-core/tests/integration/jsonschema.rs`.

## Rules the schemas encode on purpose

- **`inventory.json` is facts; `selection.yaml` is judgements**.
  Every field
  of the inventory is checkable against the source document, the file is
  regenerable at any time, and it never governs the schema — which is why
  `response` records `array_root_properties` and `root_property_count` and
  has no `envelope`. The envelope, like the GraphQL root and the exposed
  pagination, is a judgement and lives in `selection.yaml`, where
  `$defs.response` carries it with a `confirmed` flag so a tool's draft
  cannot become a schema's shape by default. `applied-lock.schema.json`
  hashes the inventory so a hand edit to it cannot pass unseen.
- **A relationship is a judgement too**.
  The inventory's `candidate_entity_link` fact says only that a property's
  name and type family match the trailing path parameter of a canonical
  GET-by-id operation; whether to expose it, as which field, on which
  host type, is `selection.yaml`'s top-level `links[]`, keyed by `shape` +
  `path` so one entry covers every operation that returns the shape.
  `selection draft` writes an entry `confirmed: false`; lint warns
  (`link-unconfirmed`) and reconcile notes it, and nothing applies it,
  until the user confirms. The GraphQL host type is derived from the root
  fields that return the shape and is never stored — a stored copy would
  drift from the schema exactly as a stored envelope suggestion would.
  `contract_version` stays 1: the key is optional and additive.
- **Authentication is recorded as the document states it, never as a
  default**.
  `api.auth[].oauth2` carries the flows a scheme declares and the
  authorization-code flow's endpoints and scopes verbatim; `api.security`
  and `operations[].security` carry the requirements in OpenAPI's own
  grammar. What a target derives from them is the target's own file,
  checked by its own rules.
- **Operation keys are `{lowercase method}:{path}`** in the inventory, the
  selection and the evidence — the same grammar the existing Apollo
  connector-generator UI emits, so selection state can move between the two
  losslessly.
- **A hand edit is codified, never fenced.** `selection.yaml` has no
  list of untouchable operations; an `overrides[]` entry must carry a
  `reason` and says what must stay true (`assert`), so the schema keeps a
  record of *why* and the agent may still change the span. The lock file
  is what makes an uncodified edit visible: a hash per span, nothing else.
- **A conformance gap is waived, never silently passed.** `validate`
  reports a body the oracle could not compare as `unchecked` (no documented
  shape) or `unmatched` (no such operation, unreadable body — a failure);
  `selection.yaml`'s `waivers[]` records an engineer's acceptance with a
  reason, a decision and the status accepted, and evidence carries
  `unchecked` / `waived` per operation instead of an anonymous `n/a`.
- **`skipped` and `not_run` are first-class evidence statuses.** A layer
  that did not run must carry a `reason`, and the schema has no way to
  express "assume it passed".

- **Context readiness is separate from validation.** `context check` needs no
  generated schema and distinguishes build requirements from live-only access.
  It checks declared requirements and local hashes, not remote API truth. The
  generic/specialized decision is `workspace.yaml`'s `context_mode`, so a
  generic wrapper needs no `context.yaml`; the companion file appears only to
  track requirements. An unrecorded `context_mode` reads as generic, so a
  workspace with neither marker nor file is ready.
