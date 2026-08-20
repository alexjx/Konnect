//! Tool trait definitions, ToolContext, and all toolset modules.

pub mod cli;
pub mod config;
pub mod design_review;
pub mod integration;
pub mod library;
pub mod manufacturing;
pub mod pcb_access;
pub mod pcb_board;
pub mod pcb_components;
pub mod pcb_export;
pub mod pcb_routing;
pub mod project;
pub mod sch_analysis;
pub mod sch_batch;
pub mod sch_bridge;
pub mod sch_components;
pub mod sch_export;
pub mod sch_hierarchy;
pub mod sch_wiring;
pub mod schematic_builder;
pub mod svg_import;
pub mod templates;
pub mod verification;
pub mod workflow;

use crate::mcp::protocol::{CallToolResult, McpToolDescription};
use crate::router::ToolRouter;
use serde_json::Value;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

// ─── Tool Handler Type ────────────────────────────────────────────────────────

pub type ToolHandlerFn = Arc<
    dyn Fn(
            &Value,
            Arc<ToolContext>,
        ) -> Pin<Box<dyn Future<Output = anyhow::Result<CallToolResult>> + Send>>
        + Send
        + Sync,
>;

// ─── ToolDef ─────────────────────────────────────────────────────────────────

/// A single tool definition: schema + async handler.
#[derive(Clone)]
pub struct ToolDef {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub handler: ToolHandlerFn,
}

impl ToolDef {
    pub fn to_mcp_description(&self) -> McpToolDescription {
        McpToolDescription {
            name: self.name.to_string(),
            description: self.description.to_string(),
            input_schema: self.input_schema.clone(),
        }
    }
}

// Implement Debug manually because handler is not Debug
impl std::fmt::Debug for ToolDef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolDef")
            .field("name", &self.name)
            .field("description", &self.description)
            .finish()
    }
}

// ─── ToolContext ──────────────────────────────────────────────────────────────

/// Shared context passed to every tool handler.
/// Contains config, the tool router, lazily-initialized KiCAD clients, and the
/// per-call observer (used by `get_recent_calls` / `server_stats` meta-tools).
pub struct ToolContext {
    pub config: ServerConfig,
    pub router: Arc<ToolRouter>,
    pub observer: crate::observability::CallObserver,
    /// Shared KiCad session state (instance token + serialized request gate).
    pub ipc: konnect_ipc::KiCadIpcClient,
    /// In-memory TTL cache for repeated JLCPCB parts-database queries.
    pub jlcpcb_cache: QueryCache,
}

impl ToolContext {
    /// Construct a context with an in-memory-only observer (no JSONL). Used by
    /// tests and by callers that don't need persistent call logs.
    pub fn new(config: ServerConfig, router: Arc<ToolRouter>) -> Self {
        let ipc = konnect_ipc::KiCadIpcClient::new(config.ipc_address.clone());
        ToolContext {
            config,
            router,
            observer: crate::observability::CallObserver::new(None),
            ipc,
            jlcpcb_cache: QueryCache::default(),
        }
    }

    /// Construct a context with a specific observer — wired in by `McpHandler`
    /// so the JSONL log and in-memory ring are shared across all tool calls.
    pub fn new_with_observer(
        config: ServerConfig,
        router: Arc<ToolRouter>,
        observer: crate::observability::CallObserver,
    ) -> Self {
        let ipc = konnect_ipc::KiCadIpcClient::new(config.ipc_address.clone());
        ToolContext {
            config,
            router,
            observer,
            ipc,
            jlcpcb_cache: QueryCache::default(),
        }
    }
}

// ─── QueryCache ───────────────────────────────────────────────────────────────

/// A small in-memory, TTL-based cache for repeated read-only query results
/// (JSON values keyed by a caller-constructed string). One instance lives on
/// `ToolContext` for the life of the server, shared across all tool calls.
pub struct QueryCache {
    ttl: std::time::Duration,
    entries: std::sync::Mutex<std::collections::HashMap<String, (Value, std::time::Instant)>>,
}

