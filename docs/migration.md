# Migrating an existing repository

Strandmap can be introduced into an established repository without changing its
language, build system, directory layout, or existing ownership conventions.
Migration means adding a version-controlled relationship model around artifacts
that already exist. It should not require renaming files, reorganizing packages,
or converting every dependency in the build graph into a Strandmap relation.

There are three valid adoption styles:

- **Sidecar-first** keeps all initial metadata under `.strandmap/` and does not
  touch existing source files.
- **Annotation-first** puts stable identities and memberships beside source
  locations using the host language's comments.
- **Hybrid** uses annotations for identities that move with source and sidecars
  for intents, non-source artifacts, and cross-cutting policy.

These styles can coexist. Choose per artifact rather than imposing one format on
the repository.

## Migrating legacy source ranges

Repositories created with early Strandmap versions may contain inline
`lines=`, `line=`, `line_start=`, `line_end=`, or `watch=range` coordinates.
Those coordinates drift when code moves. Validate and migrate them in one batch:

```sh
strandmap migrate dynamic-locations --check
strandmap migrate dynamic-locations
strandmap check --strict
```

The migration removes only authored coordinates. It preserves anchor IDs,
memberships, relations, roles, targets, kinds, and tags, resolves every new
dynamic location in memory, and writes nothing if any file is ambiguous. Commit
the source annotation changes together as a mechanical migration.

## 1. Establish a clean baseline

Install Strandmap, start from a branch with a known repository state, and run
the repository's normal test and validation commands before adding metadata.
This separates pre-existing failures from migration problems.

From the repository root:

```sh
strandmap init
```

Initialization creates:

```text
.strandmap/
  config.yaml
  anchors/
  strands/
  relations/
  cache/
  reviews/
  .gitignore
```

The generated `.gitignore` excludes `.lock`, `cache/`, and `reviews/`. Commit
the configuration and authored metadata; do not commit the generated cache or
local review sessions.

`strandmap init` refuses to overwrite an existing configuration. `init --force`
replaces the active configuration with current defaults while preserving other
metadata, so use it only when that replacement is deliberate.

If the repository already contains a `.strandmap/` directory, inspect it before
running `init`. Strandmap loads the first existing configuration named
`config.yaml`, `config.yml`, `config.json`, or `config.toml`.

## 2. Set the repository boundary

Decide whether the graph belongs at a monorepo root or whether independently
released subprojects should have separate graphs.

- One root graph can connect artifacts across packages and languages.
- Separate graphs give each project independent configuration and validation.
- Agents and CI working in an ambiguous workspace should pass
  `--root /absolute/repository/path` explicitly.

For multiple repositories, use one graph per repository. External anchors may
use stable URIs, but local change detection operates on the selected repository.
Do not make one metadata directory depend on filesystem paths outside its root.

## 3. Configure scanning before importing metadata

Review `.strandmap/config.yaml` against the repository. In particular, tune:

- `scan.include` when only selected trees should be scanned;
- `scan.exclude` for generated, vendored, fixture, cache, and large artifact
  directories;
- `scan.max_file_bytes` for unusually large source or schema files;
- `scan.hidden`, `scan.follow_symlinks`, and `scan.respect_gitignore` to match
  repository policy;
- annotation markers if the repository already has a compatible comment
  convention;
- traversal depth, relation filters, and optional-member policy;
- allowed review dispositions if review outcomes use a controlled vocabulary.

The active metadata directory is excluded automatically. An empty
`scan.include` means all otherwise eligible paths; a non-empty value is an
allow-list. Excludes are still applied.

Validate the empty graph and scan configuration:

```sh
strandmap check --strict
```

Resolve scan and configuration diagnostics before bulk authoring. Otherwise a
large import can conceal a simple root, glob, encoding, or file-size mistake.

## 4. Inventory compatibility boundaries, not every dependency

Strandmap connections mean “review these together.” Start from contracts that
can become stale across artifacts, for example:

- a serializer, parser, schema, fixtures, and API documentation;
- a configuration key, its loader, deployment values, and operator docs;
- a public API, contract tests, generated clients, and examples;
- a database migration, query layer, rollback procedure, and data model docs;
- a feature flag, both behavior branches, rollout config, and removal docs;
- an authorization decision, policy data, audit event, and security tests.

