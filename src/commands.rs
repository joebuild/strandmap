use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, Write},
    path::Path,
};

use anyhow::{Context, Result, bail};
use clap::CommandFactory;
use schemars::schema_for;
use serde::Serialize;

use crate::{
    cli::{
        AffectedArgs, AnchorCommand, Cli, Command, ContextArgs, MigrateCommand, QueryView,
        RelationCommand, ReviewCommand, SchemaDocument, StrandCommand, WatchArg,
    },
    config::Config,
    context_output::{self, RenderOptions},
    git,
    graph::{self, AffectedOptions, ContextSeeds},
    index, metadata, migration,
    model::{
        Anchor, ContextPacket, Diagnostic, Index, Location, Member, Relation, Review, Severity,
        Strand, WatchMode,
    },
    output::{self, OutputFormat},
    repo::Repository,
    review, search,
};

pub fn run(cli: Cli) -> Result<u8> {
    match cli.command {
        Command::Init(args) => init(cli.root.as_deref(), &cli.metadata, args.force, cli.format),
        Command::Schema(args) => schema(args.document),
        Command::Completions(args) => completions(args.shell),
        Command::Man(args) => man(args.output.as_deref()),
        command => {
            let repo = Repository::discover(cli.root.as_deref(), &cli.metadata)?;
            let (config, _) = Config::load(&repo)?;
            config.validate()?;
            dispatch(&repo, &config, command, cli.format, cli.no_auto_index)
        }
    }
}

fn dispatch(
    repo: &Repository,
    config: &Config,
    command: Command,
    format: OutputFormat,
    no_auto_index: bool,
) -> Result<u8> {
    let _lock = command_needs_lock(&command)
        .then(|| repo.lock())
        .transpose()?;
    match command {
        Command::Index(args) => command_index(repo, config, args.force, format),
        Command::Check(args) => command_check(repo, config, args.strict, format),
        Command::Strand { command } => command_strand(repo, config, command, format, no_auto_index),
        Command::Anchor { command } => command_anchor(repo, config, command, format, no_auto_index),
        Command::Member { command } => command_member(repo, config, command, format, no_auto_index),
        Command::Relation { command } => {
            command_relation(repo, config, command, format, no_auto_index)
        }
        Command::Affected(args) => command_affected(repo, config, &args, format, no_auto_index),
        Command::Context(args) => command_context(repo, config, &args, format, no_auto_index),
        Command::Query(args) => {
            let indexed = index::ensure(repo, config, no_auto_index)?.index;
            let anchors = args.anchors.into_iter().collect();
            let strands = args.strands.into_iter().collect();
            validate_query_seeds(&indexed, &anchors, &strands)?;
            let result = graph::query(
                &indexed,
                &anchors,
                &strands,
                args.depth,
                &args.relations.into_iter().collect(),
            );
            match args.view {
                QueryView::Nodes => {
                    if matches!(format, OutputFormat::Human) {
                        print_query(&result);
                    } else {
                        output::structured(&result, format)?;
                    }
                }
                QueryView::Dot => print!("{}", graph::to_dot(&result)),
                QueryView::Mermaid => print!("{}", graph::to_mermaid(&result)),
            }
            Ok(0)
        }
        Command::Review { command } => command_review(repo, config, command, format, no_auto_index),
        Command::Migrate { command } => command_migrate(repo, config, command, format),
        Command::Init(_) | Command::Schema(_) | Command::Completions(_) | Command::Man(_) => {
            unreachable!("handled before repository discovery")
        }
    }
}

fn command_migrate(
    repo: &Repository,
    config: &Config,
    command: MigrateCommand,
    format: OutputFormat,
) -> Result<u8> {
    match command {
        MigrateCommand::DynamicLocations(args) => {
            let report = migration::dynamic_locations(repo, config, args.check)?;
            if matches!(format, OutputFormat::Human) {
                let action = if args.check { "Found" } else { "Migrated" };
                println!(
                    "{action} {} static source locations in {} files ({} files scanned)",
                    report.annotations_migrated, report.files_changed, report.files_scanned
                );
            } else {
                output::structured(&report, format)?;
            }
            Ok(u8::from(args.check && report.annotations_migrated > 0))
        }
    }
}

fn init(root: Option<&Path>, metadata_name: &str, force: bool, format: OutputFormat) -> Result<u8> {
    let repo = Repository::for_init(root, metadata_name)?;
    fs::create_dir_all(&repo.metadata_dir)
        .with_context(|| format!("failed to create {}", repo.metadata_dir.display()))?;
    let _lock = repo.lock()?;
    for directory in ["anchors", "strands", "relations", "cache", "reviews"] {
        repo.ensure_dir(directory)?;
    }
    let config_path = ["config.yaml", "config.yml", "config.json", "config.toml"]
        .into_iter()
        .map(|name| repo.metadata_dir.join(name))
        .find(|path| path.exists())
        .unwrap_or_else(|| repo.metadata_dir.join("config.yaml"));
    if config_path.exists() && !force {
        bail!(
            "{} already exists; use --force to replace the default configuration",
            config_path.display()
        );
    }
    atomic_write(
        &config_path,
        &encode_config(&config_path, &Config::default())?,
    )?;
    let ignore_path = repo.metadata_dir.join(".gitignore");
    if !ignore_path.exists() {
        atomic_write(&ignore_path, b".lock\ncache/\nreviews/\n")?;
    }
    #[derive(Serialize)]
    struct Initialized {
        root: String,
        metadata: String,
        config: String,
    }
    let value = Initialized {
        root: repo.root.to_string_lossy().into_owned(),
        metadata: repo.relative(&repo.metadata_dir),
        config: repo.relative(&config_path),
    };
    if matches!(format, OutputFormat::Human) {
        println!("Initialized Strandmap in {}", repo.metadata_dir.display());
    } else {
        output::structured(&value, format)?;
    }
    Ok(0)
}

