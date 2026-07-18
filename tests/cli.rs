use std::{fs, path::Path, process::Command};

use serde_json::Value;
use tempfile::TempDir;

fn strandmap() -> assert_cmd::Command {
    assert_cmd::Command::cargo_bin("strandmap").expect("binary")
}

fn initialize(root: &Path) {
    strandmap()
        .args(["--root", root.to_str().unwrap(), "init"])
        .assert()
        .success();
}

#[test]
fn schemas_exclude_removed_prose_metadata() {
    let anchor = strandmap().args(["schema", "anchor"]).output().unwrap();
    assert!(anchor.status.success());
    let anchor_schema: Value = serde_json::from_slice(&anchor.stdout).unwrap();
    assert!(!anchor_schema.to_string().contains("description"));

    let strand = strandmap().args(["schema", "strand"]).output().unwrap();
    assert!(strand.status.success());
    let strand_schema: Value = serde_json::from_slice(&strand.stdout).unwrap();
    assert!(!strand_schema.to_string().contains("rationale"));
}

#[test]
fn sidecar_crud_affected_and_review_work_end_to_end() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("src/auth.rs"), "fn issue_token() {}\n").unwrap();
    fs::write(root.join("docs/auth.md"), "# Authentication\n").unwrap();
    initialize(root);

    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "strand",
            "add",
            "token-contract",
            "--intent",
            "Issuer and verifier agree",
        ])
        .assert()
        .success();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "anchor",
            "add",
            "docs.auth",
            "--path",
            "docs/auth.md",
            "--kind",
            "documentation",
        ])
        .assert()
        .success();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "anchor",
            "add",
            "auth.issue",
            "--path",
            "src/auth.rs",
            "--symbol",
            "issue_token",
        ])
        .assert()
        .success();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "relation",
            "add-global",
            "auth.issue",
            "docs.auth",
            "--type",
            "documented-by",
        ])
        .assert()
        .success();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "member",
            "add",
            "token-contract",
            "auth.issue",
            "--role",
            "producer",
        ])
        .assert()
        .success();

    let affected = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "affected",
            "--file",
            "src/auth.rs",
        ])
        .output()
        .unwrap();
    assert!(
        affected.status.success(),
        "{}",
        String::from_utf8_lossy(&affected.stderr)
    );
    let packet: Value = serde_json::from_slice(&affected.stdout).unwrap();
    assert_eq!(packet["strands"][0]["id"], "token-contract");
    assert_eq!(packet["strands"][0]["anchors"][0]["id"], "auth.issue");
    assert_eq!(packet["related_anchors"][0]["id"], "docs.auth");

    let started = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "review",
            "start",
            "--file",
            "src/auth.rs",
            "--id",
            "change-1",
        ])
        .output()
        .unwrap();
    assert!(
        started.status.success(),
        "{}",
        String::from_utf8_lossy(&started.stderr)
    );
    let review: Value = serde_json::from_slice(&started.stdout).unwrap();
    assert_eq!(review["id"], "change-1");

    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "review",
            "record",
            "change-1",
            "auth.issue",
            "compatible",
        ])
        .assert()
        .success();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "review",
            "record",
            "change-1",
            "docs.auth",
            "compatible",
        ])
        .assert()
        .success();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "review",
            "complete",
            "change-1",
        ])
        .assert()
        .success();
}