Do not mechanically copy the entire import graph, build graph, or call graph.
Those graphs express execution or compilation dependencies; Strandmap expresses
review compatibility. Add a connection only when its intent can be stated
clearly enough for a future reviewer to evaluate.

Useful inventory sources include existing architecture documents, schema
registries, CODEOWNERS, test manifests, generator configs, deployment files,
and comments describing synchronized changes. These are evidence, not automatic
proof of a Strandmap relationship.

## 5. Define stable identities

Every anchor ID is repository-global. Choose IDs that survive file moves and
symbol renames. Prefer conceptual identities such as:

```text
payments.refund-request
payments.refund-schema
payments.refund-contract-test
docs.refund-api
deploy.refund-timeout
```

Avoid IDs containing line numbers, temporary ticket numbers, or current
directory layouts. IDs are opaque to Strandmap; dots, dashes, or another naming
scheme are repository conventions rather than semantics.

For each strand, write an `intent` as the compatibility property to preserve,
not a list of files:

```text
Refund producers, consumers, schema, examples, and documentation agree on the
request fields and error behavior.
```

Roles, relation kinds, scopes, tags, URI schemes, and dispositions are all
free-form. Reuse an existing repository vocabulary where it is clear. Introduce
a small documented vocabulary where consistency helps querying, but do not
force unrelated domains into the same role names.

## 6. Choose watch precision intentionally

Each file-backed anchor has one watch mode:

- `file` matches any change in the file and is the conservative default;
- `line` matches overlap with one recorded line;
- `range` matches overlap with an inclusive line range.

Use `file` for small files, generated contracts, or artifacts whose internal
line positions are unstable. Use `range` when a large file contains independent
contracts and the range can be maintained reliably. Source annotations reduce
range-maintenance cost because the identity moves with the surrounding code.

Avoid false precision during bulk migration. A file-level match that produces a
few extra review candidates is safer than a stale range that silently misses a
change. Precision can be changed later without changing the anchor ID.

## 7. Import anchors in batches

For sidecar-first migration, place lists under `.strandmap/anchors/`. YAML, JSON,
and TOML can coexist. This example imports several artifact types in one file:

```yaml
# .strandmap/anchors/refunds.yaml
- schema: 1
  id: payments.refund-request
  kind: code
  location:
    path: src/payments/refunds.rs
    symbol: build_refund_request
    watch: file
  tags: [payments, external-contract]

- schema: 1
  id: payments.parse-refund-response
  kind: code
  location:
    path: src/payments/refunds.rs
    symbol: parse_refund_response
    watch: file

- schema: 1
  id: payments.refund-schema
  target: file://schemas/refund-request.json
  kind: schema
  location:
    path: schemas/refund-request.json
    watch: file

- schema: 1
  id: payments.refund-contract-test
  kind: test
  location:
    path: tests/refund_contract.rs
    watch: file

- schema: 1
  id: docs.refund-api
  target: file://docs/refunds.md
  kind: documentation
  location:
    path: docs/refunds.md
    watch: file
```

For annotation-first migration, add several declarations in the same source
edit and validate once afterward:

```rust
// @anchor payments.refund-request target=rust://payments::build_refund_request
// @strand refund-provider-contract role=producer
fn build_refund_request() { /* ... */ }

// @anchor payments.parse-refund-response target=rust://payments::parse_refund_response
// @strand refund-provider-contract role=consumer
fn parse_refund_response() { /* ... */ }
```

Annotations use ordinary comments and are language-agnostic. The namespaced
forms `@strandmap anchor`, `@strandmap strand`, and `@strandmap relation` are
available when concise markers might conflict with existing text.

A source annotation and sidecar may share an anchor ID intentionally: the
sidecar can provide kind, tags, and custom attributes while the
annotation supplies the current location. Two sidecars defining the same ID, or
two conflicting source locations, are errors rather than overrides.

## 8. Import complete strands, memberships, and relations

A strand sidecar can contain all review semantics for one compatibility
boundary:

