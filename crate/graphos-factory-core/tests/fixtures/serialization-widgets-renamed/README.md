# Fixture: serialization-widgets-renamed

Not a product pilot. `../serialization-widgets/`'s own workspace with every
vendor/domain identifier (source name, type/field/argument names, path
segments, GraphQL operation names, file and case names) mechanically
substituted by `rename.py` in this directory — same shapes, same structure,
different names — to expose accidental name coupling in
`crate::request_serialization` (task requirement: "one renamed variant to
detect name coupling").

Deliberately **not** renamed: `key`/`value` (the map-entry-input contract
`crate::request_serialization` reads by literal field name, the request-side
mirror of `->entries`), the phrase "explicit null" in `.factory/decisions.json` (the
textual heuristic the instrument reads to require the explicit-null case),
and every WireMock/GraphQL/Connectors protocol keyword. This `README.md` is
also excluded from the substitution pass itself (it describes the rename
relationship between the two directories, which a blind substitution would
corrupt).

Regenerate: `python3 crate/factory-core/tests/fixtures/serialization-widgets-renamed/rename.py`
(run from the repo root; it rebuilds this directory from
`serialization-widgets/` on every run and re-writes its own script file, so
reruns are safe). `crate/factory-core/tests/integration/request_serialization_fixtures.rs`'s
`renamed_widgets_fixture_produces_the_identical_obligation_set` test asserts
that `request_serialization::obligations` returns the exact same
`(id, status, denominator)` tuples for both directories — only the
`evidence_ref`/`message` text (which repeats the now-different names) may
differ.

One real bug this fixture caught while it was being built: the first
version of `rename.py` used `\bwidgets?\b` (a regex word boundary), which
Python treats as not matching across an underscore, so a snake_case case
name like `update_widget_full` renamed as a *file* (via a separate,
boundary-free filename substitution) but did not get renamed inside the
evidence log's `PASS: update_widget_full` line — every obligation then
read as unproven, because the log's case reference no longer matched any
real case file. Fixed by dropping the word boundary on the `widget(s)`
substitution itself; the corrected script is what produced this directory.

Never composed, never added to `ci.yml`.
