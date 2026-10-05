# Fixture: serialization-widgets

Not a product pilot. A compact, hand-authored workspace (schema, selection,
inventory, `tests/cases`, `tests/fixtures/mappings`, `.factory/decisions.json`, and a
recorded `.factory/evidence/latest.json` + per-case e2e log) built to exercise
`crate::request_serialization::obligations` end to end: a list-typed query
argument, a read-only POST (`searchWidgets`, a `Query` field whose connector
still sends a body), and a write (`updateWidget`) with a required id, an
optional scalar with a documented explicit-null decision (`note`), a
map-shaped input (`tags`, the five-case contract), and a nested `input`
member (`address.city`/`address.zip`).

Same content as `crate/factory-core/src/request_serialization/tests.rs`'s inline
`baseline_workspace_passes_every_obligation_with_real_counts` fixture,
materialized as real files so `crate/factory-core/tests/integration/request_serialization_fixtures.rs`
can run the instrument against it directly (an integration-level check, in
addition to the unit-level tempdir tests) and so
`serialization-widgets-renamed/` has something to mechanically rename.

Never composed, never added to `ci.yml` (no full connector build is a goal
here; the instrument's own correctness is).
