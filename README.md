# Strandmap

Strandmap is a version-controlled relationship graph for code, tests, schemas,
configuration, documentation, and anything else that should be reconsidered
together. It turns a task, graph seed, path, symbol, or change set into one
bounded source-bearing context packet without requiring the connected
locations to live in the same file, language, package, or repository
convention.

Strandmap's central rule is deliberately modest: a connection means “review
these together,” not “change these together.” Review records distinguish an
actual change from a compatible, irrelevant, or otherwise dispositioned
location without imposing a fixed vocabulary.

## Install

Build from source with Rust 1.85 or newer:

```sh
cargo install --path .
```

The release is one `strandmap` executable. It invokes the installed `git`
executable only for Git-based change selection; explicit `--file` analysis works
without Git.

## Start a repository

```sh
strandmap init
strandmap strand add session-token-format \
  --intent "Token creation, verification, storage, and documentation remain compatible"
strandmap anchor add auth.issue-token --path src/auth.rs --symbol issue_token
strandmap anchor add auth.verify-token --path src/auth.rs --symbol verify_token
strandmap member add session-token-format auth.issue-token --role producer
strandmap member add session-token-format auth.verify-token --role consumer
strandmap relation add session-token-format auth.issue-token auth.verify-token \
  --type mirrors
strandmap check
```

This creates human-editable metadata under `.strandmap/`. YAML, JSON, and TOML
sidecars can coexist. Generated indexes and local review records are ignored by
the metadata directory's `.gitignore` by default.

## Put anchors beside the source

Annotations are language-agnostic and can use the host language's normal
comment syntax:

```rust
// @anchor auth.issue-token target=rust://auth::issue_token
// @strand session-token-format role=producer
fn issue_token() { /* ... */ }

// @anchor auth.verify-token target=rust://auth::verify_token
// @strand session-token-format role=consumer
fn verify_token() { /* ... */ }
```

An anchor ID follows the source as it moves. For supported source languages, a
standalone annotation attaches to the following syntax node. Strandmap derives
the node's exact current line span during indexing; the reported span excludes
the annotation, whose line is retained separately as provenance. Line numbers
are never maintained in the annotation. A sidecar with the same ID can add
kind, tags, and arbitrary attributes while the annotation supplies its current
location. A `@strand`
without `anchor=` uses a nearby `@anchor`; if
there is none, a deterministic implicit anchor is created unless that behavior
is disabled.

## Gather complete context in one command

`context` is the primary agent command. Give it either task language or a known
strand. It returns the relevant functions or sections, their exact current
locations, and the strand contracts attached to them:

```sh
strandmap context --search "add customer profile avatars"
strandmap context --strand profile-avatar-contract
strandmap context --diff main...HEAD
```

Search is source-first. Strandmap divides repository text into overlapping
retrieval chunks, normalizes identifier styles, and ranks the chunks with BM25
using term rarity and document length. Strandmap annotations are removed from
the searchable text, so IDs, tags, roles, and intents cannot manufacture a
high relevance score. A winning location is expanded to its smallest coherent
syntax node or text section. Only then are covering anchors and strands looked
up and surfaced.

This order keeps the graph useful without letting graph vocabulary substitute
for source relevance:

```text
task text -> ranked source locations -> functions/sections -> anchors -> strands
```

Search works for untagged code too: it still returns the source excerpt, simply
without a strand. Quoted phrases receive a relevance bonus, leading `-terms`
exclude matching chunks, and camel case, snake case, punctuation, and namespace
separators normalize to the same terms.

The default output is already the normal agent packet. It emits search matches,
explicit source selections, change matches, and members of an explicitly named
strand. A strand discovered through search contributes its intent and matching
roles without dumping every member as source. Shared and overlapping ranges are
emitted once.

Source scope is chosen independently of change scope. `watch=file` remains
conservative for impact analysis, but a local search hit in that file produces
its function or section. A complete file is emitted when the selected context
is genuinely file-scoped and no more specific selected range represents it.

Rust `#[cfg(test)]` modules and attributed test functions are omitted from
search and source excerpts by default, and the packet states that policy. Use
`--include-tests` for one command or set `context.include_rust_tests: true` in
the repository configuration when tests should normally be included.