#[test]
fn annotations_and_git_diff_find_affected_strands() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add watch=file\n// @strand arithmetic role=implementation intent=\"Arithmetic behavior stays covered\"\npub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();
    run_git(root, &["init"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Strandmap Test"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "initial"]);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add watch=file\n// @strand arithmetic role=implementation intent=\"Arithmetic behavior stays covered\"\npub fn add(a: i32, b: i32) -> i32 { a.saturating_add(b) }\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "affected",
            "--no-untracked",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(packet["strands"][0]["id"], "arithmetic");
    assert_eq!(packet["changes"]["files"][0]["path"], "src/lib.rs");
}

#[test]
fn dynamic_locations_survive_lines_inserted_before_the_anchor() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add\n// @strand arithmetic role=implementation intent=stable\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();
    run_git(root, &["init"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Strandmap Test"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "initial"]);
    fs::write(
        root.join("src/lib.rs"),
        "const OFFSET: i32 = 1;\n\n// @anchor calculator.add\n// @strand arithmetic role=implementation intent=stable\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "insert prelude"]);
    fs::write(
        root.join("src/lib.rs"),
        "const OFFSET: i32 = 1;\n\n// @anchor calculator.add\n// @strand arithmetic role=implementation intent=stable\npub fn add(a: i32, b: i32) -> i32 {\n    a.saturating_add(b)\n}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "affected",
            "--no-untracked",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(packet["strands"][0]["id"], "arithmetic");
    assert_eq!(
        packet["strands"][0]["anchors"][0]["anchor"]["location"]["line_start"],
        5
    );
    assert_eq!(
        packet["strands"][0]["anchors"][0]["anchor"]["location"]["line_end"],
        7
    );
}

#[test]
fn annotation_metadata_changes_match_outside_the_exact_node_span() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add\n// @strand arithmetic role=implementation intent=stable\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();
    run_git(root, &["init"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Strandmap Test"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "initial"]);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add kind=function\n// @strand arithmetic role=producer intent=stable\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "affected",
            "--no-untracked",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
    let selected = packet["strands"][0]["anchors"]
        .as_array()
        .unwrap()
        .iter()
        .find(|anchor| anchor["role"] == "producer")
        .unwrap();
    assert_eq!(packet["strands"][0]["id"], "arithmetic");
    assert_eq!(selected["role"], "producer");
    assert_eq!(selected["direct"], true);
    assert_eq!(selected["anchor"]["location"]["line_start"], 3);
    assert_eq!(selected["anchor"]["location"]["line_end"], 5);
}

#[test]
fn one_context_command_searches_expands_and_deduplicates_source() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/auth.rs"),
        "// @anchor auth.entry\n// @strand session-contract role=producer intent=shared-token\n// @strand audit-contract role=issuer intent=audit-token\npub fn mint_session_token() {\n    persist_token();\n}\n\n// @anchor auth.validate\n// @strand session-contract role=consumer intent=shared-token\npub fn validate_session_token() {\n    load_token();\n}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--search",
            "mint_session_token",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("# Strandmap context"));
    assert!(text.contains("### session-contract [connected]"));
    assert!(text.contains("### audit-contract [connected]"));
    assert!(text.contains("src/auth.rs#L4-L6"));
    assert!(text.contains("src/auth.rs#L10-L12"));
    assert!(text.contains("pub fn validate_session_token()"));
    assert_eq!(text.matches("pub fn mint_session_token()").count(), 1);
}

#[test]
fn context_reads_the_complete_source_for_file_watch_anchors() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/policy.txt"),
        "# @anchor policy.file watch=file\nfirst=true\nsecond=true\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--anchor",
            "policy.file",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("src/policy.txt#L1-L3"));
    assert!(text.contains("first=true\nsecond=true"));
}

#[test]
fn context_budget_summarizes_whole_excerpts_it_omits() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add\n// @strand arithmetic role=implementation intent=stable\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--strand",
            "arithmetic",
            "--token-budget",
            "1",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("## Source excerpts (0)"));
    assert!(text.contains("## Omitted source excerpts"));
    assert!(text.contains("Context budget omitted 1 complete source excerpts"));
    assert!(!text.contains("pub fn add"));
}

#[test]
fn bounded_context_suppresses_a_redundant_file_scope() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    let mut source = "// @anchor profile.file watch=file\n".to_string();
    for index in 0..100 {
        source.push_str(&format!("// filler {index}\n"));
    }
    source.push_str(
        "// @anchor profile.avatar\npub fn update_profile_avatar() { store_avatar(); }\n",
    );
    fs::write(root.join("src/profile.rs"), source).unwrap();
    fs::write(
        root.join(".strandmap/strands/profile.yaml"),
        "schema: 1\nid: profile\nintent: stable\nmembers:\n  - anchor: profile.file\n    role: file\n  - anchor: profile.avatar\n    role: operation\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--anchor",
            "profile.avatar",
            "--token-budget",
            "100",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("pub fn update_profile_avatar()"), "{text}");
    assert!(!text.contains("Context budget omitted"));
    assert!(!text.contains("// filler 0"));
}

