# Memory — gitea pilot

Per-connector notes for the next agent. Vendor-independent lessons go to
the skill's `references/lessons.md` instead.

## Running the fixture

- CI's fixture script (`start`) downloads the pinned Gitea
  static binary, writes an `app.ini` (SQLite, `INSTALL_LOCK`, registration
  off, no mailer), starts it on 127.0.0.1:3000, creates the admin user
  `fixture`, mints an access token and seeds `fixture/fixture-repo` with one
  open issue (#1, "First fixture issue") and one label (`bug`). It prints
  `GITEA_TOKEN=<token>`; CI exports it so `live.sh` runs. The token exists
  only for that job.
- Gitea refuses to run as root (`Gitea is not supposed to be run as root`).
  The script runs it as the invoking user on the CI runner and under a
  dedicated user (`setpriv`) when invoked as root, as in the sandbox.
- The spec's `basePath` is the template `{{.SwaggerAppSubUrl}}/api/v1`, so
  the reader records no base URL; `BASE_URL` must carry `/api/v1`.

## Auth

- `Authorization: token <access token>`. Gitea also accepts `Bearer`; `token`
  is what its own docs show. The prefix stays outside `{{AUTH_EXPR}}`.
- Unauthenticated: 401 `{"message": "token is required"}`, no `errors` key.

## Vendor quirks (spec vs reality)

- **`User` carries an undocumented `username`** (same value as `login`).
  Fixtures recorded from the instance keep it; it is not mapped, and the
  spec's `User` has no `additionalProperties: false`, so the oracle accepts it.
- **Empty strings, not nulls.** Optional text fields the user never set come
  back as `""` (`full_name`, `location`, `website`, `language`, `link`,
  `original_url`), and unset dates as sentinels: `last_login`
  `0001-01-01T00:00:00Z` (never logged in) or `1970-01-01T00:00:00Z`, and
  `archived_at` `1970-01-01T00:00:00Z` on a live repository. Null does appear
  for `milestone`, `assignee`, `assignees`, `pull_request`, `closed_at`,
  `due_date`.
- **`errors` in an error body is `null`**, not `[]`, and absent on 401/422
  (D-0004).
- **`Repository.owner.active` is `false` in an embedded owner** while
  `GET /user` says `true` for the same account: the embedded user is a
  cut-down projection. Do not assert `active` across operations.
- **`is_private`'s spec description reads `pubic`** (`show only pubic,
  private or all repositories (defaults to all)`). An upstream typo in
  `swagger.json`, published verbatim on `gitea_searchRepos(isPrivate:)` under
  ADR 0040; the correction is a `codify --source` patch recorded in
  `sources.lock.yaml`, not a schema edit.

## Testing gotchas

- Fixtures assert `Authorization: token test-token`; `e2e.sh` exports
  `GITEA_TOKEN=test-token` so a real token never reaches WireMock.
- `gitea_listIssues` list-shaped response: the connector selection has no
  envelope, so the WireMock body is the bare array and the unit payload too.
- `labels: [ID!]` reaches the body as an array of strings when driven by the
  unit framework (every `$args` scalar is a string there); the API wants
  integers. The e2e case `create_issue` asserts the body the router really
  sends (`"labels": [1]`); the unit entry asserts everything but `labels`.
- The relationship field `Gitea_RepositoryMeta.ownerUser` (D-0017) has no
  stubs of its own: its e2e case `issue_repository_owner` is answered by
  `issue.json` (repository.owner = "fixture") and `user.json`
  (`/users/fixture`), both tagged with it in `x-cases`. A second stub on the
  same request would be a `fixture-collision`. Its live case needs only the
  seed's issue #1.
- `RepositoryMeta.owner`'s `candidate_entity_link` fact targeted
  `get:/packages/{owner}` (`list_context: true`), the owner's package list.
  The owner's record is `get:/users/{username}`, which Gitea answers for a
  user or an organisation; the `links:` entry names that one. Since crate
  0.5.51 (ADR 0085) there is no fact at all: Gitea's User spells the key
  `login`, so `inventory links` refuses `get:/users/{username}` and lists 0
  candidates. The entry is a judgement (D-0018); lint and reconcile accept it.
- `RepositoryMeta.owner` is not in the spec's `required`, so the field is
  nullable and `ownerUser` carries the ADR 0084 null guard. The null-parent
  e2e case `list_issues_repository_owner_null` is hand-built: the seed never
  returns an owner-less repository. Its parent stub omits `owner` rather
  than sending `null` — the spec allows no null string, and conformance
  fails a `null` there. `GET /api/v1/users/` (the empty segment) answers
  404, `text/plain`, `404 page not found`; `user_empty_segment.json` carries
  that and is waived as unmatched, since no operation describes it. The
  router sends that request for the null parent even with the guard (ADR
  0084); the guard only maps the 404 to a null `ownerUser`.