fn command_index(
    repo: &Repository,
    config: &Config,
    force: bool,
    format: OutputFormat,
) -> Result<u8> {
    let result = if force {
        index::BuildResult {
            index: index::rebuild(repo, config)?,
            refreshed: true,
        }
    } else {
        index::refresh(repo, config)?
    };
    if matches!(format, OutputFormat::Human) {
        let action = if result.refreshed {
            "Built"
        } else {
            "Index is current:"
        };
        let errors = count_severity(&result.index.diagnostics, Severity::Error);
        let warnings = count_severity(&result.index.diagnostics, Severity::Warning);
        println!(
            "{action} {} strands, {} anchors, {} memberships, {} relations ({} errors, {} warnings)",
            result.index.strands.len(),
            result.index.anchors.len(),
            result.index.memberships.len(),
            result.index.relations.len(),
            errors,
            warnings
        );
    } else {
        output::structured(&result.index, format)?;
    }
    Ok(0)
}

fn command_check(
    repo: &Repository,
    config: &Config,
    strict: bool,
    format: OutputFormat,
) -> Result<u8> {
    let indexed = index::rebuild(repo, config)?;
    let errors = count_severity(&indexed.diagnostics, Severity::Error);
    let warnings = count_severity(&indexed.diagnostics, Severity::Warning);
    if matches!(format, OutputFormat::Human) {
        for diagnostic in &indexed.diagnostics {
            print_diagnostic(diagnostic);
        }
        println!(
            "Checked {} strands and {} anchors: {} errors, {} warnings",
            indexed.strands.len(),
            indexed.anchors.len(),
            errors,
            warnings
        );
    } else {
        output::structured(&indexed.diagnostics, format)?;
    }
    Ok(u8::from(errors > 0 || (strict && warnings > 0)))
}

fn command_strand(
    repo: &Repository,
    config: &Config,
    command: StrandCommand,
    format: OutputFormat,
    no_auto_index: bool,
) -> Result<u8> {
    match command {
        StrandCommand::Add(args) => {
            validate_id(&args.id, "strand")?;
            if args.intent.trim().is_empty() {
                bail!("strand intent cannot be empty");
            }
            let strand = Strand {
                schema: 1,
                id: args.id,
                title: args.title,
                intent: args.intent,
                scope: args.scope,
                tags: args.tags.into_iter().collect(),
                members: Vec::new(),
                relations: Vec::new(),
                on_change: None,
                attributes: BTreeMap::new(),
            };
            let path = metadata::add_strand(repo, &strand)?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Added strand", &strand.id, &path, repo, format)?;
        }
        StrandCommand::Set(args) => {
            if args.title.is_some() && args.clear_title {
                bail!("--title and --clear-title cannot be used together");
            }
            if args.scope.is_some() && args.clear_scope {
                bail!("--scope and --clear-scope cannot be used together");
            }
            let id = args.id.clone();
            let path = metadata::update_strand(repo, &id, move |strand| {
                if let Some(intent) = args.intent {
                    if intent.trim().is_empty() {
                        bail!("strand intent cannot be empty");
                    }
                    strand.intent = intent;
                }
                if args.clear_title {
                    strand.title = None;
                } else if args.title.is_some() {
                    strand.title = args.title;
                }
                if args.clear_scope {
                    strand.scope = None;
                } else if args.scope.is_some() {
                    strand.scope = args.scope;
                }
                strand.tags.extend(args.tags);
                for tag in args.remove_tags {
                    strand.tags.remove(&tag);
                }
                Ok(())
            })?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Updated strand", &id, &path, repo, format)?;
        }
        StrandCommand::Remove(args) => {
            let indexed = index::ensure(repo, config, no_auto_index)?.index;
            if !indexed.strands.contains_key(&args.id) {
                bail!("unknown strand {:?}", args.id);
            }
            let references = indexed
                .memberships
                .iter()
                .filter(|member| member.strand.as_deref() == Some(args.id.as_str()))
                .count()
                + indexed
                    .relations
                    .iter()
                    .filter(|relation| relation.strand.as_deref() == Some(args.id.as_str()))
                    .count();
            if references > 0 && !args.force {
                bail!(
                    "strand {:?} has {references} memberships or relations; use --force to remove its sidecar definition",
                    args.id
                );
            }
            let path = metadata::remove_strand(repo, &args.id)?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Removed strand", &args.id, &path, repo, format)?;
        }
        StrandCommand::List(args) => {
            let indexed = index::ensure(repo, config, no_auto_index)?.index;
            let values: Vec<_> = indexed
                .strands
                .values()
                .map(|value| &value.value)
                .filter(|strand| {
                    args.tag
                        .as_ref()
                        .is_none_or(|tag| strand.tags.contains(tag))
                })
                .filter(|strand| {
                    args.scope
                        .as_ref()
                        .is_none_or(|scope| strand.scope.as_ref() == Some(scope))
                })
                .collect();
            if matches!(format, OutputFormat::Human) {
                for strand in values {
                    println!("{}\t{}", strand.id, strand.intent);
                }
            } else {
                output::structured(&values, format)?;
            }
        }
        StrandCommand::Show(args) => {
            let indexed = index::ensure(repo, config, no_auto_index)?.index;
            let strand = indexed
                .strands
                .get(&args.id)
                .with_context(|| format!("unknown strand {:?}", args.id))?;
            show_value(&strand.value, format)?;
        }
    }
    Ok(0)
}