#[test]
fn context_budget_keeps_a_large_strand_packet_compact() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    let mut source = String::new();
    let mut strand = "schema: 1\nid: huge\nintent: wide\nmembers:\n".to_string();
    for index in 0..200 {
        source.push_str(&format!(
            "// @anchor huge.member-{index}\npub fn member_{index}() {{}}\n\n"
        ));
        strand.push_str(&format!(
            "  - anchor: huge.member-{index}\n    role: member\n"
        ));
    }
    fs::write(root.join("src/lib.rs"), source).unwrap();
    fs::write(root.join(".strandmap/strands/huge.yaml"), strand).unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--strand",
            "huge",
            "--token-budget",
            "1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("200 anchors across 1 files summarized"));
    assert_eq!(text.matches("Context budget omitted").count(), 1);
    assert!(!text.contains("huge.member-199"));
    assert!(text.lines().count() < 35, "{text}");

    let source_bearing = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--strand",
            "huge",
        ])
        .output()
        .unwrap();
    assert!(source_bearing.status.success());
    let source_bearing = String::from_utf8(source_bearing.stdout).unwrap();
    assert!(
        source_bearing.contains("200 anchors across 1 files summarized"),
        "{source_bearing}"
    );
    assert!(
        !source_bearing.contains("- [candidate]"),
        "{source_bearing}"
    );

    let filtered = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--path",
            "src",
            "--source-extension",
            "py",
        ])
        .output()
        .unwrap();
    assert!(filtered.status.success());
    let filtered = String::from_utf8(filtered.stdout).unwrap();
    assert!(filtered.contains("196 additional anchors across 1 files summarized"));
    assert!(filtered.contains("Source filters excluded 200 anchored ranges"));
    assert!(filtered.lines().count() < 45, "{filtered}");
}

#[test]
fn context_filters_source_before_reading_and_reports_it_compactly() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor mixed.code\n// @strand mixed role=code intent=aligned\npub fn implementation() {}\n",
    )
    .unwrap();
    fs::write(
        root.join("guide.md"),
        "# @anchor mixed.docs watch=file\n# @strand mixed role=docs\nDo not emit this markdown.\n",
    )
    .unwrap();
    initialize(root);

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--strand",
            "mixed",
            "--source-extension",
            "rs",
            "--source-include",
            "src/**",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("pub fn implementation()"));
    assert!(!text.contains("Do not emit this markdown."));
    assert!(text.contains("Source filters excluded 1 anchored ranges across 1 files"));
}

#[test]
fn search_path_limits_discovery_without_hiding_graph_expansion() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/core.rs"),
        "// @anchor core.checkout-summary\n// @strand core-contract role=code intent=core\npub fn checkout_summary() {}\n",
    )
    .unwrap();
    fs::write(
        root.join("docs/note.rs"),
        "// @anchor docs.checkout-summary\n// @strand docs-contract role=docs intent=docs\npub fn checkout_summary_notes() {}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "context",
            "--search",
            "checkout summary",
            "--search-path",
            "src",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(packet["strands"].as_array().unwrap().len(), 1);
    assert_eq!(packet["strands"][0]["id"], "core-contract");
}

#[test]
fn search_ranks_source_text_and_does_not_rank_annotation_metadata() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/metadata.rs"),
        "// @anchor metadata.only\n// @strand metadata-contract role=profile-avatar intent=metadata-only\npub fn unrelated() {}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/source.rs"),
        "// @anchor source.hit\n// @strand source-contract role=implementation intent=source-backed\npub fn render_profile_avatar() {\n    resize_avatar();\n}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--search",
            "profile avatar",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("pub fn render_profile_avatar()"), "{text}");
    assert!(text.contains("source-contract"), "{text}");
    assert!(!text.contains("metadata-contract"), "{text}");
    assert!(!text.contains("pub fn unrelated()"), "{text}");
}

#[test]
fn search_emits_the_matching_function_not_its_file_watch_scope() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/policy.rs"),
        "// @anchor profile.file watch=file\n// @strand profile role=file intent=stable\nconst LARGE_UNRELATED_PREAMBLE: &str = \"do not emit this\";\n\n// @anchor profile.avatar\n// @strand profile role=operation\npub fn update_profile_avatar() {\n    store_avatar();\n}\n\npub fn unrelated_tail() {}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--search",
            "update profile avatar",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("pub fn update_profile_avatar()"), "{text}");
    assert!(!text.contains("LARGE_UNRELATED_PREAMBLE"), "{text}");
    assert!(!text.contains("pub fn unrelated_tail()"), "{text}");
}