The default approximate source budget is 12,000 tokens. Complete excerpts are
included or omitted; functions are never silently clipped. Most agents should
not add tuning flags. When the task itself has a known boundary, it can be
expressed without a second command:

The index refreshes automatically. Source edits reparse only changed files and
update their graph entries; agents do not need a separate indexing step before
gathering context.

Agent-facing text output keeps graph context compact: each strand shows its
intent, at most four direct task entry points, and one summary for candidate or
overflow anchors. Source excerpts do not repeat reverse-reference lists. JSON
and YAML remain lossless for programmatic consumers.

```sh
strandmap context --search "profile avatar upload" --search-path apps/web/src/profile
strandmap context --search "profile avatar upload" --strand profile-avatar-contract
strandmap context --anchor profile.avatar-upload
strandmap context --symbol AvatarUploader
strandmap context --path apps/web/src/profile/avatar.ts
strandmap context --search "profile avatar upload tests" --include-tests
```

`--search-path` limits retrieval without selecting an entire directory.
`--path` is an explicit graph/source selection. Advanced source filters and
budgets remain available for exceptional workflows; they are not required for
useful default output.

## Analyze changes

```sh
# Staged, unstaged, and untracked changes relative to HEAD by default
strandmap affected

# Any revision or range Git accepts
strandmap affected --diff main...HEAD
strandmap affected --staged
strandmap affected --worktree

# Explicit whole-file changes, including outside a Git work tree
strandmap affected --file src/auth.rs --file docs/authentication.md
strandmap affected --file src/auth.rs --exclude-optional

# Complete source-bearing context for an agent
strandmap context --diff HEAD~1 --depth 2
```

File-watched anchors are conservative: any change in their file matches. Inline
code anchors use dynamically resolved syntax-node spans by default. Trailing
parameter or field annotations may use `watch=line`; their current line is
derived from the annotation itself. Changed ranges, renames, deleted
annotations, binary changes, and untracked files are handled explicitly.
Relation traversal is bounded and can be filtered by type.
`affected` is the compact impact report; `context` performs the same diff
selection while also gathering and batching source.

## Record review outcomes

```sh
strandmap review start --diff main...HEAD --id pr-482
strandmap review record pr-482 auth.issue-token changed
strandmap review record pr-482 auth.verify-token compatible --note "Encoding unchanged"
strandmap review status pr-482
strandmap review complete pr-482
```

Disposition names are free-form unless a repository configures an allow-list.
Completion fails when required anchors are missing a disposition or reviewed
files drifted. Both checks have explicit override flags for deliberate policy
exceptions.

## Explore and integrate

```sh
strandmap query --strand session-token-format --depth 2
strandmap query --anchor auth.issue-token --view dot
strandmap query --anchor auth.issue-token --view mermaid
strandmap migrate dynamic-locations --check
strandmap schema config
strandmap schema context
strandmap completions zsh
strandmap man --output strandmap.1
```

Every data-producing command supports `--format json` and `--format yaml`.
`strandmap check --strict` is suitable for CI, and `affected --require-match`
can enforce that a selected change set touches at least one strand.

## Data model

- **Anchor** — a stable identity and an optional target URI, resolved source
  location, symbol, type, tags, and custom attributes.
- **Strand** — an intent-bearing hyperedge containing any number of anchors.
- **Member** — an anchor's role, required/optional status, and custom
  attributes within one strand.
- **Relation** — a typed, optionally bidirectional edge between anchors.
- **Review** — a durable snapshot of affected anchors and their dispositions.

No role, relation type, tag, scope, URI scheme, or disposition vocabulary is
hard-coded. See [the metadata specification](docs/metadata.md),
[annotation grammar](docs/annotations.md), [configuration reference](docs/configuration.md),
[migration guide](docs/migration.md), and
[agent operating guide](docs/agent-integration.md), including the one-command
context workflow for low-token agent use.

The repository includes a runnable [multi-artifact example](examples/demo).

## License

Licensed under the MIT license.