```yaml
# .strandmap/strands/refund-provider-contract.yaml
schema: 1
id: refund-provider-contract
title: Refund provider request compatibility
intent: >-
  Refund producers, consumers, schema, tests, and documentation agree on
  request fields and error behavior.
scope: payments
tags: [external-contract, payments]
members:
  - anchor: payments.refund-request
    role: producer
    required: true
  - anchor: payments.refund-schema
    role: schema
    required: true
  - anchor: payments.parse-refund-response
    role: consumer
    required: true
  - anchor: payments.refund-contract-test
    role: verification
    required: true
  - anchor: docs.refund-api
    role: documentation
    required: false
relations:
  - from: payments.refund-request
    to: payments.refund-contract-test
    kind: tested-by
    bidirectional: true
on_change:
  depth: 1
  follow_relations: [tested-by]
  require_disposition: true
```

Membership says the anchors participate in one shared intent. Relations add a
more specific typed connection and allow bounded traversal beyond direct strand
membership. Do not add a relation when membership already expresses everything
a reviewer needs.

Members that are informative but not normally required for review may set
`required: false`. This is distinct from whether optional members appear in
context; traversal configuration controls inclusion, while review policy
controls required dispositions.

Repository-global relations belong under `.strandmap/relations/`. Use them when
an edge is meaningful independently of one strand. Prefer strand-owned
relations when the edge's meaning depends on the strand intent.

## 9. Generate metadata from an existing registry

Repositories with a reliable contract registry, service catalog, schema index,
or test mapping can generate sidecars. A generator should:

1. emit deterministic YAML, JSON, or TOML under the appropriate metadata
   directory;
2. preserve stable IDs across runs;
3. use one authoritative definition per strand and anchor;
4. sort output to keep reviews readable;
5. replace its owned files atomically rather than appending duplicates;
6. run `strandmap check --strict` after generation.

Generate the authoritative input schemas rather than reverse-engineering
examples:

```sh
strandmap schema anchor > /tmp/strandmap-anchor.schema.json
strandmap schema strand > /tmp/strandmap-strand.schema.json
strandmap schema config > /tmp/strandmap-config.schema.json
```

Unknown metadata fields are retained as custom attributes, so an importer can
carry ownership, catalog IDs, rollout policy, or generator provenance without
waiting for Strandmap-specific fields. Keep generated and hand-authored files
separate by path so ownership is unambiguous.

## 10. Validate every import batch

After each coherent metadata batch:

```sh
strandmap check --strict
strandmap --format json strand list
strandmap --format json anchor list
```

Do not run one CLI mutation command per imported item unless step-by-step
validation is specifically useful. Editing list sidecars or generating them in
one pass performs less I/O, rebuilds the index once, and produces less agent
output.

Test representative change scenarios using all relevant files in one call:

```sh
strandmap context \
  --file src/payments/refunds.rs \
  --file schemas/refund-request.json \
  --file tests/refund_contract.rs \
  --file docs/refunds.md
```

Also test each artifact independently. A schema-only or documentation-only
change should still select the intended strand when that artifact is a member:

```sh
strandmap context --file schemas/refund-request.json
strandmap context --file docs/refunds.md
```

For a large validation matrix, keep raw JSON out of the agent transcript:

```sh
strandmap --format json context \
  --file src/payments/refunds.rs \
  --file schemas/refund-request.json \
  > /tmp/refund-context.json

jq -c '{
  strands: [.strands[] | {id, intent}],
  unmatched_files,
  diagnostics
}' /tmp/refund-context.json
```

Inspect both false negatives and excessive noise. Missing context usually means
an incorrect path, stale range, excluded file, missing membership, or over-tight
filter. Excessive context usually means a file-level anchor is too broad, a
strand combines unrelated intents, or traversal depth is larger than needed.

## 11. Measure and declare coverage honestly

Strandmap does not assume that every repository file needs an anchor. Generated
files, vendored code, snapshots, and isolated implementation details may be
intentionally outside the graph.

`affected --require-match` fails only when the entire selection affects no
strand. It does **not** guarantee that every changed file is mapped. If repository
policy requires every selected file to be mapped, inspect `unmatched_files`
explicitly:

```sh
strandmap --format json context --diff origin/main...HEAD \
  --no-untracked > /tmp/strandmap-ci-context.json

jq -e '.unmatched_files | length == 0' /tmp/strandmap-ci-context.json
```

Apply such a policy only to paths that genuinely require mapping. For mixed
repositories, filter the changed-file scope in CI or maintain a documented
allow-list rather than creating meaningless strands solely to satisfy a count.

During a domain-by-domain migration, state which directories or contract types
are covered. An empty context outside that declared scope must not be
misinterpreted as proof that no compatibility work exists.

