//! The tool counts quoted in the docs must match the registry.
//!
//! CONTRIBUTING asks for `router/registry.rs`, `tool-directory.md`, DEV.md's
//! "Current Stats" and the README to move together, and notes that "those
//! three counts have drifted apart before precisely because only one of them
//! got updated". Nothing enforced it: `registry_tool_counts_match_reality`
//! only checks each toolset's `tool_count` against `tools_for()`, so a PR that
//! adds a tool and updates two of the four documents is green.
//!
//! That is not hypothetical — PRs #159 and #160 each bump `registry.rs`,
//! `tool-directory.md` and DEV.md while leaving README.md behind, and CI has
//! nothing to say about it.
//!
//! So derive the numbers from the registry and require the prose to agree.

use konnect_core::router::registry;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    // crates/konnect -> crates -> repo root
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root above crates/konnect")
        .to_path_buf()
}

fn read(rel: &str) -> String {
    let path = repo_root().join(rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("reading {rel}: {e}"))
}

#[derive(Debug, Clone, Copy)]
struct Counts {
    toolsets: usize,
    implemented_raw: usize,
    exposed_raw: usize,
    workflow: usize,
    meta: usize,
    legacy_full: usize,
    expert_full: usize,
    workflow_full: usize,
    legacy_starter: usize,
    expert_starter: usize,
}

/// Ground truth: derive each public surface from its owning definitions.
fn counts() -> Counts {
    let toolsets = registry::ALL_TOOLSETS.len();
    let exposed_raw: usize = registry::ALL_TOOLSETS.iter().map(|t| t.tool_count).sum();
    let implemented_raw = exposed_raw + registry::DISABLED_TOOLS.len();
    let workflow = konnect_core::tools::workflow::tools().len();
    let meta_defs = konnect_core::router::meta_tools::meta_tool_descriptions();
    let meta = meta_defs.len();
    let workflow_meta = meta_defs
        .iter()
        .filter(|tool| matches!(tool.name.as_str(), "get_recent_calls" | "server_stats"))
        .count();
    let starter_raw: usize = registry::STARTER_KIT
        .iter()
        .map(|name| {
            registry::tools_for(name)
                .expect("starter toolset resolves")
                .len()
        })
        .sum();
    Counts {
        toolsets,
        implemented_raw,
        exposed_raw,
        workflow,
        meta,
        legacy_full: exposed_raw + meta,
        expert_full: exposed_raw + workflow + meta,
        workflow_full: workflow + workflow_meta,
        legacy_starter: starter_raw + meta,
        expert_starter: starter_raw + workflow + meta,
    }
}

/// Every number a document is required to quote, with the exact spelling to
/// look for. Kept as whole phrases rather than bare integers so a coincidental
/// "187" elsewhere in the file cannot satisfy the check.
fn required_phrases() -> Vec<(&'static str, String)> {
    let counts = counts();
    vec![
        (
            "README.md",
            format!(
                "**{} raw implementations: {} exposed across {} on-demand toolsets, plus {} guarded workflow tools.**",
                counts.implemented_raw, counts.exposed_raw, counts.toolsets, counts.workflow
            ),
        ),
        (
            "DEV.md",
            format!(
                "**{} raw implementations, {} exposed raw tools** across {} toolsets",
                counts.implemented_raw, counts.exposed_raw, counts.toolsets
            ),
        ),
        (
            "DEV.md",
            format!(
                "Legacy full: {} tools; Expert full: {} tools; Workflow: {} tools",
                counts.legacy_full, counts.expert_full, counts.workflow_full
            ),
        ),
        (
            "tool-directory.md",
            format!(
                "**{} exposed raw tools** + **{} guarded workflow tools** + **{} meta-tools**",
                counts.exposed_raw, counts.workflow, counts.meta
            ),
        ),
    ]
}

#[test]
fn docs_quote_the_registry_tool_counts() {
    let mut wrong = Vec::new();
    for (file, phrase) in required_phrases() {
        if !read(file).contains(&phrase) {
            wrong.push(format!("{file} is missing: {phrase}"));
        }
    }
    assert!(
        wrong.is_empty(),
        "tool counts have drifted from the registry.\n{}\n\n\
         Update registry.rs, tool-directory.md, DEV.md's \"Current Stats\" and \
         the README together — see CONTRIBUTING.",
        wrong.join("\n")
    );
}

/// `tool-directory.md` lists every tool in a table, so its row count is a
/// second, independent statement of the same number. A tool added to the
/// registry without a directory entry is undocumented; one listed but not
/// registered is a phantom the LLM will try to call.
#[test]
fn tool_directory_lists_every_registered_tool() {
    let directory = read("tool-directory.md");

    let mut missing = Vec::new();
    for ts in registry::ALL_TOOLSETS {
        for def in registry::tools_for(ts.name).expect("toolset resolves") {
            if !directory.contains(&format!("`{}`", def.name)) {
                missing.push(format!("{} (in {})", def.name, ts.name));
            }
        }
    }

    assert!(
        missing.is_empty(),
        "tool-directory.md does not document {} registered tool(s):\n  {}",
        missing.len(),
        missing.join("\n  ")
    );
}