fn command_anchor(
    repo: &Repository,
    config: &Config,
    command: AnchorCommand,
    format: OutputFormat,
    no_auto_index: bool,
) -> Result<u8> {
    match command {
        AnchorCommand::Add(args) => {
            validate_id(&args.id, "anchor")?;
            let location = make_location(
                args.path,
                args.symbol,
                args.line_start,
                args.line_end,
                args.watch,
            )?;
            if args.target.is_none() && location.is_none() {
                bail!("anchor requires --target or --path");
            }
            let anchor = Anchor {
                schema: 1,
                id: args.id,
                target: args.target,
                kind: args.kind,
                location,
                tags: args.tags.into_iter().collect(),
                attributes: BTreeMap::new(),
            };
            let path = metadata::add_anchor(repo, &anchor)?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Added anchor", &anchor.id, &path, repo, format)?;
        }
        AnchorCommand::Set(args) => {
            let id = args.id.clone();
            let path = metadata::update_anchor(repo, &id, move |anchor| {
                if args.clear_target {
                    anchor.target = None;
                } else if args.target.is_some() {
                    anchor.target = args.target;
                }
                if args.clear_kind {
                    anchor.kind = None;
                } else if args.kind.is_some() {
                    anchor.kind = args.kind;
                }
                if args.clear_location {
                    anchor.location = None;
                } else if args.path.is_some()
                    || args.symbol.is_some()
                    || args.clear_symbol
                    || args.line_start.is_some()
                    || args.line_end.is_some()
                    || args.watch.is_some()
                {
                    let mut location = anchor.location.take().unwrap_or(Location {
                        path: args
                            .path
                            .clone()
                            .context("--path is required when creating a location")?,
                        line_start: None,
                        line_end: None,
                        symbol: None,
                        language: None,
                        fingerprint: None,
                        watch: None,
                    });
                    if let Some(path) = args.path {
                        location.path = normalize_location_path(&path)?;
                    }
                    if args.clear_symbol {
                        location.symbol = None;
                    } else if args.symbol.is_some() {
                        location.symbol = args.symbol;
                    }
                    if args.line_start.is_some() {
                        location.line_start = args.line_start;
                    }
                    if args.line_end.is_some() {
                        location.line_end = args.line_end;
                    }
                    if let Some(watch) = args.watch {
                        location.watch = Some(watch.into());
                    }
                    validate_location(&location)?;
                    anchor.location = Some(location);
                }
                anchor.tags.extend(args.tags);
                for tag in args.remove_tags {
                    anchor.tags.remove(&tag);
                }
                if anchor.target.is_none() && anchor.location.is_none() {
                    bail!("anchor requires a target or location");
                }
                Ok(())
            })?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Updated anchor", &id, &path, repo, format)?;
        }
        AnchorCommand::Remove(args) => {
            let indexed = index::ensure(repo, config, no_auto_index)?.index;
            let referenced = indexed
                .memberships
                .iter()
                .any(|member| member.member.anchor == args.id)
                || indexed.relations.iter().any(|relation| {
                    relation.relation.from == args.id || relation.relation.to == args.id
                });
            if referenced && !args.force {
                bail!(
                    "anchor {:?} is referenced by memberships or relations; use --force to remove sidecar references",
                    args.id
                );
            }
            if args.force {
                metadata::remove_anchor_references(repo, &args.id)?;
            }
            let path = metadata::remove_anchor(repo, &args.id)?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Removed anchor", &args.id, &path, repo, format)?;
        }
        AnchorCommand::List(args) => {
            let indexed = index::ensure(repo, config, no_auto_index)?.index;
            let values: Vec<_> = indexed
                .anchors
                .values()
                .map(|value| &value.value)
                .filter(|anchor| {
                    args.kind
                        .as_ref()
                        .is_none_or(|kind| anchor.kind.as_ref() == Some(kind))
                })
                .filter(|anchor| {
                    args.tag
                        .as_ref()
                        .is_none_or(|tag| anchor.tags.contains(tag))
                })
                .filter(|anchor| {
                    args.path.as_ref().is_none_or(|path| {
                        anchor
                            .location
                            .as_ref()
                            .is_some_and(|location| &location.path == path)
                    })
                })
                .collect();
            if matches!(format, OutputFormat::Human) {
                for anchor in values {
                    let location = anchor.location.as_ref().map_or_else(
                        || anchor.target.as_deref().unwrap_or("-").to_string(),
                        display_location,
                    );
                    println!(
                        "{}\t{}\t{}",
                        anchor.id,
                        anchor.kind.as_deref().unwrap_or("-"),
                        location
                    );
                }
            } else {
                output::structured(&values, format)?;
            }
        }
        AnchorCommand::Show(args) => {
            let indexed = index::ensure(repo, config, no_auto_index)?.index;
            let anchor = indexed
                .anchors
                .get(&args.id)
                .with_context(|| format!("unknown anchor {:?}", args.id))?;
            show_value(&anchor.value, format)?;
        }
    }
    Ok(0)
}