#[test]
fn rust_test_sections_are_omitted_by_default_and_configurable() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor library.file watch=file\npub fn production_path() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn hidden_test_sentinel() {}\n}\n\n#[test]\nfn first_standalone_test() {}\n\n#[test]\nfn second_standalone_test() {}\n",
    )
    .unwrap();

    let default = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--anchor",
            "library.file",
        ])
        .output()
        .unwrap();
    assert!(default.status.success());
    let text = String::from_utf8(default.stdout).unwrap();
    assert!(text.contains("pub fn production_path()"), "{text}");
    assert!(!text.contains("hidden_test_sentinel"), "{text}");
    assert!(!text.contains("#[test]"), "{text}");
    assert!(!text.contains("first_standalone_test"), "{text}");
    assert!(!text.contains("second_standalone_test"), "{text}");
    assert!(!text.contains("```rust\n\n```"), "{text}");
    assert!(!text.contains("Used by:"), "{text}");
    assert!(
        text.contains("Rust test sections are omitted by default"),
        "{text}"
    );
    assert!(text.contains("Omitted 3 Rust test sections"), "{text}");

    fs::write(
        root.join(".strandmap/config.yaml"),
        "version: 1\ncontext:\n  include_rust_tests: true\n",
    )
    .unwrap();
    let configured = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--anchor",
            "library.file",
        ])
        .output()
        .unwrap();
    assert!(configured.status.success());
    let text = String::from_utf8(configured.stdout).unwrap();
    assert!(text.contains("hidden_test_sentinel"), "{text}");
    assert!(
        !text.contains("Rust test sections are omitted by default"),
        "{text}"
    );
}

#[test]
fn rust_test_only_search_requires_the_include_option() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/lib.rs"),
        "pub fn production_path() {}\n\n#[cfg(test)]\nmod tests {\n    #[test]\n    fn dashboard_widget_test_sentinel() {}\n}\n",
    )
    .unwrap();

    let default = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--search",
            "dashboard widget sentinel",
        ])
        .output()
        .unwrap();
    assert!(default.status.success());
    let text = String::from_utf8(default.stdout).unwrap();
    assert!(!text.contains("dashboard_widget_test_sentinel"), "{text}");

    let included = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--search",
            "dashboard widget sentinel",
            "--include-tests",
        ])
        .output()
        .unwrap();
    assert!(included.status.success());
    let text = String::from_utf8(included.stdout).unwrap();
    assert!(text.contains("dashboard_widget_test_sentinel"), "{text}");
}

#[test]
fn search_returns_an_untagged_source_unit_in_one_command() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/plain.rs"),
        "pub fn unrelated() {}\n\npub fn recalculate_cart_total() {\n    sum_line_items();\n}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--search",
            "recalculate cart total",
            "--require-match",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("No strands selected."), "{text}");
    assert!(text.contains("pub fn recalculate_cart_total()"), "{text}");
    assert!(!text.contains("pub fn unrelated()"), "{text}");
}

#[test]
fn focused_source_policy_emits_only_exact_paths() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/aaa.rs"),
        "// @anchor search.hit\npub fn unrelated_signal() {}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/zzz.rs"),
        "// @anchor exact.path\npub fn requested_source() {}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--search",
            "unrelated_signal",
            "--path",
            "src/zzz.rs",
            "--source",
            "focused",
            "--token-budget",
            "60",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("src/zzz.rs#L2-L2 [focused]"), "{text}");
    assert!(text.contains("pub fn requested_source()"));
    assert!(!text.contains("pub fn unrelated_signal()"));
}

#[test]
fn search_matches_and_exact_paths_share_the_bounded_packet() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/search.rs"),
        "// @anchor search.small\npub fn global_search_signal() {}\n",
    )
    .unwrap();
    fs::write(
        root.join("src/first.rs"),
        "// @anchor exact.first\npub fn first_requested() {}\n",
    )
    .unwrap();
    let mut large = "// @anchor exact.second watch=file\n".to_string();
    for index in 0..100 {
        large.push_str(&format!("// large requested context {index}\n"));
    }
    fs::write(root.join("src/second.rs"), large).unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--search",
            "global_search_signal",
            "--path",
            "src/first.rs",
            "--path",
            "src/second.rs",
            "--token-budget",
            "100",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("pub fn first_requested()"), "{text}");
    assert!(text.contains("pub fn global_search_signal()"));
    assert!(!text.contains("large requested context 0"));
    assert!(text.contains("Context budget omitted"));
}

#[test]
fn structured_pre_edit_context_needs_no_git_diff() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add\n// @strand arithmetic role=implementation intent=stable\npub fn add() {}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "context",
            "--strand",
            "arithmetic",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(packet["changes"]["files"].as_array().unwrap().len(), 0);
    assert_eq!(packet["strands"][0]["id"], "arithmetic");
    assert_eq!(
        packet["strands"][0]["anchors"][0]["anchor"]["location"]["line_start"],
        3
    );
}

