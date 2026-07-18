# Security policy

Please report vulnerabilities privately through the security advisory feature
of the canonical repository rather than opening a public issue.

Strandmap treats repository content and metadata as untrusted input. It does not
execute metadata-defined commands or URI targets. Git arguments are passed as
individual process arguments, metadata-controlled cache and review paths are
confined to the metadata directory, source size is bounded by configuration,
and generated state is replaced atomically.

Review records and the generated index can contain repository paths, symbols,
strand intents, and review dispositions. Their default directories are ignored
by Git; teams should apply their normal data-handling policy before sharing
them.
