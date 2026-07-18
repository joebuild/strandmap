# Strandmap demo

From the Strandmap repository root:

```sh
cargo run -- --root examples/demo check
cargo run -- --root examples/demo context --search "session token"
cargo run -- --root examples/demo context --strand session-token-format
cargo run -- --root examples/demo affected --file src/auth.rs
cargo run -- --root examples/demo query --strand session-token-format --view mermaid
```

The example connects two code anchors, a JSON schema, and documentation through
one strand and two typed relationships.