fn command_member(
    repo: &Repository,
    config: &Config,
    command: crate::cli::MemberCommand,
    format: OutputFormat,
    no_auto_index: bool,
) -> Result<u8> {
    let indexed = index::ensure(repo, config, no_auto_index)?.index;
    match command {
        crate::cli::MemberCommand::Add(args) => {
            require_anchor(&indexed, &args.anchor)?;
            require_strand(&indexed, &args.strand)?;
            let strand_id = args.strand.clone();
            let strand_label = strand_id.clone();
            let anchor_id = args.anchor.clone();
            let role = args.role;
            let path = metadata::update_strand(repo, &strand_id, move |strand| {
                if strand
                    .members
                    .iter()
                    .any(|member| member.anchor == anchor_id && member.role == role)
                {
                    bail!("anchor {anchor_id:?} already has that role in strand {strand_label:?}");
                }
                strand.members.push(Member {
                    anchor: anchor_id,
                    role,
                    required: !args.optional,
                    attributes: BTreeMap::new(),
                });
                Ok(())
            })?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Added member", &args.anchor, &path, repo, format)?;
        }
        crate::cli::MemberCommand::Remove(args) => {
            let strand_id = args.strand.clone();
            let strand_label = strand_id.clone();
            let anchor_id = args.anchor.clone();
            let path = metadata::update_strand(repo, &strand_id, move |strand| {
                let before = strand.members.len();
                strand.members.retain(|member| member.anchor != anchor_id);
                if before == strand.members.len() {
                    bail!(
                        "anchor {anchor_id:?} is not a sidecar member of strand {strand_label:?}"
                    );
                }
                Ok(())
            })?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Removed member", &args.anchor, &path, repo, format)?;
        }
    }
    Ok(0)
}

fn command_relation(
    repo: &Repository,
    config: &Config,
    command: RelationCommand,
    format: OutputFormat,
    no_auto_index: bool,
) -> Result<u8> {
    let indexed = index::ensure(repo, config, no_auto_index)?.index;
    match command {
        RelationCommand::Add(args) => {
            require_strand(&indexed, &args.strand)?;
            require_anchor(&indexed, &args.from)?;
            require_anchor(&indexed, &args.to)?;
            if args.kind.trim().is_empty() {
                bail!("relationship type cannot be empty");
            }
            let strand_id = args.strand.clone();
            let from = args.from.clone();
            let to = args.to.clone();
            let kind = args.kind.clone();
            let path = metadata::update_strand(repo, &strand_id, move |strand| {
                if strand.relations.iter().any(|relation| {
                    relation.from == from && relation.to == to && relation.kind == kind
                }) {
                    bail!("that relationship already exists");
                }
                strand.relations.push(Relation {
                    from,
                    to,
                    kind,
                    bidirectional: args.bidirectional,
                    attributes: BTreeMap::new(),
                });
                Ok(())
            })?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Added relation", &args.kind, &path, repo, format)?;
        }
        RelationCommand::Remove(args) => {
            let strand_id = args.strand.clone();
            let from = args.from.clone();
            let to = args.to.clone();
            let kind = args.kind.clone();
            let path = metadata::update_strand(repo, &strand_id, move |strand| {
                let before = strand.relations.len();
                strand.relations.retain(|relation| {
                    !(relation.from == from
                        && relation.to == to
                        && kind.as_ref().is_none_or(|kind| &relation.kind == kind))
                });
                if before == strand.relations.len() {
                    bail!("matching sidecar relationship not found");
                }
                Ok(())
            })?;
            refresh_after_mutation(repo, config)?;
            mutation_output(
                "Removed relation",
                args.kind.as_deref().unwrap_or("*"),
                &path,
                repo,
                format,
            )?;
        }
        RelationCommand::AddGlobal(args) => {
            require_anchor(&indexed, &args.from)?;
            require_anchor(&indexed, &args.to)?;
            if args.kind.trim().is_empty() {
                bail!("relationship type cannot be empty");
            }
            let relation = Relation {
                from: args.from,
                to: args.to,
                kind: args.kind,
                bidirectional: args.bidirectional,
                attributes: BTreeMap::new(),
            };
            let path = metadata::add_global_relation(repo, &relation)?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Added global relation", &relation.kind, &path, repo, format)?;
        }
        RelationCommand::RemoveGlobal(args) => {
            let path = metadata::remove_global_relation(repo, &args.from, &args.to, &args.kind)?;
            refresh_after_mutation(repo, config)?;
            mutation_output("Removed global relation", &args.kind, &path, repo, format)?;
        }
    }
    Ok(0)
}

fn command_affected(
    repo: &Repository,
    config: &Config,
    args: &AffectedArgs,
    format: OutputFormat,
    no_auto_index: bool,
) -> Result<u8> {
    let indexed = index::ensure(repo, config, no_auto_index)?.index;
    let changes = git::changes(repo, config, &args.changes)?;
    let packet = graph::affected(
        &indexed,
        changes,
        config,
        &AffectedOptions {
            depth: args.depth,
            relations: args.relations.iter().cloned().collect(),
            tags: args.tags.iter().cloned().collect(),
            include_optional: optional_override(args.include_optional, args.exclude_optional),
        },
    );
    if matches!(format, OutputFormat::Human) {
        print_context(&packet);
    } else {
        output::structured(&packet, format)?;
    }
    Ok(u8::from(args.require_match && packet.strands.is_empty()))
}