impl QueryCache {
    pub fn new(ttl: std::time::Duration) -> Self {
        QueryCache {
            ttl,
            entries: std::sync::Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Returns a cached value for `key` if present and not yet expired.
    pub fn get(&self, key: &str) -> Option<Value> {
        let entries = self.entries.lock().unwrap();
        entries.get(key).and_then(|(value, inserted_at)| {
            if inserted_at.elapsed() < self.ttl {
                Some(value.clone())
            } else {
                None
            }
        })
    }

    /// Stores `value` under `key`, overwriting any existing (possibly expired) entry.
    pub fn put(&self, key: String, value: Value) {
        let mut entries = self.entries.lock().unwrap();
        entries.insert(key, (value, std::time::Instant::now()));
    }
}

impl Default for QueryCache {
    /// 5-minute TTL — long enough to skip redundant re-queries within a single
    /// design session, short enough that a `download_jlcpcb_database` refresh
    /// is reflected without needing an explicit cache-invalidation hook.
    fn default() -> Self {
        QueryCache::new(std::time::Duration::from_secs(300))
    }
}

// ─── ServerConfig ─────────────────────────────────────────────────────────────

/// Subset of the server configuration relevant to tool execution.
/// This is the config that flows from `konnect::Config` into the core crate.
#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub kicad_cli: String,
    pub kicad_binary: String,
    pub ipc_address: String,
    pub project_dir: Option<std::path::PathBuf>,
    pub jlcpcb_db_path: Option<std::path::PathBuf>,
}

#[cfg(test)]
mod query_cache_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn miss_on_unknown_key() {
        let cache = QueryCache::new(std::time::Duration::from_secs(60));
        assert!(cache.get("nope").is_none());
    }

    #[test]
    fn put_then_get_roundtrips() {
        let cache = QueryCache::new(std::time::Duration::from_secs(60));
        cache.put("key".to_string(), json!({ "count": 3 }));
        assert_eq!(cache.get("key"), Some(json!({ "count": 3 })));
    }

    #[test]
    fn entry_expires_after_ttl() {
        let cache = QueryCache::new(std::time::Duration::from_millis(10));
        cache.put("key".to_string(), json!("value"));
        assert_eq!(cache.get("key"), Some(json!("value")));
        std::thread::sleep(std::time::Duration::from_millis(20));
        assert!(cache.get("key").is_none());
    }

    #[test]
    fn put_overwrites_existing_entry() {
        let cache = QueryCache::new(std::time::Duration::from_secs(60));
        cache.put("key".to_string(), json!("first"));
        cache.put("key".to_string(), json!("second"));
        assert_eq!(cache.get("key"), Some(json!("second")));
    }
}

// ─── Helper macro for defining tools ─────────────────────────────────────────

/// Shorthand for building a ToolDef with a typed async handler function.
///
/// Usage:
/// ```rust,ignore
/// tool!(
///     "tool_name",
///     "Description of what it does.",
///     json_schema,        // serde_json::Value
///     |args, ctx| async move {
///         // handler body
///         Ok(CallToolResult::text("done"))
///     }
/// )
/// ```
#[macro_export]
macro_rules! tool {
    ($name:expr, $desc:expr, $schema:expr, $handler:expr) => {{
        let h: $crate::tools::ToolHandlerFn = std::sync::Arc::new(move |args, ctx| {
            let args = args.clone();
            let ctx = ctx.clone();
            Box::pin(async move { ($handler)(&args, &*ctx).await })
        });
        $crate::tools::ToolDef {
            name: $name,
            description: $desc,
            input_schema: $schema,
            handler: h,
        }
    }};
}

// ─── Argument helpers ─────────────────────────────────────────────────────────

