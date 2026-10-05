# Fixture: serialization-crm

Not a product pilot. A second, independently-authored compact workspace
(different vendor, different domain vocabulary, different operation
arrangement — a contacts CRM rather than a widget shop) exercising the same
`crate::request_serialization` obligation classes as
`serialization-widgets/`: a list-typed query argument (`emails`), a
read-only POST (`searchContacts`), and a write (`updateContact`) with a
required id, an optional scalar with a documented explicit-null decision
(`bio`), a map-shaped input (`attributes`, the five-case contract), and a
nested `input` member (`location.region`/`location.country`).

This is the "two different specs" half of the task's proof requirement —
unlike `serialization-widgets-renamed/`, this is not a mechanical rename of
anything; it was authored independently to check that the instrument's
logic is genuinely structural (schema/inventory/test shape) rather than
coupled to `serialization-widgets`' specific names by coincidence.

Never composed, never added to `ci.yml`.
