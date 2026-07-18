use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use crate::output::OutputFormat;

#[derive(Debug, Parser)]
#[command(name = "strandmap", version, about, long_about = None, propagate_version = true)]
pub struct Cli {
    /// Repository root. Without this option Strandmap searches parent directories.
    #[arg(long, global = true, value_name = "PATH")]
    pub root: Option<PathBuf>,

    /// Metadata directory name at the repository root.
    #[arg(long, global = true, default_value = ".strandmap")]
    pub metadata: String,

    /// Output format for data-producing commands.
    #[arg(long, global = true, default_value = "human", env = "STRANDMAP_FORMAT")]
    pub format: OutputFormat,

    /// Do not rebuild a stale index automatically.
    #[arg(long, global = true)]
    pub no_auto_index: bool,

    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize Strandmap metadata in a repository.
    Init(InitArgs),
    /// Build or refresh the repository index.
    Index(IndexArgs),
    /// Validate configuration, metadata, anchors, and graph integrity.
    Check(CheckArgs),
    /// Manage strands.
    Strand {
        #[command(subcommand)]
        command: StrandCommand,
    },
    /// Manage anchors.
    Anchor {
        #[command(subcommand)]
        command: AnchorCommand,
    },
    /// Manage strand memberships.
    Member {
        #[command(subcommand)]
        command: MemberCommand,
    },
    /// Manage typed relationships between anchors.
    Relation {
        #[command(subcommand)]
        command: RelationCommand,
    },
    /// Show context affected by a Git diff or explicit files.
    Affected(AffectedArgs),
    /// Gather complete source-bearing context from a task, graph seed, path, or diff.
    Context(Box<ContextArgs>),
    /// Query and export the relationship graph.
    Query(QueryArgs),
    /// Create and manage review records for a change set.
    Review {
        #[command(subcommand)]
        command: ReviewCommand,
    },
    /// Migrate repository metadata and source annotations.
    Migrate {
        #[command(subcommand)]
        command: MigrateCommand,
    },
    /// Emit JSON Schema for Strandmap data formats.
    Schema(SchemaArgs),
    /// Generate shell completion scripts.
    Completions(CompletionArgs),
    /// Generate a manual page.
    Man(ManArgs),
}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Replace an existing default configuration, preserving other metadata.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct IndexArgs {
    /// Re-read every source file even if the index is current.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct CheckArgs {
    /// Treat warnings as a failing result.
    #[arg(long)]
    pub strict: bool,
}

#[derive(Debug, Subcommand)]
pub enum MigrateCommand {
    /// Replace authored source line ranges with dynamically resolved syntax-node spans.
    DynamicLocations(MigrateDynamicLocationsArgs),
}

#[derive(Debug, Args)]
pub struct MigrateDynamicLocationsArgs {
    /// Report legacy source locations without changing files.
    #[arg(long)]
    pub check: bool,
}

#[derive(Debug, Subcommand)]
pub enum StrandCommand {
    Add(StrandAddArgs),
    Set(StrandSetArgs),
    Remove(StrandRemoveArgs),
    List(StrandListArgs),
    Show(IdArgs),
}

#[derive(Debug, Args)]
pub struct StrandAddArgs {
    pub id: String,
    #[arg(long)]
    pub intent: String,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub scope: Option<String>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
}

#[derive(Debug, Args)]
pub struct StrandSetArgs {
    pub id: String,
    #[arg(long)]
    pub intent: Option<String>,
    #[arg(long)]
    pub title: Option<String>,
    #[arg(long)]
    pub clear_title: bool,
    #[arg(long)]
    pub scope: Option<String>,
    #[arg(long)]
    pub clear_scope: bool,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    #[arg(long = "remove-tag")]
    pub remove_tags: Vec<String>,
}

#[derive(Debug, Args)]
pub struct StrandRemoveArgs {
    pub id: String,
    /// Also remove source-independent memberships and relations owned by the strand.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct StrandListArgs {
    #[arg(long)]
    pub tag: Option<String>,
    #[arg(long)]
    pub scope: Option<String>,
}

#[derive(Debug, Subcommand)]
pub enum AnchorCommand {
    Add(AnchorAddArgs),
    Set(AnchorSetArgs),
    Remove(AnchorRemoveArgs),
    List(AnchorListArgs),
    Show(IdArgs),
}

#[derive(Debug, Args)]
pub struct AnchorAddArgs {
    pub id: String,
    /// Stable URI or other external identity for this anchor.
    #[arg(long)]
    pub target: Option<String>,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long)]
    pub path: Option<String>,
    #[arg(long)]
    pub symbol: Option<String>,
    #[arg(long)]
    pub line_start: Option<u32>,
    #[arg(long)]
    pub line_end: Option<u32>,
    #[arg(long, value_enum)]
    pub watch: Option<WatchArg>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
}