/// No file anywhere quotes a catalogue total that is not the current one.
///
/// The checks above name the four files CONTRIBUTING lists, which is why
/// `packaging/metadata.json` and `plugin/plugin.json` sat at "185 tools" while
/// the guarded documents said 200 — and those two are the ones users read, in
/// the PCM package description. `docs/TROUBLESHOOTING.md` said 189.
///
/// Sweeping instead of listing means a new document is covered the day it is
/// written rather than the day someone remembers to add it here. Only
/// three-digit counts are checked: a per-toolset count cannot reach 100 with
/// `MAX_TOOLS_PER_TOOLSET` at 20, so DEV.md's per-toolset tables are
/// unambiguously not catalogue totals and are left alone.
#[test]
fn no_file_quotes_a_stale_catalogue_total() {
    let counts = counts();
    let supported = [
        counts.implemented_raw,
        counts.exposed_raw,
        counts.legacy_full,
        counts.expert_full,
    ];

    let mut stale = Vec::new();
    for path in text_files(&repo_root()) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (lineno, line) in text.lines().enumerate() {
            for n in counts_in(line) {
                if n >= 100 && !supported.contains(&n) {
                    let rel = path
                        .strip_prefix(repo_root())
                        .unwrap_or(&path)
                        .display()
                        .to_string()
                        .replace('\\', "/");
                    stale.push(format!(
                        "{rel}:{}: says \"{n} tools\" — supported catalogue counts are \
                         {} implemented raw, {} exposed raw, {} Legacy full, and {} Expert full",
                        lineno + 1,
                        counts.implemented_raw,
                        counts.exposed_raw,
                        counts.legacy_full,
                        counts.expert_full
                    ));
                }
            }
        }
    }

    assert!(
        stale.is_empty(),
        "a document quotes a tool count the registry does not support:\n  {}",
        stale.join("\n  ")
    );
}

/// Markdown and JSON under the repo, skipping build output and vendored trees.
fn text_files(root: &Path) -> Vec<PathBuf> {
    // .claude holds agent worktrees — other checkouts whose docs answer to
    // their own commit, not this one.
    const SKIP: &[&str] = &[
        "target",
        "node_modules",
        ".git",
        ".claude",
        ".agents",
        "dist",
        "build",
    ];
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if path.is_dir() {
                if !SKIP.contains(&name.as_ref()) {
                    stack.push(path);
                }
            } else if matches!(
                path.extension().and_then(|e| e.to_str()),
                Some("md") | Some("json")
            ) {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

/// Numbers written immediately before the word "tools", ignoring any `~`.
fn counts_in(line: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for (at, _) in line.match_indices("tools") {
        let before = line[..at].trim_end();
        let digits: String = before
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect();
        if digits.is_empty() {
            continue;
        }
        // Only a bare number counts: "20250610 tools" would be a version.
        if let Ok(n) = digits.chars().rev().collect::<String>().parse::<usize>() {
            out.push(n);
        }
    }
    out
}

/// No file quotes a stale **toolset** count.
///
/// The catalogue-total sweep above only looks at three-digit numbers, so a
/// two-digit toolset count slips past it by construction. That is exactly how
/// `packaging/metadata.json` and `router/meta_tools.rs` both sat at "18
/// toolsets" through a release that shipped 19 — the number users read in the
/// PCM description, and the number the router's own module doc claims.
///
/// Counted separately from tools because the plural noun differs, and because
/// a toolset count is small enough that no digit-width heuristic can separate
/// it from an unrelated number.
#[test]
fn no_file_quotes_a_stale_toolset_count() {
    let toolsets = counts().toolsets;

    let mut stale = Vec::new();

    // The router's own doc comments make this claim too — `meta_tools.rs` said
    // "all 18 toolsets" — so sweep those sources alongside the documents.
    for path in text_files(&repo_root()).into_iter().chain(rust_sources(
        &repo_root().join("crates/konnect-core/src/router"),
    )) {
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        for (lineno, line) in text.lines().enumerate() {
            for n in counts_before(line, "toolset") {
                if n != toolsets {
                    let rel = path
                        .strip_prefix(repo_root())
                        .unwrap_or(&path)
                        .display()
                        .to_string()
                        .replace('\\', "/");
                    stale.push(format!(
                        "{rel}:{}: says \"{n} toolset(s)\" — the registry defines {toolsets}",
                        lineno + 1
                    ));
                }
            }
        }
    }
    assert!(
        stale.is_empty(),
        "a file quotes a toolset count the registry does not support:\n  {}",
        stale.join("\n  ")
    );
}

/// Every `### \`toolset\` · N tools` heading in `tool-directory.md` matches that
/// toolset's `tool_count`.
///
/// `sch_components` shipped a release reading "19 tools" over a table of 20
/// rows. Both the file's own overview (202) and the registry (20) disagreed
/// with it, and nothing noticed: the catalogue sweep ignores two-digit numbers,
/// and `tool_directory_lists_every_registered_tool` only checks that each tool
/// appears *somewhere* in the file, not that the section headings add up.
#[test]
fn tool_directory_section_headings_match_the_registry() {
    let directory = read("tool-directory.md");
    let mut wrong = Vec::new();
    let mut seen = 0usize;

    for (lineno, line) in directory.lines().enumerate() {
        let Some(rest) = line.strip_prefix("### `") else {
            continue;
        };
        let Some((name, tail)) = rest.split_once('`') else {
            continue;
        };
        let Some(meta) = registry::ALL_TOOLSETS.iter().find(|ts| ts.name == name) else {
            continue; // a heading that is not a toolset
        };
        seen += 1;
        let claimed: Option<usize> = tail
            .split_whitespace()
            .find_map(|word| word.parse::<usize>().ok());
        let exposed = registry::tools_for(meta.name)
            .expect("documented toolset resolves")
            .len();
        match claimed {
            Some(n) if n == exposed => {}
            Some(n) => wrong.push(format!(
                "tool-directory.md:{}: `{name}` heading says {n} tools, registry exposes {}",
                lineno + 1,
                exposed
            )),
            None => wrong.push(format!(
                "tool-directory.md:{}: `{name}` heading states no tool count",
                lineno + 1
            )),
        }
    }

    assert_eq!(
        seen,
        registry::ALL_TOOLSETS.len(),
        "tool-directory.md should have one section heading per toolset"
    );
    assert!(
        wrong.is_empty(),
        "tool-directory.md section headings disagree with the registry:\n  {}",
        wrong.join("\n  ")
    );
}

/// `.rs` files under `dir`, for the doc-comment sweeps.
fn rust_sources(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return files;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) == Some("rs") {
            files.push(path);
        }
    }
    files.sort();
    files
}

