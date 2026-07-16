//! Library symbol resolution — loads symbol definitions from KiCAD's installed libraries.
//!
//! KiCAD 10 stores symbols in `.kicad_symdir` directories:
//! ```text
//! C:\KiCad\10.0\share\kicad\symbols\Device.kicad_symdir\R.kicad_sym
//! C:\KiCad\10.0\share\kicad\symbols\power.kicad_symdir\VCC.kicad_sym
//! ```
//!
//! This module resolves a `lib_id` like `"Device:R"` to the full symbol S-expression
//! definition, and can inject it into a Schematic's `lib_symbols` section.

use crate::sexp::{parser, SexpNode};
use crate::Schematic;
use std::collections::BTreeSet;
use std::path::PathBuf;

/// Resolve a lib_id (e.g. "Device:R") to the full symbol S-expression string.
/// The returned string is the raw content of the `(symbol "R" ...)` block,
/// with the name prefixed as `"Device:R"`.
pub fn resolve_lib_symbol(lib_id: &str) -> Option<String> {
    let parts: Vec<&str> = lib_id.splitn(2, ':').collect();
    if parts.len() != 2 {
        return None;
    }
    let (library_name, symbol_name) = (parts[0], parts[1]);

    for base_dir in find_symbol_dirs() {
        // KiCAD 10: Library.kicad_symdir/SymbolName.kicad_sym
        let symdir = base_dir.join(format!("{}.kicad_symdir", library_name));
        let sym_file = symdir.join(format!("{}.kicad_sym", symbol_name));

        if sym_file.exists() {
            if let Ok(content) = std::fs::read_to_string(&sym_file) {
                if let Some(block) = extract_symbol_block(&content, symbol_name) {
                    // Rename symbol to include library prefix
                    let mut renamed = block.replacen(
                        &format!("(symbol \"{}\"", symbol_name),
                        &format!("(symbol \"{}:{}\"", library_name, symbol_name),
                        1,
                    );
                    // Also fix (extends "ParentName") to use prefixed name
                    if let Some(ext_pos) = renamed.find("(extends \"") {
                        let after = &renamed[ext_pos + 10..];
                        if let Some(end) = after.find('"') {
                            let parent = after[..end].to_string();
                            renamed = renamed.replace(
                                &format!("(extends \"{}\")", parent),
                                &format!("(extends \"{}:{}\")", library_name, parent),
                            );
                        }
                    }
                    // Unit sub-symbols ("Name_0_1", "Name_1_1") must stay
                    // UNPREFIXED: eeschema names only the outer symbol with
                    // the library prefix and refuses to load a schematic
                    // whose units carry it ("Failed to load schematic" —
                    // verified against kicad-cli 10.0 and the KiCAD demo
                    // corpus, which embeds units without the prefix).
                    return Some(renamed);
                }
            }
        }

        // Fallback: KiCAD 8/9 format — single Library.kicad_sym file
        let legacy = base_dir.join(format!("{}.kicad_sym", library_name));
        if legacy.exists() {
            if let Ok(content) = std::fs::read_to_string(&legacy) {
                if let Some(block) = extract_symbol_block(&content, symbol_name) {
                    let mut renamed = block.replacen(
                        &format!("(symbol \"{}\"", symbol_name),
                        &format!("(symbol \"{}:{}\"", library_name, symbol_name),
                        1,
                    );
                    if let Some(ext_pos) = renamed.find("(extends \"") {
                        let after = &renamed[ext_pos + 10..];
                        if let Some(end) = after.find('"') {
                            let parent = after[..end].to_string();
                            renamed = renamed.replace(
                                &format!("(extends \"{}\")", parent),
                                &format!("(extends \"{}:{}\")", library_name, parent),
                            );
                        }
                    }
                    // Unit sub-symbols stay UNPREFIXED here too — same rule
                    // as the symdir branch above (eeschema refuses prefixed
                    // unit names; hit in CI where KiCAD ships single-file
                    // libraries and this legacy branch handles the embed).
                    return Some(renamed);
                }
            }
        }
    }
    None
}

/// Resolve a lib_id to a parsed SexpNode tree.
pub fn resolve_lib_symbol_node(lib_id: &str) -> Option<SexpNode> {
    let raw = resolve_lib_symbol(lib_id)?;
    parser::parse(&raw).ok()
}

/// Resolve the physical pin numbers that a schematic symbol instance must own.
///
/// KiCad stores an independent KIID for every pin on every placed symbol.  The
/// library definition supplies pin numbers and geometry, but those UUIDs are
/// instance data and therefore must be generated when the symbol is placed.
/// Omitting the `(pin "N" (uuid "..."))` nodes leaves null KIIDs in eeschema;
/// the file can load, but saving/autosaving after an edit can crash while KiCad
/// orders those IDs.
pub fn resolve_lib_symbol_pin_numbers(lib_id: &str) -> Vec<String> {
    fn collect_symbol(lib_id: &str, pins: &mut BTreeSet<String>, visited: &mut BTreeSet<String>) {
        if !visited.insert(lib_id.to_owned()) {
            return;
        }

        let Some(node) = resolve_lib_symbol_node(lib_id) else {
            return;
        };

        if let Some(parent) = node.get_value("extends") {
            if parent.contains(':') {
                collect_symbol(parent, pins, visited);
            }
        }

        collect_pin_numbers(&node, pins);
    }

    let mut pins = BTreeSet::new();
    let mut visited = BTreeSet::new();
    collect_symbol(lib_id, &mut pins, &mut visited);
    pins.into_iter().collect()
}