#[derive(Debug, Args)]
pub struct AnchorSetArgs {
    pub id: String,
    #[arg(long)]
    pub target: Option<String>,
    #[arg(long, conflicts_with = "target")]
    pub clear_target: bool,
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long, conflicts_with = "kind")]
    pub clear_kind: bool,
    #[arg(long)]
    pub path: Option<String>,
    #[arg(
        long,
        conflicts_with_all = ["path", "symbol", "clear_symbol", "line_start", "line_end", "watch"]
    )]
    pub clear_location: bool,
    #[arg(long)]
    pub symbol: Option<String>,
    #[arg(long, conflicts_with = "symbol")]
    pub clear_symbol: bool,
    #[arg(long)]
    pub line_start: Option<u32>,
    #[arg(long)]
    pub line_end: Option<u32>,
    #[arg(long, value_enum)]
    pub watch: Option<WatchArg>,
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    #[arg(long = "remove-tag")]
    pub remove_tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum WatchArg {
    File,
    Line,
    Range,
}

#[derive(Debug, Args)]
pub struct AnchorRemoveArgs {
    pub id: String,
    /// Remove the anchor from all sidecar memberships and relations as well.
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct AnchorListArgs {
    #[arg(long)]
    pub kind: Option<String>,
    #[arg(long)]
    pub tag: Option<String>,
    #[arg(long)]
    pub path: Option<String>,
}

#[derive(Debug, Args)]
pub struct IdArgs {
    pub id: String,
}

#[derive(Debug, Subcommand)]
pub enum MemberCommand {
    Add(MemberAddArgs),
    Remove(MemberRemoveArgs),
}

#[derive(Debug, Args)]
pub struct MemberAddArgs {
    pub strand: String,
    pub anchor: String,
    #[arg(long)]
    pub role: Option<String>,
    #[arg(long)]
    pub optional: bool,
}

#[derive(Debug, Args)]
pub struct MemberRemoveArgs {
    pub strand: String,
    pub anchor: String,
}

#[derive(Debug, Subcommand)]
pub enum RelationCommand {
    Add(RelationAddArgs),
    Remove(RelationRemoveArgs),
    /// Add a relationship that is not owned by one strand.
    AddGlobal(GlobalRelationAddArgs),
    /// Remove a relationship that is not owned by one strand.
    RemoveGlobal(GlobalRelationRemoveArgs),
}

#[derive(Debug, Args)]
pub struct RelationAddArgs {
    pub strand: String,
    pub from: String,
    pub to: String,
    #[arg(long = "type")]
    pub kind: String,
    #[arg(long)]
    pub bidirectional: bool,
}

#[derive(Debug, Args)]
pub struct RelationRemoveArgs {
    pub strand: String,
    pub from: String,
    pub to: String,
    #[arg(long = "type")]
    pub kind: Option<String>,
}

#[derive(Debug, Args)]
pub struct GlobalRelationAddArgs {
    pub from: String,
    pub to: String,
    #[arg(long = "type")]
    pub kind: String,
    #[arg(long)]
    pub bidirectional: bool,
}

#[derive(Debug, Args)]
pub struct GlobalRelationRemoveArgs {
    pub from: String,
    pub to: String,
    #[arg(long = "type")]
    pub kind: String,
}

#[derive(Debug, Clone, Args)]
pub struct ChangeArgs {
    /// Diff revision or range, for example HEAD, HEAD~3, or main...HEAD.
    #[arg(long, value_name = "REVISION")]
    pub diff: Option<String>,
    /// Analyze staged changes only.
    #[arg(long, conflicts_with_all = ["worktree", "diff"])]
    pub staged: bool,
    /// Analyze unstaged changes only.
    #[arg(long, conflicts_with = "diff")]
    pub worktree: bool,
    /// Include untracked files as whole-file changes.
    #[arg(long)]
    pub untracked: bool,
    /// Exclude untracked files.
    #[arg(long, conflicts_with = "untracked")]
    pub no_untracked: bool,
    /// Treat a path as wholly changed. May be repeated and works outside Git.
    #[arg(long = "file", value_name = "PATH")]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Args)]
pub struct AffectedArgs {
    #[command(flatten)]
    pub changes: ChangeArgs,
    /// Maximum relation traversal depth.
    #[arg(long)]
    pub depth: Option<usize>,
    /// Follow only these relation kinds. May be repeated.
    #[arg(long = "relation")]
    pub relations: Vec<String>,
    /// Include only strands carrying this tag. May be repeated.
    #[arg(long = "tag")]
    pub tags: Vec<String>,
    /// Include optional members even if repository policy excludes them.
    #[arg(long, conflicts_with = "exclude_optional")]
    pub include_optional: bool,
    /// Exclude optional members even if repository policy includes them.
    #[arg(long)]
    pub exclude_optional: bool,
    /// Fail with exit status 1 when no strand is selected.
    #[arg(long)]
    pub require_match: bool,
}

#[derive(Debug, Clone, Args)]
pub struct ContextArgs {
    #[command(flatten)]
    pub affected: AffectedArgs,
    /// Find graph entry points from task or feature language. May be repeated.
    #[arg(long = "search", value_name = "TEXT")]
    pub searches: Vec<String>,
    /// Start from an exact anchor ID. May be repeated.
    #[arg(long = "anchor", value_name = "ID")]
    pub anchors: Vec<String>,
    /// Start from every member of an exact strand ID. May be repeated.
    #[arg(long = "strand", value_name = "ID")]
    pub strands: Vec<String>,
    /// Start from anchors with this exact indexed symbol. May be repeated.
    #[arg(long = "symbol", value_name = "SYMBOL")]
    pub symbols: Vec<String>,
    /// Start from anchors in this repository path or directory. May be repeated.
    #[arg(long = "path", value_name = "PATH")]
    pub paths: Vec<String>,
    /// Limit `--search` matches to this repository path or directory. May be repeated.
    #[arg(long = "search-path", value_name = "PATH", requires = "searches")]
    pub search_paths: Vec<String>,
    /// Maximum ranked matches retained for each `--search` value.
    #[arg(long, default_value_t = 8)]
    pub search_limit: usize,
    /// Source policy: focused=exact selectors; direct=focused/search/change/direct strands; all=candidates too.
    #[arg(long, value_enum, default_value = "direct")]
    pub source: ContextSource,
    /// Include source only from matching repository-path globs. May be repeated.
    #[arg(long = "source-include", value_name = "GLOB")]
    pub source_includes: Vec<String>,
    /// Exclude source from matching repository-path globs. May be repeated.
    #[arg(long = "source-exclude", value_name = "GLOB")]
    pub source_excludes: Vec<String>,
    /// Include only these file extensions. Repeat or separate values with commas.
    #[arg(
        long = "source-extension",
        visible_alias = "source-ext",
        value_name = "EXT",
        value_delimiter = ','
    )]
    pub source_extensions: Vec<String>,
    /// Exclude these file extensions. Repeat or separate values with commas.
    #[arg(
        long = "exclude-source-extension",
        visible_alias = "exclude-source-ext",
        value_name = "EXT",
        value_delimiter = ','
    )]
    pub excluded_source_extensions: Vec<String>,
    /// Include only these detected languages. Repeat or separate values with commas.
    #[arg(long = "source-language", value_name = "LANG", value_delimiter = ',')]
    pub source_languages: Vec<String>,
    /// Exclude these detected languages. Repeat or separate values with commas.
    #[arg(
        long = "exclude-source-language",
        value_name = "LANG",
        value_delimiter = ','
    )]
    pub excluded_source_languages: Vec<String>,
    /// Add surrounding lines before and after each resolved range.
    #[arg(long, default_value_t = 0)]
    pub context_lines: u32,
    /// Merge excerpts in the same file separated by at most this many lines.
    #[arg(long, default_value_t = 2)]
    pub merge_gap: u32,
    /// Include Rust #[cfg(test)] modules and test functions in search and source excerpts.
    #[arg(long)]
    pub include_tests: bool,
    /// Approximate source token budget; bounded output compacts accounting. Use 0 for unlimited.
    #[arg(long, default_value_t = 12_000)]
    pub token_budget: usize,
}