fn command_context(
    repo: &Repository,
    config: &Config,
    args: &ContextArgs,
    format: OutputFormat,
    no_auto_index: bool,
) -> Result<u8> {
    let indexed = index::ensure(repo, config, no_auto_index)?.index;
    let include_rust_tests = args.include_tests || config.context.include_rust_tests;
    let focused_anchors = focused_anchor_ids(&indexed, args)?;
    let mut source_matches = Vec::new();
    let has_discovery = !args.searches.is_empty()
        || !args.anchors.is_empty()
        || !args.strands.is_empty()
        || !args.symbols.is_empty()
        || !args.paths.is_empty();
    let has_changes = explicit_change_selection(&args.affected.changes);
    let mut packet = if has_discovery {
        let seeds = context_seeds(repo, &indexed, args, include_rust_tests)?;
        source_matches.clone_from(&seeds.source_matches);
        let discovered = graph::context_from_seeds(
            &indexed,
            &seeds,
            config,
            &AffectedOptions {
                depth: args.affected.depth,
                relations: args.affected.relations.iter().cloned().collect(),
                tags: args.affected.tags.iter().cloned().collect(),
                include_optional: optional_override(
                    args.affected.include_optional,
                    args.affected.exclude_optional,
                ),
            },
        );
        if has_changes {
            let changes = git::changes(repo, config, &args.affected.changes)?;
            let affected = graph::affected(
                &indexed,
                changes,
                config,
                &AffectedOptions {
                    depth: args.affected.depth,
                    relations: args.affected.relations.iter().cloned().collect(),
                    tags: args.affected.tags.iter().cloned().collect(),
                    include_optional: optional_override(
                        args.affected.include_optional,
                        args.affected.exclude_optional,
                    ),
                },
            );
            graph::merge_context(affected, discovered)
        } else {
            discovered
        }
    } else {
        let changes = git::changes(repo, config, &args.affected.changes)?;
        graph::affected(
            &indexed,
            changes,
            config,
            &AffectedOptions {
                depth: args.affected.depth,
                relations: args.affected.relations.iter().cloned().collect(),
                tags: args.affected.tags.iter().cloned().collect(),
                include_optional: optional_override(
                    args.affected.include_optional,
                    args.affected.exclude_optional,
                ),
            },
        )
    };
    packet
        .strands
        .sort_by(|left, right| right.direct.cmp(&left.direct).then(left.id.cmp(&right.id)));
    if matches!(format, OutputFormat::Human) {
        print!(
            "{}",
            context_output::render(
                repo,
                &packet,
                RenderOptions {
                    source: args.source,
                    context_lines: args.context_lines,
                    merge_gap: args.merge_gap,
                    token_budget: (args.token_budget > 0).then_some(args.token_budget),
                    focused_anchors: &focused_anchors,
                    source_matches: &source_matches,
                    include_rust_tests,
                    source_includes: &args.source_includes,
                    source_excludes: &args.source_excludes,
                    source_extensions: &args.source_extensions,
                    excluded_source_extensions: &args.excluded_source_extensions,
                    source_languages: &args.source_languages,
                    excluded_source_languages: &args.excluded_source_languages,
                },
            )?
        );
    } else {
        output::structured(&packet, format)?;
    }
    Ok(u8::from(
        args.affected.require_match && packet.strands.is_empty() && source_matches.is_empty(),
    ))
}

fn context_seeds(
    repo: &Repository,
    index: &Index,
    args: &ContextArgs,
    include_rust_tests: bool,
) -> Result<ContextSeeds> {
    let mut seeds = ContextSeeds::default();
    let mut descriptions = Vec::new();
    let search_paths = args
        .search_paths
        .iter()
        .map(|path| normalize_location_path(path))
        .collect::<Result<Vec<_>>>()?;
    for id in &args.anchors {
        require_anchor(index, id)?;
        insert_seed(&mut seeds.anchors, id, "explicit anchor");
        descriptions.push(format!("anchor {id}"));
    }
    for id in &args.strands {
        require_strand(index, id)?;
        insert_seed(&mut seeds.strands, id, "explicit strand");
        descriptions.push(format!("strand {id}"));
    }
    for symbol in &args.symbols {
        descriptions.push(format!("symbol {symbol}"));
        for (id, indexed) in &index.anchors {
            if indexed
                .value
                .location
                .as_ref()
                .and_then(|location| location.symbol.as_deref())
                == Some(symbol)
            {
                insert_seed(
                    &mut seeds.anchors,
                    id,
                    &format!("symbol {symbol:?} matched"),
                );
            }
        }
    }
    for path in &args.paths {
        let path = normalize_location_path(path)?;
        descriptions.push(format!("path {path}"));
        for (id, indexed) in &index.anchors {
            let matches = indexed
                .value
                .location
                .as_ref()
                .is_some_and(|location| path_contains(&path, &location.path));
            if matches {
                insert_seed(&mut seeds.anchors, id, &format!("path {path:?} matched"));
            }
        }
    }
    for query in &args.searches {
        if search_paths.is_empty() {
            descriptions.push(format!("search {query:?}"));
        } else {
            descriptions.push(format!(
                "search {query:?} within {}",
                search_paths.join(",")
            ));
        }
        for hit in search::search(
            repo,
            index,
            query,
            args.search_limit,
            &search_paths,
            include_rust_tests,
        )? {
            let reason = hit.reason();
            for anchor in &hit.anchors {
                seeds
                    .anchors
                    .entry(anchor.clone())
                    .or_insert_with(|| reason.clone());
                seeds.search_anchors.insert(anchor.clone());
            }
            seeds.source_matches.push(hit);
        }
    }
    seeds.description = format!("context from {}", descriptions.join(", "));
    Ok(seeds)
}