fn collect_pin_numbers(node: &SexpNode, pins: &mut BTreeSet<String>) {
    for pin in node.find_all("pin") {
        if let Some(number) = pin.get_value("number") {
            if !number.is_empty() {
                pins.insert(number.to_owned());
            }
        }
    }

    for child_symbol in node.find_all("symbol") {
        collect_pin_numbers(child_symbol, pins);
    }
}

/// Ensure a library symbol definition is present in the schematic's lib_symbols section.
/// If the symbol is already present (by name), does nothing.
/// If the lib_symbols node doesn't exist in raw_other, creates one.
/// Handles `(extends "ParentName")` — automatically embeds the parent symbol too.
pub fn ensure_lib_symbol(schematic: &mut Schematic, lib_id: &str) {
    // Check if already present
    let check_name = format!("\"{}\"", lib_id);
    let already_present = schematic.raw_other.iter().any(|node| {
        if node.tag() == Some("lib_symbols") {
            let content = format!("{:?}", node);
            content.contains(&check_name)
        } else {
            false
        }
    });
    if already_present {
        return;
    }

    // Resolve the symbol's raw text to check for (extends "ParentName")
    let sym_raw = match resolve_lib_symbol(lib_id) {
        Some(r) => r,
        None => return,
    };

    // Check for (extends "ParentName") and resolve the parent too.
    // Note: sym_raw already has prefixed names (e.g. extends "MCU_Microchip_ATmega:ATmega48PV-10A")
    // so we use the prefixed parent name directly as the lib_id for the recursive call.
    if let Some(extends_pos) = sym_raw.find("(extends \"") {
        let after = &sym_raw[extends_pos + 10..];
        if let Some(end) = after.find('"') {
            let parent_lib_id = &after[..end]; // Already has library prefix
            if parent_lib_id.contains(':') {
                ensure_lib_symbol(schematic, parent_lib_id);
            }
        }
    }

    // Now resolve and embed the symbol itself
    let sym_node = match resolve_lib_symbol_node(lib_id) {
        Some(n) => n,
        None => return,
    };

    // Find or create the lib_symbols node
    let lib_syms_idx = schematic
        .raw_other
        .iter()
        .position(|n| n.tag() == Some("lib_symbols"));

    match lib_syms_idx {
        Some(idx) => {
            // Append the symbol to the existing lib_symbols list
            if let SexpNode::List(ref mut children) = schematic.raw_other[idx] {
                children.push(sym_node);
            }
        }
        None => {
            // Create a new lib_symbols node with this symbol
            let lib_syms =
                SexpNode::List(vec![SexpNode::Atom("lib_symbols".to_string()), sym_node]);
            // Insert at the beginning of raw_other (lib_symbols should come early)
            schematic.raw_other.insert(0, lib_syms);
        }
    }
}

/// Extract a `(symbol "NAME" ...)` block from file content by balanced-paren matching.
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

/// Find directories where KiCAD symbol libraries are stored.
pub fn find_symbol_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();

    if let Ok(dir) = std::env::var("KICAD10_SYMBOL_DIR") {
        let p = PathBuf::from(&dir);
        if p.is_dir() {
            dirs.push(p);
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Ok(local_app_data) = std::env::var("LOCALAPPDATA") {
            let user_install = PathBuf::from(local_app_data)
                .join("Programs")
                .join("KiCad")
                .join("10.0")
                .join("share")
                .join("kicad")
                .join("symbols");
            if user_install.is_dir() && !dirs.contains(&user_install) {
                dirs.push(user_install);
            }
        }

        let candidates = [
            r"C:\KiCad\10.0\share\kicad\symbols",
            r"C:\Program Files\KiCad\10.0\share\kicad\symbols",
            r"C:\KiCad\9.0\share\kicad\symbols",
            r"C:\Program Files\KiCad\9.0\share\kicad\symbols",
        ];
        for c in &candidates {
            let p = PathBuf::from(c);
            if p.is_dir() && !dirs.contains(&p) {
                dirs.push(p);
            }
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let candidates = ["/usr/share/kicad/symbols", "/usr/local/share/kicad/symbols"];
        for c in &candidates {
            let p = PathBuf::from(c);
            if p.is_dir() && !dirs.contains(&p) {
                dirs.push(p);
            }
        }
    }

    dirs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_nested_pin_numbers_and_deduplicates_them() {
        let node = parser::parse(
            r#"(symbol "X"
                (symbol "X_1_1"
                    (pin input line (at 0 0 0) (length 2.54)
                        (name "A") (number "1")))
                (symbol "X_2_1"
                    (pin output line (at 0 0 0) (length 2.54)
                        (name "B") (number "2"))
                    (pin output line (at 0 0 0) (length 2.54)
                        (name "B_ALT") (number "2"))))"#,
        )
        .unwrap();

        let mut pins = BTreeSet::new();
        collect_pin_numbers(&node, &mut pins);
        assert_eq!(pins.into_iter().collect::<Vec<_>>(), vec!["1", "2"]);
    }
}