/// Numbers written immediately before `noun` (matching its plural too).
fn counts_before(line: &str, noun: &str) -> Vec<usize> {
    let mut out = Vec::new();
    for (at, _) in line.match_indices(noun) {
        let before = line[..at].trim_end();
        let digits: String = before
            .chars()
            .rev()
            .take_while(char::is_ascii_digit)
            .collect();
        if digits.is_empty() {
            continue;
        }
        if let Ok(n) = digits.chars().rev().collect::<String>().parse::<usize>() {
            out.push(n);
        }
    }
    out
}

/// The meta-tool count is quoted in prose too, and it moves far less often —
/// which is exactly why a change to it is easy to forget. PR #176 proposes
/// taking it from 6 to 7.
#[test]
fn docs_quote_the_meta_tool_count() {
    let meta = counts().meta;
    let dev = read("DEV.md");
    assert!(
        dev.contains(&format!("{meta} meta-tools")),
        "DEV.md must state \"{meta} meta-tools\" — the registry defines {meta}"
    );
}

#[test]
fn tool_directory_lists_workflows_but_not_disabled_raw_implementations() {
    let directory = read("tool-directory.md");

    let missing_workflows: Vec<_> = konnect_core::tools::workflow::tools()
        .into_iter()
        .filter(|def| !directory.contains(&format!("`{}`", def.name)))
        .map(|def| def.name)
        .collect();
    assert!(
        missing_workflows.is_empty(),
        "tool-directory.md is missing workflow tools: {}",
        missing_workflows.join(", ")
    );

    let advertised_disabled: Vec<_> = registry::DISABLED_TOOLS
        .iter()
        .filter(|disabled| directory.contains(&format!("`{}`", disabled.name)))
        .map(|disabled| disabled.name)
        .collect();
    assert!(
        advertised_disabled.is_empty(),
        "tool-directory.md advertises disabled raw implementations: {}",
        advertised_disabled.join(", ")
    );
}

#[test]
fn implementation_exposure_and_profile_counts_are_stable() {
    let counts = counts();
    assert_eq!(counts.implemented_raw, 228);
    assert_eq!(counts.exposed_raw, 195);
    assert_eq!(registry::DISABLED_TOOLS.len(), 33);
    assert_eq!(counts.workflow, 7);
    assert_eq!(counts.meta, 6);
    assert_eq!(counts.legacy_full, 201);
    assert_eq!(counts.expert_full, 208);
    assert_eq!(counts.workflow_full, 9);
    assert_eq!(counts.legacy_starter, 18);
    assert_eq!(counts.expert_starter, 25);
}