fn focused_anchor_ids(index: &Index, args: &ContextArgs) -> Result<BTreeMap<String, usize>> {
    let mut focused = BTreeMap::new();
    let mut order = 0usize;
    for anchor in &args.anchors {
        focused.entry(anchor.clone()).or_insert(order);
        order += 1;
    }
    for symbol in &args.symbols {
        for (id, anchor) in &index.anchors {
            if anchor
                .value
                .location
                .as_ref()
                .and_then(|location| location.symbol.as_deref())
                == Some(symbol.as_str())
            {
                focused.entry(id.clone()).or_insert(order);
            }
        }
        order += 1;
    }
    for path in &args.paths {
        let path = normalize_location_path(path)?;
        for (id, anchor) in &index.anchors {
            if anchor
                .value
                .location
                .as_ref()
                .is_some_and(|location| location.path == path)
            {
                focused.entry(id.clone()).or_insert(order);
            }
        }
        order += 1;
    }
    Ok(focused)
}

fn insert_seed(seeds: &mut BTreeMap<String, String>, id: &str, reason: &str) {
    seeds
        .entry(id.to_string())
        .and_modify(|existing| {
            if !existing.contains(reason) {
                existing.push_str("; ");
                existing.push_str(reason);
            }
        })
        .or_insert_with(|| reason.to_string());
}