/// Build a structured `InvalidArgument` CallToolResult. Used by the
/// `require_*` helpers so every handler that uses them emits structured
/// errors the client / observer can match on — no per-handler change needed.
fn invalid_arg(field: &str, reason: &str) -> CallToolResult {
    CallToolResult::error_kind(
        crate::mcp::error::ToolErrorKind::InvalidArgument {
            field: field.to_string(),
            reason: reason.to_string(),
        },
        format!("Argument '{}' is invalid: {}", field, reason),
    )
}

/// Extract a required string argument, returning a structured
/// `InvalidArgument` error result if missing or not a string.
pub fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str, CallToolResult> {
    args[key]
        .as_str()
        .ok_or_else(|| invalid_arg(key, "missing or not a string"))
}

/// Extract an optional string argument.
pub fn opt_str<'a>(args: &'a Value, key: &str) -> Option<&'a str> {
    args[key].as_str()
}

/// Extract a required f64 argument. Returns a structured `InvalidArgument`
/// error result if missing or not a number.
pub fn require_f64(args: &Value, key: &str) -> Result<f64, CallToolResult> {
    args[key]
        .as_f64()
        .ok_or_else(|| invalid_arg(key, "missing or not a number"))
}

/// Extract an optional f64.
pub fn opt_f64(args: &Value, key: &str) -> Option<f64> {
    args[key].as_f64()
}

/// Extract a required path string and return it as a PathBuf, using
/// `anyhow::Error`. Use this variant with `?` inside handlers that return
/// `anyhow::Result`. The surrounding dispatch will stringify the error and
/// surface it as `ToolErrorKind::HandlerError`.
pub fn get_path(args: &Value, key: &str) -> anyhow::Result<std::path::PathBuf> {
    let s = args[key]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Missing required argument: '{}'", key))?;
    Ok(std::path::PathBuf::from(s))
}

#[cfg(test)]
mod arg_helper_tests {
    use super::*;
    use crate::mcp::error::extract_error_kind;
    use serde_json::json;

    #[test]
    fn require_str_missing_produces_structured_invalid_argument() {
        let args = json!({});
        let err = require_str(&args, "path").expect_err("should fail");
        assert!(err.is_error);
        assert_eq!(
            extract_error_kind(&err).as_deref(),
            Some("invalid_argument")
        );
        // The body carries the field name so clients can branch.
        let body = match &err.content[0] {
            crate::mcp::protocol::ToolContent::Text { text } => text.clone(),
            _ => panic!(),
        };
        let parsed: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(parsed["error"]["field"], "path");
    }

    #[test]
    fn require_f64_non_number_produces_structured_invalid_argument() {
        let args = json!({ "x": "not a number" });
        let err = require_f64(&args, "x").expect_err("should fail");
        assert_eq!(
            extract_error_kind(&err).as_deref(),
            Some("invalid_argument")
        );
    }

    #[test]
    fn require_str_present_returns_value() {
        let args = json!({ "name": "ok" });
        let v = require_str(&args, "name").expect("should parse");
        assert_eq!(v, "ok");
    }
}

// ─── KiCAD config directory detection ────────────────────────────────────────

/// Find the KiCAD user config directory by probing for installed version directories.
/// Checks versions in descending order: 10.0, 9.0, 8.0, then bare "kicad".
pub fn kicad_config_dir() -> std::path::PathBuf {
    let base = kicad_config_base();
    let versions = ["10.0", "9.0", "8.0"];
    for ver in &versions {
        let dir = base.join(ver);
        if dir.is_dir() {
            return dir;
        }
    }
    // Fallback: bare kicad dir or 10.0 (will be created on first use)
    base.join("10.0")
}

