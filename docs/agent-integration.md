# Agent operating guide

Strandmap's normal agent interface is one source-bearing command:

```sh
strandmap context --search "<task language>"
```

If the exact architectural contract is already known, use:

```sh
strandmap context --strand <strand-id>
```

The command returns repository paths, current line ranges, actual functions or
sections, and the strand intents and roles needed to interpret them. Do not run
search, extract IDs, query the graph, and read files as separate agent steps.

## Drop-in instructions for `AGENTS.md`

```markdown
## Strandmap

When `.strandmap/` exists, gather implementation context before editing with one
command:

`strandmap context --search "<task language>"`

If the relevant contract is already known, use
`strandmap context --strand <strand-id>` instead. Use the default text output:
it contains the relevant source excerpts, paths, current ranges, and strand
contracts. Do not build a search → IDs → query → file-read loop around it.

For an existing change, use `strandmap context --diff origin/main...HEAD` or
plain `strandmap context` for the current worktree. Preserve stable anchor IDs,
encode durable relationships discovered during the work, and finish with
`strandmap check --strict` plus the repository's own checks.
```

## Before implementation

Most tasks need only one of these:

```sh
# Unknown entry points: describe the task naturally
strandmap context --search "add customer profile avatars"

# Known architectural contract
strandmap context --strand profile-avatar-contract

# Known stable source identity
strandmap context --anchor profile.avatar-upload
```

Selectors can be combined in the same invocation when the agent genuinely has
more information:

```sh
strandmap context \
  --search "profile avatar upload" \
  --strand profile-avatar-contract
```

Do not add file-type, source-policy, depth, or token flags speculatively. The
defaults are designed for the normal agent workflow. `--search-path` is useful
when the task itself has a real repository boundary:

```sh
strandmap context --search "profile avatar upload" \
  --search-path apps/web/src/profile
```

It limits where retrieval may match without selecting that whole directory or
preventing the resulting strand from pointing elsewhere.

## What search does

Search does not rank anchor IDs, tags, roles, strand intents, paths, kinds, or
custom metadata. Its pipeline is:

```text
task text
  -> normalized query terms
  -> BM25 ranking over repository source chunks
  -> smallest coherent function/section around each winning location
  -> covering anchors
  -> strand intents and member roles
  -> one deduplicated source packet
```

Repository text is split into overlapping retrieval chunks so a relevant
function is not lost at an arbitrary chunk boundary. Camel case, snake case,
punctuation, and namespace separators normalize to the same identifier terms.
BM25 uses repository-local term rarity, term frequency, and document-length
normalization. Quoted phrases receive a bonus. A leading `-term` excludes chunks
containing that term. Common task scaffolding such as `please`, `add`, and
`implement` is removed when the query contains meaningful feature terms.

Strandmap annotation text is stripped from the retrieval corpus. This prevents
a descriptive role or tag from making unrelated source appear relevant. Graph
metadata is consulted only after a source location wins.

An indexed node range is used when it precisely contains the winning location.
Otherwise, supported languages use their syntax tree to select the smallest
containing declaration; other text uses a bounded structural section. Untagged
source is still returned. It simply has no strand contract attached.

Search-discovered strands are concise: Strandmap shows their intent and the
matching member roles but does not dump every member as source. An explicitly
named `--strand` is different—the strand itself is the requested context, so its
selected members are eligible for source output.

## How source ranges are chosen

`watch=file` controls impact detection. It does not mean every local search hit
must read the complete file.

The source renderer gathers every selected request for a file before reading
it. When a bounded function or section represents the selected context, a
redundant file-wide request is suppressed. A complete file remains correct and
is emitted when the selected context is genuinely file-scoped and no bounded
selected range represents it. Small text sections may naturally resolve to the
complete file as their coherent unit.

Rust `#[cfg(test)]` modules and attributed test functions are excluded from the
retrieval corpus and source excerpts by default. The packet says when this
policy is active. Use `--include-tests` when the task is specifically about
tests. A repository whose normal context should include tests can set:

```yaml
context:
  include_rust_tests: true
```

Overlapping and nearby excerpts are merged, and the same source is emitted only
once even if several strands refer to it. The default approximate source budget
is 12,000 tokens. Complete excerpts are admitted by priority; Strandmap does not
clip a function and imply that the omitted portion was inspected.

Index maintenance is automatic and incremental. An edit reparses only changed
source files and updates the affected graph entries in place, so agents should
run the context command directly rather than adding a separate index command.

## What the packet means

Agent-facing strand sections show the contract plus at most four direct task entry
points. Candidate and overflow anchors are summarized by count and file, and
source excerpts do not repeat reverse-reference lists. JSON and YAML retain the
complete graph when a program needs it.

- `[match]` source directly answered the search.
- A connected strand was discovered from a matching source location.
- A direct strand was explicitly selected by ID.
- `intent` is the compatibility property the strand protects.
- `role` is the matched artifact's responsibility inside that contract.
- A graph connection means “consider these together,” not “edit all of them.”

Read and reason from the emitted source. Metadata explains why the source is
connected; it is not a substitute for source behavior.

## Existing changes

With no discovery selector, `context` gathers source for the current Git change:

```sh
strandmap context
strandmap context --staged
strandmap context --worktree
strandmap context --diff origin/main...HEAD
```

Search and change selection can be batched:

```sh
strandmap context \
  --diff origin/main...HEAD \
  --search "session token compatibility"
```

Use `strandmap affected` only when a compact graph impact report without source
is intentionally sufficient.

## Exceptional controls

The CLI retains explicit controls for unusual workflows: `--source none`,
`--source focused`, extension/language/path source filters, surrounding lines,
custom token budgets, and `--include-tests`. They are escape hatches, not steps
in normal use.

JSON and YAML contain the lossless graph context packet but do not embed source
files. The default text representation is the agent-facing form.

## After editing

Make a coherent edit batch. Rerun context when the selected paths, graph, or
resolved source ranges changed materially, then validate:

```sh
strandmap context --diff origin/main...HEAD
strandmap check --strict
```

Run the repository's formatters, tests, and linters separately. Strandmap
validates context relationships and review scope; it does not replace behavioral
verification.

For brownfield adoption and batched metadata migration, see the
[existing-repository migration guide](migration.md).
