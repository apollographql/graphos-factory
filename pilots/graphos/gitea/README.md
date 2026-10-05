# Gitea (pilot)

Apollo Connectors subgraph for the Gitea REST API v1, built with the
`graphos-factory` binary, for its target (`skill.name` in
`.factory/workspace.yaml`) as a **CI live fixture**: a workspace whose live
layer runs on every CI job, writes included, with no vendor account and no
shared credential. Eight operations over four resources; the reasoning
behind every shape is in [`.factory/decisions.json`](.factory/decisions.json).

## Why Gitea

Gitea is open source, ships a single static binary with an embedded SQLite
database, and publishes its Swagger 2.0 description (467 operations). CI
downloads the pinned binary, starts it on the loopback interface, creates an
admin user, mints an access token for that job only and seeds one repository
with one issue and one label, in about ten seconds, with a fixture script
that CI runs before the evidence step.
Every selected operation then runs against a real API whose data the test
knows, so `tests/live.yaml` asserts values, not just shape (D-0001, D-0008).

## Scope

| Query | Source |
|---|---|
| `gitea_version` | `GET /version` |
| `gitea_searchRepos(q, topic, includeDesc, uid, priorityOwnerId, teamId, starredBy, private, isPrivate, template, archived, mode, exclusive, sort, order, page, limit)` | `GET /repos/search` |
| `gitea_repo(owner, repo)` | `GET /repos/{owner}/{repo}` |
| `gitea_listIssues(owner, repo, state, labels, q, type, milestones, since, before, createdBy, assignedBy, mentionedBy, page, limit)` | `GET /repos/{owner}/{repo}/issues` |
| `gitea_issue(owner, repo, index)` | `GET /repos/{owner}/{repo}/issues/{index}` |
| `gitea_currentUser` | `GET /user` |
| `gitea_user(username)` | `GET /users/{username}` |

| Mutation (`@tag(name: "write")`) | Source |
|---|---|
| `gitea_createIssue(owner, repo, title, body, assignees, labels, milestone, dueDate, ref, closed)` | `POST /repos/{owner}/{repo}/issues` |

## Template variables

| Variable | Test default |
|---|---|
| `{{BASE_URL}}` | `http://127.0.0.1:3000/api/v1` — the Swagger `basePath` is a template, so the URL carries `/api/v1` itself |
| `{{AUTH_EXPR}}` | the access token; sent as `Authorization: token <token>` |

## Limitations & exclusions

- **459 of the API's 467 operations are excluded** for pilot scope (D-0001);
  the eleven the reader marks unsupported (binary and text bodies, multipart
  uploads) carry its reason. Each is listed in `.factory/selection.yaml`.
- **Repository merge configuration is not exposed** (D-0005): the `allow_*`
  switches, tracker/wiki integration settings, mirror schedule, `parent`
  and `repo_transfer`. Everything else the API returns is mapped and typed.
- **Pagination is `page`/`limit` only** (D-0002): Gitea reports totals in
  response headers a selection cannot read.
- **The spec is patched** (`patches` in sources.lock.yaml): six `Issue` properties the instance
  returns as null (`assignee`, `assignees`, `milestone`, `pull_request`,
  `closed_at`, `due_date`) carry `x-nullable: true` in the working copy;
  `.factory/sources.lock.yaml` records the patches over the untouched
  upstream, with the recorded fixture as evidence.
- **Two conformance waivers** (D-0010): Gitea declares its 404 and 422
  responses without a schema, so the recorded error bodies are `waived`
  in `.factory/selection.yaml` rather than left `unchecked`; lint reports
  the waivers the day the spec documents them.
- **Eighteen int64-backed numbers are `String`, not `Int`** (D-0011): the
  three magnitudes — `Gitea_Repository.size`, `Gitea_Attachment.size`,
  `Gitea_Issue.timeEstimate` — and fifteen counters across `Gitea_User`,
  `Gitea_Repository`, `Gitea_Milestone`, `Gitea_Attachment` and
  `Gitea_Issue`. GraphQL `Int` is 32-bit and Gitea declares every one of them
  `format: int64` with no bound, so the value is carried exactly and the
  caller parses it. `path->match([null, null], [@, @->jsonStringify])` keeps a
  JSON null null; the bare `->jsonStringify` would make it the string
  `"null"`, which this suite cannot catch; a separate CI probe of the
  mapping proves it. `gitea_issue(index:)` takes `ID!` for the same reason: an
  integer literal still coerces, so existing queries are unchanged.