fn path_contains(selected: &str, located: &str) -> bool {
    located == selected
        || located
            .strip_prefix(selected)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

fn explicit_change_selection(args: &crate::cli::ChangeArgs) -> bool {
    args.diff.is_some()
        || args.staged
        || args.worktree
        || args.untracked
        || args.no_untracked
        || !args.files.is_empty()
}

fn command_review(
    repo: &Repository,
    config: &Config,
    command: ReviewCommand,
    format: OutputFormat,
    no_auto_index: bool,
) -> Result<u8> {
    match command {
        ReviewCommand::Start(args) => {
            let indexed = index::ensure(repo, config, no_auto_index)?.index;
            let changes = git::changes(repo, config, &args.affected.changes)?;
            let packet = graph::affected(
                &indexed,
                changes,
                config,
                &AffectedOptions {
                    depth: args.affected.depth,
                    relations: args.affected.relations.into_iter().collect(),
                    tags: args.affected.tags.into_iter().collect(),
                    include_optional: optional_override(
                        args.affected.include_optional,
                        args.affected.exclude_optional,
                    ),
                },
            );
            if args.affected.require_match && packet.strands.is_empty() {
                return Ok(1);
            }
            let (review, path) =
                review::start(repo, config, &indexed, &packet, args.id.as_deref())?;
            if matches!(format, OutputFormat::Human) {
                println!(
                    "Started review {} with {} required dispositions ({})",
                    review.id,
                    review.required_anchors.len(),
                    repo.relative(&path)
                );
            } else {
                output::structured(&review, format)?;
            }
        }
        ReviewCommand::Record(args) => {
            let review = review::record(
                repo,
                config,
                &args.id,
                &args.anchor,
                &args.disposition,
                args.note,
            )?;
            print_review_result(&review, format)?;
        }
        ReviewCommand::Status(args) => {
            let review = review::get(repo, config, args.id.as_deref())?;
            print_review_result(&review, format)?;
        }
        ReviewCommand::Complete(args) => {
            let indexed = index::refresh(repo, config)?.index;
            let review = review::complete(
                repo,
                config,
                &indexed,
                &args.id,
                args.allow_incomplete,
                args.allow_drift,
            )?;
            print_review_result(&review, format)?;
        }
        ReviewCommand::Reopen(args) => {
            let review = review::reopen(repo, config, &args.id)?;
            print_review_result(&review, format)?;
        }
        ReviewCommand::List => {
            let reviews = review::list(repo, config)?;
            if matches!(format, OutputFormat::Human) {
                for review in reviews {
                    println!(
                        "{}\t{:?}\t{}/{}\t{}",
                        review.id,
                        review.status,
                        review.dispositions.len(),
                        review.required_anchors.len(),
                        review.change_description
                    );
                }
            } else {
                output::structured(&reviews, format)?;
            }
        }
    }
    Ok(0)
}

fn schema(document: SchemaDocument) -> Result<u8> {
    let schema = match document {
        SchemaDocument::Config => serde_json::to_value(schema_for!(Config))?,
        SchemaDocument::Strand => serde_json::to_value(schema_for!(Strand))?,
        SchemaDocument::Anchor => serde_json::to_value(schema_for!(Anchor))?,
        SchemaDocument::Index => serde_json::to_value(schema_for!(Index))?,
        SchemaDocument::Context => serde_json::to_value(schema_for!(ContextPacket))?,
        SchemaDocument::Review => serde_json::to_value(schema_for!(Review))?,
    };
    serde_json::to_writer_pretty(io::stdout().lock(), &schema)?;
    println!();
    Ok(0)
}

fn completions(shell: clap_complete::Shell) -> Result<u8> {
    let mut command = Cli::command();
    let name = command.get_name().to_string();
    clap_complete::generate(shell, &mut command, name, &mut io::stdout());
    Ok(0)
}

fn man(path: Option<&Path>) -> Result<u8> {
    let manual = clap_mangen::Man::new(Cli::command());
    match path {
        Some(path) => {
            let mut file = fs::File::create(path)
                .with_context(|| format!("failed to create {}", path.display()))?;
            manual
                .render(&mut file)
                .context("failed to render manual page")?;
        }
        None => manual
            .render(&mut io::stdout())
            .context("failed to render manual page")?,
    }
    Ok(0)
}

fn make_location(
    path: Option<String>,
    symbol: Option<String>,
    line_start: Option<u32>,
    line_end: Option<u32>,
    watch: Option<WatchArg>,
) -> Result<Option<Location>> {
    let Some(path) = path else {
        if symbol.is_some() || line_start.is_some() || line_end.is_some() || watch.is_some() {
            bail!("--path is required with location-specific options");
        }
        return Ok(None);
    };
    let location = Location {
        path: normalize_location_path(&path)?,
        line_start,
        line_end,
        symbol,
        language: None,
        fingerprint: None,
        watch: watch.map(Into::into),
    };
    validate_location(&location)?;
    Ok(Some(location))
}

fn validate_location(location: &Location) -> Result<()> {
    if location.path.trim().is_empty() {
        bail!("location path cannot be empty");
    }
    if location.line_start.is_some_and(|line| line == 0)
        || location.line_end.is_some_and(|line| line == 0)
        || location
            .line_start
            .zip(location.line_end)
            .is_some_and(|(start, end)| end < start)
    {
        bail!("invalid line range");
    }
    Ok(())
}

fn normalize_location_path(value: &str) -> Result<String> {
    let mut normalized = value.replace('\\', "/");
    while let Some(stripped) = normalized.strip_prefix("./") {
        normalized = stripped.to_string();
    }
    if normalized.is_empty()
        || normalized.starts_with('/')
        || normalized.split('/').any(|part| part == "..")
    {
        bail!("location paths must be repository-relative and may not escape the repository");
    }
    Ok(normalized)
}

impl From<WatchArg> for WatchMode {
    fn from(value: WatchArg) -> Self {
        match value {
            WatchArg::File => Self::File,
            WatchArg::Line => Self::Line,
            WatchArg::Range => Self::Range,
        }
    }
}

fn refresh_after_mutation(repo: &Repository, config: &Config) -> Result<()> {
    index::rebuild(repo, config).map(|_| ())
}

fn require_anchor(index: &Index, id: &str) -> Result<()> {
    if index.anchors.contains_key(id) {
        Ok(())
    } else {
        bail!("unknown anchor {id:?}")
    }
}

fn require_strand(index: &Index, id: &str) -> Result<()> {
    if index.strands.contains_key(id) {
        Ok(())
    } else {
        bail!("unknown strand {id:?}")
    }
}

fn validate_query_seeds(
    index: &Index,
    anchors: &BTreeSet<String>,
    strands: &BTreeSet<String>,
) -> Result<()> {
    if anchors.is_empty() && strands.is_empty() {
        bail!("query requires at least one --anchor or --strand");
    }
    for anchor in anchors {
        require_anchor(index, anchor)?;
    }
    for strand in strands {
        require_strand(index, strand)?;
    }
    Ok(())
}

fn optional_override(include: bool, exclude: bool) -> Option<bool> {
    if include {
        Some(true)
    } else if exclude {
        Some(false)
    } else {
        None
    }
}

fn validate_id(id: &str, kind: &str) -> Result<()> {
    if id.trim().is_empty() || id.chars().any(char::is_whitespace) {
        bail!("{kind} id cannot be empty or contain whitespace");
    }
    Ok(())
}

fn count_severity(diagnostics: &[Diagnostic], severity: Severity) -> usize {
    diagnostics
        .iter()
        .filter(|diagnostic| diagnostic.severity == severity)
        .count()
}

fn print_diagnostic(diagnostic: &Diagnostic) {
    let location = diagnostic.path.as_deref().map_or_else(String::new, |path| {
        diagnostic
            .line
            .map_or_else(|| format!("{path}: "), |line| format!("{path}:{line}: "))
    });
    println!(
        "{:?} [{}] {location}{}",
        diagnostic.severity, diagnostic.code, diagnostic.message
    );
    if let Some(hint) = &diagnostic.hint {
        println!("  hint: {hint}");
    }
}

fn mutation_output(
    action: &str,
    id: &str,
    path: &Path,
    repo: &Repository,
    format: OutputFormat,
) -> Result<()> {
    #[derive(Serialize)]
    struct Mutation<'a> {
        action: &'a str,
        id: &'a str,
        path: String,
    }
    let value = Mutation {
        action,
        id,
        path: repo.relative(path),
    };
    if matches!(format, OutputFormat::Human) {
        println!("{action} {id:?} ({})", value.path);
    } else {
        output::structured(&value, format)?;
    }
    Ok(())
}

