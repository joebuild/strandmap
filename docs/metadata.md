# Metadata specification

Strandmap schema version 1 is serialized with Serde and represented identically
in YAML, JSON, and TOML. Unknown fields are retained as custom attributes. This
lets repositories attach organization-specific ownership, policy, ticket,
rollout, or generator metadata without waiting for Strandmap to recognize it.

Metadata is discovered in these locations:

- `.strandmap/strands/**/*.{yaml,yml,json,toml}`
- `.strandmap/strands.{yaml,yml,json,toml}`
- `.strandmap/anchors/**/*.{yaml,yml,json,toml}`
- `.strandmap/anchors.{yaml,yml,json,toml}`
- `.strandmap/relations/**/*.{yaml,yml,json,toml}`
- `.strandmap/relations.{yaml,yml,json,toml}`

Each file can contain one object, a top-level list, or a wrapped `strands:` or
`anchors:` list. IDs are repository-global within their type. Documents are
loaded in sorted path order; duplicate IDs are errors rather than implicit
overrides.

## Strand

```yaml
schema: 1
id: session-token-format
title: Session token compatibility
intent: Issuance, verification, storage, and docs agree on the token contract.
scope: authentication
tags: [authentication, security]
members:
  - anchor: auth.issue-token
    role: producer
    required: true
  - anchor: auth.verify-token
    role: consumer
  - anchor: docs.session-tokens
    role: documentation
    required: false
relations:
  - from: auth.issue-token
    to: auth.verify-token
    kind: mirrors
    bidirectional: true
on_change:
  include_roles: [producer, consumer, documentation]
  exclude_roles: []
  follow_relations: [mirrors, tested-by]
  depth: 2
  require_disposition: true
owner: identity-platform
```

`id` and `intent` are required. Roles and relationship kinds are arbitrary
non-empty strings. `include_roles` is an allow-list when non-empty;
`exclude_roles` is then applied as a deny-list. A member defaults to
`required: true`.

## Anchor

```yaml
schema: 1
id: auth.issue-token
target: rust://auth::tokens::issue_token
kind: code
location:
  path: crates/auth/src/tokens.rs
  line_start: 42
  line_end: 78
  symbol: issue_token
  language: rust
  fingerprint: optional-content-or-structure-fingerprint
  watch: range
tags: [security-boundary]
owner: identity-platform
```

An anchor needs a `target` or `location`. `target` accepts any URI scheme.
`file://relative/path#L10-L20` targets are resolved into locations automatically.
Other URI schemes remain stable external identities and can be located by a
source annotation with the same anchor ID.

Watch modes control change matching:

- `file`: any change to the path matches; this is the conservative default.
- `line`: a changed hunk must overlap the recorded line.
- `range`: a changed hunk must overlap the inclusive line range.

Sidecar line and range locations are intentionally static escape hatches for
artifacts that cannot carry annotations. For code that can carry a source
annotation, omit authored coordinates and let the annotation resolve its syntax
node dynamically. `watch: node` is reserved for source annotations and is
rejected in sidecars because a sidecar has no physical marker to attach.

Renames match both old and new paths. Whole-file, untracked, and binary changes
match every anchor located in that file.

## Merging source and sidecar anchors

A source annotation and sidecar may intentionally share an anchor ID. The
sidecar provides graph metadata; the source annotation provides the current
location and may fill missing target, kind, and tags. Two
independent sidecars or two conflicting source locations with the same ID are
reported as duplicates.

Relations normally live inside a strand. Repository-global relations use the
same relation object in the `relations` locations above and are indexed with no
owning strand. The CLI manages them with `relation add-global` and
`relation remove-global`.

Members may directly use a URI or `path#Lx-Ly` string as their `anchor`. Such a
reference is materialized as an anchor automatically. A bare unknown ID is an
error, which prevents misspellings from silently weakening the graph.

## Generated index

`.strandmap/cache/index.json` is an atomic, deterministic cache containing file
fingerprints, merged graph entities, provenance, and diagnostics. It is not the
source of truth and is ignored by Git by default. Commands automatically rebuild
a stale index unless `--no-auto-index` or `index.auto_refresh: false` is set.