/// Platform-specific base directory for KiCAD configs.
fn kicad_config_base() -> std::path::PathBuf {
    #[cfg(target_os = "windows")]
    {
        let appdata = std::env::var("APPDATA").unwrap_or_default();
        std::path::PathBuf::from(appdata).join("kicad")
    }
    #[cfg(target_os = "macos")]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        std::path::PathBuf::from(home)
            .join("Library")
            .join("Preferences")
            .join("kicad")
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    {
        let home = std::env::var("HOME").unwrap_or_default();
        std::path::PathBuf::from(home).join(".config").join("kicad")
    }
}

// ─── KiCAD symbol library resolution ────────────────────────────────────────

/// Resolve a lib_id like "Device:R" to the full symbol S-expression definition.
/// KiCAD 10 stores symbols in .kicad_symdir directories, one .kicad_sym file per symbol.
/// Returns the symbol block with the lib_id prefix (e.g. "Device:R") as the symbol name.
pub fn resolve_lib_symbol(lib_id: &str) -> Option<String> {
    resolve_lib_symbol_from_dirs(lib_id, &find_kicad_symbol_dirs())
}

fn resolve_lib_symbol_from_dirs(lib_id: &str, sym_dirs: &[std::path::PathBuf]) -> Option<String> {
    let mut visiting = std::collections::HashSet::new();
    flatten_lib_symbol(lib_id, sym_dirs, &mut visiting)
}

fn flatten_lib_symbol(
    lib_id: &str,
    sym_dirs: &[std::path::PathBuf],
    visiting: &mut std::collections::HashSet<String>,
) -> Option<String> {
    if !visiting.insert(lib_id.to_string()) {
        tracing::warn!("[BETA] Cyclic library symbol inheritance at '{}'", lib_id);
        return None;
    }
    let alias = resolve_lib_symbol_raw_from_dirs(lib_id, sym_dirs)?;
    let flattened = if let Some(parent) = symbol_parent(&alias) {
        let library_name = lib_id.split_once(':')?.0;
        let parent_lib_id = if parent.contains(':') {
            parent
        } else {
            format!("{}:{}", library_name, parent)
        };
        let base = flatten_lib_symbol(&parent_lib_id, sym_dirs, visiting)?;
        flatten_alias_definition(base, &alias, &parent_lib_id, lib_id)
    } else {
        alias
    };
    visiting.remove(lib_id);
    Some(flattened)
}

fn resolve_lib_symbol_raw_from_dirs(
    lib_id: &str,
    sym_dirs: &[std::path::PathBuf],
) -> Option<String> {
    let parts: Vec<&str> = lib_id.splitn(2, ':').collect();
    if parts.len() != 2 {
        tracing::warn!(
            "[BETA] Cannot resolve lib_id '{}' — expected 'Library:Symbol' format",
            lib_id
        );
        return None;
    }
    let (library_name, symbol_name) = (parts[0], parts[1]);

    for base_dir in sym_dirs {
        // KiCAD 10: Library.kicad_symdir/SymbolName.kicad_sym
        let symdir_path = base_dir.join(format!("{}.kicad_symdir", library_name));
        let sym_file = symdir_path.join(format!("{}.kicad_sym", symbol_name));

        if sym_file.exists() {
            tracing::debug!("[BETA] Found symbol file: {}", sym_file.display());
            match std::fs::read_to_string(&sym_file) {
                Ok(content) => {
                    if let Some(sym_block) = extract_symbol_block(&content, symbol_name) {
                        let renamed = sym_block.replacen(
                            &format!("(symbol \"{}\"", symbol_name),
                            &format!("(symbol \"{}:{}\"", library_name, symbol_name),
                            1,
                        );
                        return Some(renamed);
                    }
                }
                Err(e) => tracing::warn!("[BETA] Failed to read {}: {}", sym_file.display(), e),
            }
        }

        // Fallback: KiCAD 8/9 format — Library.kicad_sym (single file)
        let legacy_path = base_dir.join(format!("{}.kicad_sym", library_name));
        if legacy_path.exists() {
            match std::fs::read_to_string(&legacy_path) {
                Ok(content) => {
                    if let Some(sym_block) = extract_symbol_block(&content, symbol_name) {
                        let renamed = sym_block.replacen(
                            &format!("(symbol \"{}\"", symbol_name),
                            &format!("(symbol \"{}:{}\"", library_name, symbol_name),
                            1,
                        );
                        return Some(renamed);
                    }
                }
                Err(e) => tracing::warn!("[BETA] Failed to read {}: {}", legacy_path.display(), e),
            }
        }
    }

    tracing::warn!(
        "[BETA] Symbol '{}' not found in any library directory",
        lib_id
    );
    None
}