- **Operation descriptions end with the returned field names** (F-0001):
  each of the seven object- or list-returning root fields closes its doc
  comment with `Returns: …`, capped at 14 names in declaration order — every
  one of the seven reaches the cap and carries `(+N more)`, so the doc comment
  names a prefix of the type, not all of it. `gitea_version` returns a scalar
  and carries no such line.
- **Object-typed entries in the Returns line render one level deep**
  (F-0002): `owner`, `user`, `assignee` and `assignees` are spelled with
  `Gitea_User`'s fields in braces, capped at 6 — `owner { id, login,
  loginName, sourceId, fullName, email (+16 more) }` — so the line names a
  prefix at both levels. Five of the seven lines changed; `gitea_currentUser`
  and `gitea_user` did not, because the first 14 fields of `Gitea_User` are
  scalars.
- **Root-field arguments carry the source's one-sentence description**
  (D-0016): 30 of the 47 arguments quote the first sentence of their
  `swagger.json` parameter description verbatim — lowercase-first, no
  terminal period, Gitea's own `pubic` typo on `isPrivate` included — so 37
  of 47 are documented. Excepted: `page` and `limit` (pagination wording
  only, and they carry none today); `mode`, `sort`, `order`, `state` and
  `type` (curated doc comments kept); `createIssue`'s `labels` and
  `milestone`, whose source text only restates `[ID!]` and `ID`; and `title`,
  `body`, `assignees` and `ref`, which have no source text.
- **One relationship field** (D-0017, ADR 0089):
  `Gitea_RepositoryMeta.ownerUser` follows an issue's `repository.owner`
  login to `GET /users/{username}` through a field-level `@connect` keyed by
  `$this.owner` — one request per issue that selects it, the same token as
  `gitea_user`. It is the repository's CI proof that a link field runs
  through unit, e2e and live. `owner` is nullable, so the field carries the
  null guard `links apply --dry-run` prints (ADR 0084), and a second e2e
  case gives it an owner-less parent. No inventory fact backs the link:
  the old hint pointed at `GET /packages/{owner}`, a list of packages, and
  since ADR 0085 `inventory links` refuses `GET /users/{username}` because
  Gitea's User spells the key `login` (D-0018). `Organization > username`
  is declined.
- **Live runs everything** (D-0008): nine cases, no exclusions — every
  operation plus the relationship field — including
  `create_issue` against the job's own instance. Locally, `GITEA_TOKEN`
  unset means the layer is `not_run`; run the fixture script to get one.

## Validation

`.factory/evidence/latest.json` records the last run. Reproduce with the
`graphos-factory` binary and the core's `graphos-factory-core/scripts/`
wrappers (`bash skills/graphos-factory/scripts/bootstrap.sh` installs the
binary). The live layer needs a running
Gitea and `GITEA_TOKEN`; the fixture script CI runs starts one and prints the
token.

```bash
graphos-factory lock      pilots/graphos/gitea --check   # no hand edits to the schema or the spec since the last apply
graphos-factory reconcile pilots/graphos/gitea --baseline HEAD   # schema ↔ selection ↔ inventory; what moved
bash compose.sh  pilots/graphos/gitea      # rover supergraph compose
bash unit.sh     pilots/graphos/gitea      # rover connector test — 10 entries, 51 assertions; every entry asserts the credential, both write bodies asserted, one relationship field
bash e2e.sh      pilots/graphos/gitea      # WireMock + Apollo Router — 13 cases (two error mappings; the write with every argument and two-element lists, and with the required ones only; the relationship field, and its owner-less parent)
graphos-factory validate pilots/graphos/gitea     # fixture/request bodies vs swagger.json — 25 conform, 3 waived (undocumented 404/422 bodies, D-0010; the empty-segment GET /users/ a null owner sends, D-0018)
graphos-factory lint    pilots/graphos/gitea      # contract + coverage
GITEA_TOKEN=$GITEA_TOKEN bash live.sh pilots/graphos/gitea   # 9 cases against the instance
graphos-factory evidence pilots/graphos/gitea     # all of the above, written to .factory/evidence/latest.json
```

The target adds one evidence layer, `supergraph_check` (composing this
subgraph against the user's existing supergraph). It is not built yet, so
`evidence` records it under `target_evidence_layers` as `not_run` with the
reason, never as a pass. The workspace carries no publication file: this
target publishes nothing yet.
