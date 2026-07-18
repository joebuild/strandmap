# Configuration reference

The first existing file named `.strandmap/config.yaml`, `config.yml`,
`config.json`, or `config.toml` is loaded. Version 1 accepts omitted sections,
using the defaults emitted by `strandmap init`. Unknown fields are retained.

```yaml
version: 1

scan:
  include: []
  exclude:
    - .git/**
    - .strandmap/**
    - "**/.strandmap/**"
    - target/**
    - "**/target/**"
    - node_modules/**
    - "**/node_modules/**"
    - vendor/**
    - dist/**
    - build/**
    - "*.min.js"
    - "*.map"
    - "*.lock"
  max_file_bytes: 4194304
  hidden: false
  follow_symlinks: false
  respect_gitignore: true

annotations:
  enabled: true
  anchor_markers: ["@strandmap anchor", "@anchor"]
  strand_markers: ["@strandmap strand", "@strand"]
  relation_markers: ["@strandmap relation", "@relation"]
  anchor_block_gap: 3
  implicit_anchors: true

index:
  path: cache/index.json
  auto_refresh: true

traversal:
  depth: 1
  relation_kinds: []
  include_optional_members: true

context:
  include_rust_tests: false

reviews:
  path: reviews
  allowed_dispositions: []
  require_all_members: false

git:
  default_diff: HEAD
  detect_renames: true
  include_untracked: true
```

`scan.include` is an allow-list only when non-empty. Excludes are always
applied. Glob matching uses `/`-normalized repository-relative paths. The active
metadata directory is excluded independently of these patterns.

`index.path` and `reviews.path` are relative to the metadata directory and may
not escape it. Both are safe for atomic replacement.

An empty `traversal.relation_kinds` follows every relationship type. A non-empty
set is a default allow-list and can be overridden per invocation with repeated
`--relation` options.

`context.include_rust_tests` controls whether Rust `#[cfg(test)]` modules and
attributed test functions participate in search and source excerpts. It is
`false` by default. `strandmap context --include-tests` enables them for one
invocation without changing repository policy.

An empty `reviews.allowed_dispositions` accepts any non-empty disposition.
Repositories that need a controlled vocabulary can configure one without
changing the data model.

Generate the authoritative machine-readable schema with:

```sh
strandmap schema config
```