fn flatten_alias_definition(
    mut base: String,
    alias: &str,
    base_lib_id: &str,
    result_lib_id: &str,
) -> String {
    let base_name = base_lib_id
        .split_once(':')
        .map_or(base_lib_id, |(_, name)| name);
    let result_name = result_lib_id
        .split_once(':')
        .map_or(result_lib_id, |(_, name)| name);

    // Rename the outer cache entry and every unqualified unit sub-symbol.
    base = base.replacen(
        &format!("(symbol \"{}\"", base_lib_id),
        &format!("(symbol \"{}\"", result_lib_id),
        1,
    );
    base = base.replace(
        &format!("(symbol \"{}_", base_name),
        &format!("(symbol \"{}_", result_name),
    );

    // Derived symbols override fields while inheriting graphics and pins.
    for property_name in [
        "Reference",
        "Value",
        "Footprint",
        "Datasheet",
        "Description",
        "ki_keywords",
        "ki_fp_filters",
    ] {
        let Some(alias_property) = extract_named_property(alias, property_name) else {
            continue;
        };
        let Some(base_property) = extract_named_property(&base, property_name) else {
            continue;
        };
        if let Some(start) = base.find(&base_property) {
            base.replace_range(start..start + base_property.len(), &alias_property);
        }
    }
    base
}

fn extract_named_property(content: &str, property_name: &str) -> Option<String> {
    let start = content.find(&format!("(property \"{}\"", property_name))?;
    let mut depth = 0i32;
    for (offset, ch) in content[start..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(content[start..=start + offset].to_string());
                }
            }
            _ => {}
        }
    }
    None
}

/// Extract a top-level (symbol "NAME" ...) block from a .kicad_sym file.
fn extract_symbol_block(content: &str, symbol_name: &str) -> Option<String> {
    let pattern = format!("(symbol \"{}\"", symbol_name);
    let start = content.find(&pattern)?;
    let mut depth = 0i32;
    let mut end = start;
    for (i, ch) in content[start..].char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    end = start + i + 1;
                    break;
                }
            }
            _ => {}
        }
    }
    if end > start {
        Some(content[start..end].to_string())
    } else {
        None
    }
}

/// Insert a symbol definition into the schematic's lib_symbols section.
/// Creates the lib_symbols section if it doesn't exist. Skips if already present.
pub fn ensure_lib_symbol_in_schematic(content: &mut String, lib_id: &str) {
    let mut visiting = std::collections::HashSet::new();
    let sym_dirs = find_kicad_symbol_dirs();
    ensure_lib_symbol_in_schematic_inner(content, lib_id, &mut visiting, &sym_dirs);
}