## 12. Add CI at the intended enforcement level

Pin Strandmap using the repository's normal tool-distribution mechanism, then
choose checks independently:

```sh
# Graph correctness
strandmap check --strict

# Require at least one affected strand for this selected change set
strandmap affected --diff origin/main...HEAD --no-untracked --require-match

# Produce a context artifact for another CI step or review bot
strandmap --format json context --diff origin/main...HEAD \
  --no-untracked > strandmap-context.json
```

Graph correctness is a sensible baseline once metadata lands. Affected-strand
or unmatched-file enforcement is a repository policy decision and can be
limited to selected paths. Pin a specific Strandmap release rather than using
an unbounded latest version in CI.

If review records are required, configure allowed dispositions and required
membership policy before enabling completion enforcement. Test the workflow on
realistic changes, including a file modified after review start so the team
understands drift detection.

## 13. Enable agents and reviewers

Add the compact Strandmap policy from the
[agent operating guide](agent-integration.md) to the repository's `AGENTS.md` or
equivalent instructions. Agents should use one context query per logical change
set. For new work, use `context --search`, `--strand`, `--anchor`, `--symbol`,
or `--path`; for existing changes, use its diff selectors. Default context
output already includes ranked, deduplicated functions or sections in one
bounded packet, so agents should not turn the result into separate search,
graph, show, and file-read steps. Do not teach routine tuning flags. Add
`--search-path` only when the task has a real repository boundary, and use the
advanced source controls only for an exceptional workflow. JSON remains
appropriate for CI and other programmatic consumers.

Human review guidance can remain short:

- a connection means review together, not necessarily change together;
- direct anchors explain why a strand was selected;
- other anchors are bounded candidates;
- intent defines what compatibility to check;
- unmatched files still receive normal review and testing.

## 14. Migrate from common legacy mechanisms

### CODEOWNERS and ownership catalogs

Keep the existing ownership system. Import owner or team values as custom
attributes when useful, but do not treat common ownership as evidence that all
owned files belong to one strand.

### Test-to-source maps

Turn stable source/test contracts into strand memberships or `tested-by`
relations. Preserve the reason the test is relevant, especially when filenames
do not make the contract obvious.

### Documentation link checkers

Keep link checking for URL integrity. Use Strandmap for semantic staleness: an
API or configuration behavior can change while every hyperlink remains valid.

### Build and package graphs

Keep the build graph authoritative for compilation order. Import only edges
that imply human review beyond what the compiler or package manager already
enforces.

### Existing source markers

If an existing comment format is compatible, configure annotation marker
strings instead of rewriting every marker immediately. Marker recognition is
literal; validate representative files before a bulk scan.

### Path-based dependency manifests

Create stable anchors for the existing paths, then point memberships and
relations at anchor IDs. Once consumers use IDs, paths and symbols can move
without rewriting the conceptual graph.

## 15. Evolve the migrated graph safely

- Preserve anchor IDs during file and symbol moves.
- Update watched locations in the same change that moves an artifact.
- Split a strand when it accumulates unrelated intents or routinely produces
  irrelevant candidates.
- Merge strands only when one compatibility statement accurately covers all
  members.
- Remove metadata in the same change that removes the underlying contract.
- Run `strandmap check --strict` after ID changes so stale memberships and
  relations cannot silently survive.
- Use `strandmap index --force` to rebuild generated state; do not hand-edit the
  cache.

Changing an anchor ID is a graph migration, not a cosmetic rename. Update its
definition, every membership, and every relation atomically in one patch, then
validate. There is no implicit aliasing because silent aliases would weaken
identity guarantees.

## Completion checklist

A repository migration is operationally ready when:

- authored configuration and metadata are version-controlled;
- cache, lock, and local review files remain untracked;
- scan boundaries match the actual repository;
- anchor IDs are stable and unique;
- every strand has a specific compatibility intent;
- member roles make candidate review understandable under the strand intent;
- representative changes select the expected strands from every relevant
  artifact direction;
- `strandmap check --strict` passes locally and in CI;
- enforcement scope and intentional unmapped areas are documented;
- agent and human review instructions describe how to interpret context;
- the repository's normal tests still pass after annotation or metadata edits.

The [metadata specification](metadata.md),
[configuration reference](configuration.md), and
[annotation grammar](annotations.md) define the complete formats used during
migration.