fn show_value<T: Serialize>(value: &T, format: OutputFormat) -> Result<()> {
    match format {
        OutputFormat::Human | OutputFormat::Yaml => {
            print!("{}", serde_yaml_ng::to_string(value)?);
        }
        OutputFormat::Json => output::structured(value, format)?,
    }
    Ok(())
}

fn display_location(location: &Location) -> String {
    match (location.line_start, location.line_end) {
        (Some(start), Some(end)) if start != end => format!("{}#L{start}-L{end}", location.path),
        (Some(line), _) => format!("{}#L{line}", location.path),
        _ => location.path.clone(),
    }
}

fn print_context(packet: &ContextPacket) {
    println!(
        "Changes: {} ({} files)",
        packet.changes.description,
        packet.changes.files.len()
    );
    if packet.strands.is_empty() {
        println!("No affected strands.");
    } else {
        println!("Affected strands: {}", packet.strands.len());
        for strand in &packet.strands {
            let marker = if strand.direct { "*" } else { "-" };
            println!("{marker} {} — {}", strand.id, strand.intent);
            for anchor in &strand.anchors {
                let direct = if anchor.direct { "!" } else { " " };
                let role = anchor.role.as_deref().unwrap_or("member");
                let location = anchor
                    .anchor
                    .as_ref()
                    .and_then(|anchor| anchor.location.as_ref())
                    .map(display_location)
                    .unwrap_or_else(|| "unlocated".into());
                println!(
                    "  {direct} {role}: {} ({location}) — {}",
                    anchor.id, anchor.reason
                );
            }
        }
    }
    if !packet.related_anchors.is_empty() {
        println!("Related anchors: {}", packet.related_anchors.len());
        for anchor in &packet.related_anchors {
            let location = anchor
                .anchor
                .as_ref()
                .and_then(|value| value.location.as_ref())
                .map_or_else(|| "unlocated".into(), display_location);
            println!("  {} ({location}) — {}", anchor.id, anchor.reason);
        }
    }
    if !packet.unmatched_files.is_empty() {
        println!(
            "Unmatched changed files: {}",
            packet.unmatched_files.join(", ")
        );
    }
    let errors = count_severity(&packet.diagnostics, Severity::Error);
    let warnings = count_severity(&packet.diagnostics, Severity::Warning);
    if errors > 0 || warnings > 0 {
        println!("Index diagnostics: {errors} errors, {warnings} warnings (run `strandmap check`)");
    }
}

fn print_query(result: &graph::QueryResult) {
    println!("Anchors ({}):", result.anchors.len());
    for anchor in &result.anchors {
        println!("  {}", anchor.id);
    }
    println!("Strands ({}):", result.strands.len());
    for strand in &result.strands {
        println!("  {}", strand.id);
    }
    println!("Relations ({}):", result.relations.len());
    for relation in &result.relations {
        println!(
            "  {} -[{}]-> {}",
            relation.relation.from, relation.relation.kind, relation.relation.to
        );
    }
}

fn print_review_result(review: &Review, format: OutputFormat) -> Result<()> {
    if matches!(format, OutputFormat::Human) {
        let missing = review
            .required_anchors
            .iter()
            .filter(|anchor| !review.dispositions.contains_key(*anchor))
            .count();
        println!(
            "Review {}: {:?}, {} recorded, {} required, {} missing",
            review.id,
            review.status,
            review.dispositions.len(),
            review.required_anchors.len(),
            missing
        );
        for anchor in &review.anchors {
            if let Some(disposition) = review.dispositions.get(anchor) {
                println!("  {anchor}: {}", disposition.disposition);
            } else {
                let marker = if review.required_anchors.contains(anchor) {
                    "required"
                } else {
                    "optional"
                };
                println!("  {anchor}: pending ({marker})");
            }
        }
    } else {
        output::structured(review, format)?;
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("path has no parent")?;
    fs::create_dir_all(parent)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
    set_metadata_permissions(temporary.as_file(), path)?;
    temporary.write_all(bytes)?;
    temporary.flush()?;
    temporary
        .persist(path)
        .map_err(|error| error.error)
        .with_context(|| format!("failed to replace {}", path.display()))?;
    Ok(())
}

fn encode_config(path: &Path, config: &Config) -> Result<Vec<u8>> {
    match path.extension().and_then(std::ffi::OsStr::to_str) {
        Some("yaml" | "yml") => Ok(serde_yaml_ng::to_string(config)?.into_bytes()),
        Some("json") => {
            let mut bytes = serde_json::to_vec_pretty(config)?;
            bytes.push(b'\n');
            Ok(bytes)
        }
        Some("toml") => Ok(toml::to_string_pretty(config)?.into_bytes()),
        _ => bail!("unsupported configuration format: {}", path.display()),
    }
}

fn command_needs_lock(command: &Command) -> bool {
    match command {
        Command::Strand { .. }
        | Command::Anchor { .. }
        | Command::Member { .. }
        | Command::Relation { .. }
        | Command::Migrate { .. } => true,
        Command::Review { command } => {
            !matches!(command, ReviewCommand::Status(_) | ReviewCommand::List)
        }
        _ => false,
    }
}

fn set_metadata_permissions(file: &fs::File, existing: &Path) -> Result<()> {
    if let Ok(metadata) = fs::metadata(existing) {
        file.set_permissions(metadata.permissions())?;
        return Ok(());
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(fs::Permissions::from_mode(0o644))?;
    }
    Ok(())
}