fn ensure_lib_symbol_in_schematic_inner(
    content: &mut String,
    lib_id: &str,
    visiting: &mut std::collections::HashSet<String>,
    sym_dirs: &[std::path::PathBuf],
) {
    // Library aliases may form an inheritance chain through `(extends ...)`.
    // Guard against malformed/cyclic libraries while recursively embedding it.
    if !visiting.insert(lib_id.to_string()) {
        tracing::warn!(
            "[BETA] Cyclic symbol inheritance detected while embedding '{}'",
            lib_id
        );
        return;
    }

    // Check if already present. Replace an inherited library alias with the
    // complete flattened cache symbol required by schematic instances.
    let lib_id_check = format!("(symbol \"{}\"", lib_id);
    if content.contains(&lib_id_check) {
        if let Some(sym_start) = content.find(&lib_id_check) {
            if let Some(sym_block) = extract_symbol_block(&content[sym_start..], lib_id) {
                if symbol_parent(&sym_block).is_some() {
                    if let Some(flattened) = resolve_lib_symbol_from_dirs(lib_id, sym_dirs) {
                        content.replace_range(sym_start..sym_start + sym_block.len(), &flattened);
                    }
                }
            }
        }
        visiting.remove(lib_id);
        return;
    }

    // Resolve the symbol from KiCAD libraries
    let sym_def = match resolve_lib_symbol_from_dirs(lib_id, sym_dirs) {
        Some(s) => s,
        None => {
            visiting.remove(lib_id);
            return;
        }
    };

    // Ensure lib_symbols section exists
    if !content.contains("(lib_symbols") {
        if let Some(insert_after) = content.find(")\n") {
            content.insert_str(insert_after + 2, "\n\t(lib_symbols\n\t)\n");
        }
    }

    // Find the closing paren of lib_symbols and insert before it
    if let Some(ls_start) = content.find("(lib_symbols") {
        let mut depth = 0i32;
        let mut ls_end = ls_start;
        for (i, ch) in content[ls_start..].char_indices() {
            match ch {
                '(' => depth += 1,
                ')' => {
                    depth -= 1;
                    if depth == 0 {
                        ls_end = ls_start + i;
                        break;
                    }
                }
                _ => {}
            }
        }
        let indented = sym_def
            .lines()
            .map(|l| {
                if l.is_empty() {
                    String::new()
                } else {
                    format!("\t\t{}", l)
                }
            })
            .collect::<Vec<_>>()
            .join("\n");
        content.insert_str(ls_end, &format!("\n{}\n\t", indented));
    }

    visiting.remove(lib_id);
}

fn symbol_parent(symbol: &str) -> Option<String> {
    let marker = "(extends \"";
    let start = symbol.find(marker)? + marker.len();
    let end = start + symbol[start..].find('"')?;
    Some(symbol[start..end].to_string())
}

/// Resolve the embedded symbol that owns an instance's pin graphics.
///
/// KiCad library aliases often contain only `(extends "Library:Parent")`.
/// Every schematic analysis/export tool must follow that chain instead of
/// treating the pin-less alias as the complete symbol.
pub(crate) fn resolve_embedded_pin_symbol<'a>(
    lib_syms: &[&'a konnect_sexp::SexpNode],
    lib_id: &str,
) -> Option<&'a konnect_sexp::SexpNode> {
    let mut current = lib_id.to_string();
    let mut visited = std::collections::HashSet::new();
    for _ in 0..32 {
        if !visited.insert(current.clone()) {
            return None;
        }
        let symbol = lib_syms
            .iter()
            .copied()
            .find(|node| node.get(1).and_then(|child| child.as_str()) == Some(current.as_str()))?;
        if !konnect_sexp::schematic::extract_lib_pins(symbol).is_empty() {
            return Some(symbol);
        }
        let parent = symbol
            .find("extends")
            .and_then(|node| node.get(1))
            .and_then(konnect_sexp::SexpNode::as_str)?;
        current = if parent.contains(':') {
            parent.to_string()
        } else {
            let library_name = current.split_once(':').map_or("", |(library, _)| library);
            format!("{}:{}", library_name, parent)
        };
    }
    None
}

#[cfg(test)]
mod symbol_embedding_tests {
    use super::*;