#[derive(Debug, Clone, Copy, Default, ValueEnum)]
pub enum ContextSource {
    None,
    Focused,
    Direct,
    #[default]
    All,
}

#[derive(Debug, Args)]
pub struct QueryArgs {
    /// Start traversal at this anchor. May be repeated.
    #[arg(long = "anchor")]
    pub anchors: Vec<String>,
    /// Start traversal with every member of this strand. May be repeated.
    #[arg(long = "strand")]
    pub strands: Vec<String>,
    #[arg(long, default_value_t = 1)]
    pub depth: usize,
    #[arg(long = "relation")]
    pub relations: Vec<String>,
    #[arg(long, value_enum, default_value = "nodes")]
    pub view: QueryView,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum QueryView {
    Nodes,
    Dot,
    Mermaid,
}

#[derive(Debug, Subcommand)]
pub enum ReviewCommand {
    Start(ReviewStartArgs),
    Record(ReviewRecordArgs),
    Status(ReviewStatusArgs),
    Complete(ReviewCompleteArgs),
    Reopen(ReviewIdArgs),
    List,
}

#[derive(Debug, Args)]
pub struct ReviewStartArgs {
    #[command(flatten)]
    pub affected: AffectedArgs,
    #[arg(long)]
    pub id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReviewRecordArgs {
    pub id: String,
    pub anchor: String,
    pub disposition: String,
    #[arg(long)]
    pub note: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReviewStatusArgs {
    pub id: Option<String>,
}

#[derive(Debug, Args)]
pub struct ReviewCompleteArgs {
    pub id: String,
    #[arg(long)]
    pub allow_incomplete: bool,
    #[arg(long)]
    pub allow_drift: bool,
}

#[derive(Debug, Args)]
pub struct ReviewIdArgs {
    pub id: String,
}

#[derive(Debug, Args)]
pub struct SchemaArgs {
    #[arg(value_enum)]
    pub document: SchemaDocument,
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum SchemaDocument {
    Config,
    Strand,
    Anchor,
    Index,
    Context,
    Review,
}

#[derive(Debug, Args)]
pub struct CompletionArgs {
    #[arg(value_enum)]
    pub shell: clap_complete::Shell,
}

#[derive(Debug, Args)]
pub struct ManArgs {
    /// Output file. Defaults to stdout.
    #[arg(long)]
    pub output: Option<PathBuf>,
}
