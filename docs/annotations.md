# Source annotation grammar

Strandmap scans UTF-8, non-binary files selected by the repository scan config.
Markers are literal configurable strings. The defaults recognize both concise
and namespaced forms:

```text
@anchor ID key=value ...
@strandmap anchor ID key=value ...

@strand STRAND_ID key=value ...
@strandmap strand STRAND_ID key=value ...

@relation TYPE key=value ...
@strandmap relation TYPE key=value ...
```

Values are shell-tokenized: whitespace separates tokens, quotes preserve
whitespace, and backslashes escape characters. Unrecognized key/value pairs are
stored as typed custom attributes. `true`, `false`, `null`, integers, and finite
floating-point values become their corresponding JSON types.

Multiple annotations can share a line:

```text
// @anchor api.handler @strand request-contract role=consumer @relation tested-by to=test.handler
```

## Anchor keys

| Key | Meaning |
| --- | --- |
| `id` | Alternative to the first positional ID. |
| `target` | Stable URI or other target identity. |
| `kind` | Free-form anchor type. |
| `path` | Repository-relative location; defaults to the containing file. |
| `symbol` | Language-specific symbol identity. |
| `language` | Language identifier. |
| `watch` | `node`, `line`, or `file`. Omit it for the normal language-sensitive default. |
| `fingerprint` | Optional structural/content recovery fingerprint. |
| `tags` | Comma-separated tags. |

A standalone annotation in Rust, JavaScript/JSX, Python, shell, Lean, or TLA+
defaults to `watch=node`. Strandmap attaches it to the following syntax node or
declaration and derives the current inclusive span during every index build.
The resolved span starts at the declaration, not the annotation; the annotation
line is retained separately as provenance and still participates in change
matching. Moving the declaration, inserting lines above it, or changing its
size does not require editing the annotation.

Use `watch=line` for a trailing parameter, field, or token annotation. Its
current line is the physical annotation line, not an authored number. Use
`watch=file` for file-level source markers. Other extensions default to
`watch=file`; they may opt into the structural resolver with `watch=node`.
Watch mode controls change matching. During context retrieval, a local source
match still resolves to its containing function or section; a file watch does
not force the complete file when a bounded selected range represents it.

`line`, `lines`, `line_start`, `line_end`, and `watch=range` remain readable as
legacy source syntax so existing repositories can migrate. They produce a
`static-source-location` warning and should not be authored. Run:

```sh
strandmap migrate dynamic-locations
```

The migration validates that anchor IDs, memberships, and relations are
unchanged and that every resulting dynamic anchor resolves before writing any
file. `--check` reports legacy locations without modifying the repository.

## Strand membership keys

| Key | Meaning |
| --- | --- |
| `anchor` | Anchor ID; otherwise the nearest active anchor is used. |
| `role` | Free-form role in this strand. |
| `required` | Whether review normally requires a disposition; defaults true. |
| `intent` | Declares an annotation-only strand when no sidecar exists. |
| `title`, `scope`, `tags` | Annotation-only strand metadata. |

An explicit anchor remains active for the configured `anchor_block_gap`, which
allows adjacent comment lines. Without an active or explicit anchor, Strandmap
creates a deterministic implicit source anchor when `implicit_anchors` is true.

## Relation keys

| Key | Meaning |
| --- | --- |
| `kind` / `type` | Alternative to the positional relation type. |
| `from` | Source anchor; otherwise the active anchor is used. |
| `to` | Required destination anchor. |
| `strand` | Optional owning strand. |
| `bidirectional` | `true` or `false`; defaults false. |

Impact traversal considers both directions because either endpoint changing may
make the other stale. Direction remains available in JSON, DOT, and Mermaid
exports for consumers that interpret the relationship semantically.

`description`, `rationale`, and `reason` are not supported metadata keys. Put
the compatibility contract in the strand `intent`; use compact anchor IDs,
roles, and relation kinds for the rest of the graph.

## Deleted annotations

Git diff parsing retains removed lines and scans them for annotations. Deleting
the only declaration of a strand therefore still surfaces that strand in the
change's context packet instead of silently erasing the connection.
