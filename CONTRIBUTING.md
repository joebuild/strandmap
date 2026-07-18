# Contributing

Strandmap requires Rust 1.85 or newer. Before submitting a change, run:

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
```

Behavioral changes must include unit or integration coverage and update the
relevant specification document. Metadata schema changes must preserve version
1 decoding or introduce a new explicit schema version; silently changing the
meaning of an existing field is not accepted.

Error messages are part of the CLI interface. Keep them actionable, avoid
panics for repository input, and preserve exit status semantics documented in
the agent integration guide.
