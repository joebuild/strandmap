# Changelog

## 1.3.1 - 2026-07-18

Initial public release.

- One-command, source-bearing context gathering for task text, strands,
  anchors, symbols, paths, and Git changes.
- Source-first BM25 retrieval over coherent functions and sections; annotation
  metadata is attached after ranking and does not influence relevance.
- Compact agent-facing output with bounded source, deduplicated excerpts,
  summarized graph expansion, and no reverse-reference lists.
- Dynamic syntax-node anchors for Rust, JavaScript/JSX, Python, shell, Lean,
  and TLA+, plus checked migration from authored source coordinates.
- Rust test sections omitted by default as complete attributed declarations,
  with `--include-tests` for test-focused work.
- Incremental, content-aware indexing with atomic buffered cache updates and
  strict graph-integrity diagnostics.
- Versioned YAML, JSON, and TOML metadata; typed relations; hyperedge traversal;
  durable review records; mutation commands; schemas; completions; and man pages.
- Cross-platform CI, release binaries, an end-to-end example, and MIT licensing.