#[test]
fn context_unions_search_and_diff_before_rendering_source_once() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add\n// @strand arithmetic role=implementation intent=stable\npub fn add(a: i32, b: i32) -> i32 {\n    a + b\n}\n",
    )
    .unwrap();
    run_git(root, &["init"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Strandmap Test"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "initial"]);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add\n// @strand arithmetic role=implementation intent=stable\npub fn add(a: i32, b: i32) -> i32 {\n    a.saturating_add(b)\n}\n",
    )
    .unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "context",
            "--worktree",
            "--search",
            "saturating_add",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains("worktree"));
    assert!(text.contains("search \"saturating_add\""));
    assert_eq!(text.matches("a.saturating_add(b)").count(), 1);
}

#[test]
fn migration_removes_static_ranges_and_query_nodes_have_ids() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/lib.rs"),
        "// @anchor calculator.add lines=40-80\n// @strand arithmetic role=implementation intent=stable\npub fn add(a: i32, b: i32) -> i32 { a + b }\n",
    )
    .unwrap();

    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "migrate",
            "dynamic-locations",
        ])
        .assert()
        .success();
    let source = fs::read_to_string(root.join("src/lib.rs")).unwrap();
    assert!(!source.contains("lines="));
    strandmap()
        .args(["--root", root.to_str().unwrap(), "check", "--strict"])
        .assert()
        .success();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "query",
            "--anchor",
            "calculator.add",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let query: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(query["anchors"][0]["id"], "calculator.add");
    assert_eq!(query["strands"][0]["id"], "arithmetic");
}

#[test]
fn review_detects_file_drift() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::write(
        root.join("contract.txt"),
        "# @anchor contract.file\n# @strand contract intent=\"Contract stays aligned\"\nvalue=1\n",
    )
    .unwrap();
    initialize(root);
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "review",
            "start",
            "--file",
            "contract.txt",
            "--id",
            "drift",
        ])
        .assert()
        .success();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "review",
            "record",
            "drift",
            "contract.file",
            "reviewed",
        ])
        .assert()
        .success();
    fs::write(
        root.join("contract.txt"),
        "# @anchor contract.file\n# @strand contract intent=\"Contract stays aligned\"\nvalue=2\n",
    )
    .unwrap();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "review",
            "complete",
            "drift",
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains("reviewed files changed"));
}

#[test]
fn deleted_annotation_only_strands_remain_affected() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::create_dir_all(root.join("src")).unwrap();
    initialize(root);
    fs::write(
        root.join("src/legacy.rs"),
        "// @anchor legacy.entry\n// @strand legacy-contract role=implementation intent=\"Legacy behavior remains accounted for\"\nfn legacy() {}\n",
    )
    .unwrap();
    run_git(root, &["init"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Strandmap Test"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "initial"]);
    fs::remove_file(root.join("src/legacy.rs")).unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "affected",
            "--no-untracked",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(packet["strands"][0]["id"], "legacy-contract");
    assert_eq!(packet["strands"][0]["anchors"][0]["id"], "legacy.entry");
}

#[test]
fn deleted_sidecar_strands_remain_affected() {
    let temporary = TempDir::new().unwrap();
    let root = temporary.path();
    fs::write(root.join("contract.txt"), "value=1\n").unwrap();
    initialize(root);
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "strand",
            "add",
            "sidecar-contract",
            "--intent",
            "Deleted metadata remains visible",
        ])
        .assert()
        .success();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "anchor",
            "add",
            "contract.file",
            "--path",
            "contract.txt",
        ])
        .assert()
        .success();
    strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "member",
            "add",
            "sidecar-contract",
            "contract.file",
        ])
        .assert()
        .success();
    run_git(root, &["init"]);
    run_git(root, &["config", "user.email", "test@example.com"]);
    run_git(root, &["config", "user.name", "Strandmap Test"]);
    run_git(root, &["add", "."]);
    run_git(root, &["commit", "-m", "initial"]);
    fs::remove_file(root.join(".strandmap/strands/sidecar-contract.yaml")).unwrap();

    let output = strandmap()
        .args([
            "--root",
            root.to_str().unwrap(),
            "--format",
            "json",
            "affected",
            "--no-untracked",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let packet: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(packet["strands"][0]["id"], "sidecar-contract");
    assert_eq!(packet["strands"][0]["anchors"][0]["id"], "contract.file");
}

fn run_git(root: &Path, args: &[&str]) {
    let output = Command::new("git")
        .current_dir(root)
        .args(args)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "git {:?}: {}",
        args,
        String::from_utf8_lossy(&output.stderr)
    );
}