    #[test]
    fn embeds_flattened_library_alias() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("TestLib.kicad_sym"),
            r#"(kicad_symbol_lib
  (version 20231120)
  (generator kicad_symbol_editor)
  (symbol "Parent"
    (property "Reference" "U" (at 0 0 0) (effects (font (size 1.27 1.27))))
    (property "Value" "Parent" (at 0 1 0) (effects (font (size 1.27 1.27))))
    (symbol "Parent_0_1" (rectangle (start -1 -1) (end 1 1) (stroke (width 0) (type default)) (fill (type background))))
    (symbol "Parent_1_1" (pin input line (at -2 0 0) (length 1) (name "IN" (effects (font (size 1.27 1.27)))) (number "1" (effects (font (size 1.27 1.27))))))
  )
  (symbol "Child"
    (extends "Parent")
    (property "Reference" "U" (at 0 0 0) (effects (font (size 1.27 1.27))))
    (property "Value" "Child" (at 0 1 0) (effects (font (size 1.27 1.27))))
  )
)"#,
        )
        .unwrap();

        let mut schematic = "(kicad_sch\n  (lib_symbols\n  )\n)\n".to_string();
        let mut visiting = std::collections::HashSet::new();
        ensure_lib_symbol_in_schematic_inner(
            &mut schematic,
            "TestLib:Child",
            &mut visiting,
            &[dir.path().to_path_buf()],
        );

        assert!(schematic.contains("(symbol \"TestLib:Child\""));
        assert!(!schematic.contains("(extends"));
        assert!(schematic.contains("(number \"1\""));
        assert!(schematic.contains("(property \"Value\" \"Child\""));
        assert!(schematic.contains("(symbol \"Child_1_1\""));
        assert!(!schematic.contains("(symbol \"Parent_1_1\""));
    }

    #[test]
    fn resolves_pin_owner_through_embedded_alias() {
        let tree = konnect_sexp::parse_sexp(
            r#"(kicad_sch
  (lib_symbols
    (symbol "TestLib:Parent"
      (symbol "TestLib:Parent_1_1"
        (pin input line
          (at 0 0 0)
          (length 2.54)
          (name "IN" (effects (font (size 1.27 1.27))))
          (number "1" (effects (font (size 1.27 1.27)))))))
    (symbol "TestLib:Child" (extends "Parent"))))"#,
        )
        .unwrap();
        let lib_syms = tree.find("lib_symbols").unwrap().find_all("symbol");
        let owner = resolve_embedded_pin_symbol(&lib_syms, "TestLib:Child").unwrap();
        let pins = konnect_sexp::schematic::extract_lib_pins(owner);
        assert_eq!(pins.len(), 1);
        assert_eq!(pins[0].number, "1");
    }
}

/// Find directories where KiCAD symbol libraries are stored.
fn find_kicad_symbol_dirs() -> Vec<std::path::PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(dir) = std::env::var("KICAD10_SYMBOL_DIR") {
        let p = std::path::PathBuf::from(&dir);
        if p.is_dir() {
            dirs.push(p);
        }
    }
    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            for version in ["10.0", "9.0"] {
                let p = std::path::PathBuf::from(&local_app_data)
                    .join("Programs")
                    .join("KiCad")
                    .join(version)
                    .join("share")
                    .join("kicad")
                    .join("symbols");
                if p.is_dir() && !dirs.contains(&p) {
                    dirs.push(p);
                }
            }
        }
        let candidates = [
            r"C:\KiCad\10.0\share\kicad\symbols",
            r"C:\Program Files\KiCad\10.0\share\kicad\symbols",
            r"C:\KiCad\9.0\share\kicad\symbols",
            r"C:\Program Files\KiCad\9.0\share\kicad\symbols",
        ];
        for c in &candidates {
            let p = std::path::PathBuf::from(c);
            if p.is_dir() && !dirs.contains(&p) {
                dirs.push(p);
            }
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        let candidates = ["/usr/share/kicad/symbols", "/usr/local/share/kicad/symbols"];
        for c in &candidates {
            let p = std::path::PathBuf::from(c);
            if p.is_dir() && !dirs.contains(&p) {
                dirs.push(p);
            }
        }
    }
    dirs
}
