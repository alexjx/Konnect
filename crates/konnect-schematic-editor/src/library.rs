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

/// Resolve a symbol from a project `sym-lib-table` first, then fall back to
/// the installed KiCad libraries. This keeps project-specific symbols local
/// while allowing the schematic editor to place them safely.
pub fn resolve_lib_symbol_in_project(
    lib_id: &str,
    project_dir: Option<&std::path::Path>,
) -> Option<String> {
    let (library_name, symbol_name) = lib_id.split_once(':')?;
    if let Some(project_dir) = project_dir {
        if let Some(library_path) = project_symbol_library_path(project_dir, library_name) {
            if let Ok(content) = std::fs::read_to_string(library_path) {
                if let Some(block) = extract_symbol_block(&content, symbol_name) {
                    let mut renamed = block.replacen(
                        &format!("(symbol \"{}\"", symbol_name),
                        &format!("(symbol \"{}\"", lib_id),
                        1,
                    );
                    if let Some(ext_pos) = renamed.find("(extends \"") {
                        let after = &renamed[ext_pos + 10..];
                        if let Some(end) = after.find('"') {
                            let parent = after[..end].to_string();
                            if !parent.contains(':') {
                                renamed = renamed.replace(
                                    &format!("(extends \"{}\")", parent),
                                    &format!("(extends \"{}:{}\")", library_name, parent),
                                );
                            }
                        }
                    }
                    return Some(renamed);
                }
            }
        }
    }
    resolve_lib_symbol(lib_id)
}

/// Resolve the complete cache definition KiCad stores inside a schematic.
///
/// Library aliases may contain only `(extends "Parent")` plus overridden
/// properties. KiCad's schematic cache, however, needs a self-contained symbol
/// with the inherited graphics and pins copied into the alias definition.
pub fn resolve_flattened_lib_symbol_in_project(
    lib_id: &str,
    project_dir: Option<&std::path::Path>,
) -> Option<String> {
    let mut visiting = BTreeSet::new();
    flatten_lib_symbol_in_project(lib_id, project_dir, &mut visiting)
}

fn flatten_lib_symbol_in_project(
    lib_id: &str,
    project_dir: Option<&std::path::Path>,
    visiting: &mut BTreeSet<String>,
) -> Option<String> {
    if !visiting.insert(lib_id.to_owned()) {
        return None;
    }

    let alias = resolve_lib_symbol_in_project(lib_id, project_dir)?;
    let flattened = if let Some(parent) = symbol_parent(&alias) {
        let parent_id = if parent.contains(':') {
            parent
        } else {
            format!("{}:{}", lib_id.split_once(':')?.0, parent)
        };
        let base = flatten_lib_symbol_in_project(&parent_id, project_dir, visiting)?;
        flatten_alias_definition(base, &alias, &parent_id, lib_id)
    } else {
        alias
    };

    visiting.remove(lib_id);
    Some(flattened)
}

fn symbol_parent(symbol: &str) -> Option<String> {
    let marker = "(extends \"";
    let start = symbol.find(marker)? + marker.len();
    let end = start + symbol[start..].find('"')?;
    Some(symbol[start..end].to_string())
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

    base = base.replacen(
        &format!("(symbol \"{}\"", base_lib_id),
        &format!("(symbol \"{}\"", result_lib_id),
        1,
    );
    base = base.replace(
        &format!("(symbol \"{}_", base_name),
        &format!("(symbol \"{}_", result_name),
    );

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

fn project_symbol_library_path(
    project_dir: &std::path::Path,
    library_name: &str,
) -> Option<PathBuf> {
    let table = std::fs::read_to_string(project_dir.join("sym-lib-table")).ok()?;
    let name_token = format!("(name \"{}\")", library_name);
    let line = table.lines().find(|line| line.contains(&name_token))?;
    let uri_start = line.find("(uri \"")? + 6;
    let uri_end = line[uri_start..].find('"')? + uri_start;
    let mut uri = line[uri_start..uri_end].to_string();
    uri = uri.replace("${KIPRJMOD}", &project_dir.to_string_lossy());
    let path = PathBuf::from(uri);
    Some(if path.is_absolute() {
        path
    } else {
        project_dir.join(path)
    })
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

pub fn resolve_lib_symbol_pin_numbers_in_project(
    lib_id: &str,
    project_dir: Option<&std::path::Path>,
) -> Vec<String> {
    fn collect_symbol(
        lib_id: &str,
        project_dir: Option<&std::path::Path>,
        pins: &mut BTreeSet<String>,
        visited: &mut BTreeSet<String>,
    ) {
        if !visited.insert(lib_id.to_owned()) {
            return;
        }
        let Some(raw) = resolve_lib_symbol_in_project(lib_id, project_dir) else {
            return;
        };
        let Ok(node) = parser::parse(&raw) else {
            return;
        };
        if let Some(parent) = node.get_value("extends") {
            let parent_id = if parent.contains(':') {
                parent.to_string()
            } else {
                format!("{}:{}", lib_id.split_once(':').map_or("", |v| v.0), parent)
            };
            collect_symbol(&parent_id, project_dir, pins, visited);
        }
        collect_pin_numbers(&node, pins);
    }

    let mut pins = BTreeSet::new();
    let mut visited = BTreeSet::new();
    collect_symbol(lib_id, project_dir, &mut pins, &mut visited);
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

pub fn ensure_lib_symbol_in_project(
    schematic: &mut Schematic,
    lib_id: &str,
    project_dir: Option<&std::path::Path>,
) {
    let check_name = format!("\"{}\"", lib_id);
    let already_present = schematic.raw_other.iter().any(|node| {
        node.tag() == Some("lib_symbols") && format!("{:?}", node).contains(&check_name)
    });
    if already_present {
        let embedded_is_inherited = schematic.raw_other.iter().any(|node| {
            node.tag() == Some("lib_symbols")
                && node.find_all("symbol").iter().any(|symbol| {
                    symbol.value() == Some(lib_id) && symbol.get_value("extends").is_some()
                })
        });
        if !embedded_is_inherited {
            return;
        }
    }

    let Some(raw) = resolve_flattened_lib_symbol_in_project(lib_id, project_dir) else {
        return;
    };
    let Ok(node) = parser::parse(&raw) else {
        return;
    };

    if let Some(idx) = schematic
        .raw_other
        .iter()
        .position(|node| node.tag() == Some("lib_symbols"))
    {
        if let SexpNode::List(ref mut children) = schematic.raw_other[idx] {
            children
                .retain(|child| !(child.tag() == Some("symbol") && child.value() == Some(lib_id)));
            children.push(node);
        }
    } else {
        schematic.raw_other.insert(
            0,
            SexpNode::List(vec![SexpNode::Atom("lib_symbols".to_string()), node]),
        );
    }
}

/// Replace the embedded cache copy of a symbol with the current project or
/// installed library definition.  This is intentionally stronger than
/// `ensure_lib_symbol_in_project`: a component swap or library-symbol repair
/// must not retain an older embedded definition that makes KiCad report a
/// library mismatch.
pub fn refresh_lib_symbol_in_project(
    schematic: &mut Schematic,
    lib_id: &str,
    project_dir: Option<&std::path::Path>,
) -> bool {
    let mut visiting = BTreeSet::new();
    refresh_lib_symbol_in_project_inner(schematic, lib_id, project_dir, &mut visiting)
}

fn refresh_lib_symbol_in_project_inner(
    schematic: &mut Schematic,
    lib_id: &str,
    project_dir: Option<&std::path::Path>,
    visiting: &mut BTreeSet<String>,
) -> bool {
    if !visiting.insert(lib_id.to_owned()) {
        return false;
    }
    let Some(raw) = resolve_flattened_lib_symbol_in_project(lib_id, project_dir) else {
        return false;
    };
    let Ok(node) = parser::parse(&raw) else {
        return false;
    };

    if let Some(index) = schematic
        .raw_other
        .iter()
        .position(|item| item.tag() == Some("lib_symbols"))
    {
        if let SexpNode::List(children) = &mut schematic.raw_other[index] {
            children
                .retain(|child| !(child.tag() == Some("symbol") && child.value() == Some(lib_id)));
            children.push(node);
            return true;
        }
    }

    schematic.raw_other.insert(
        0,
        SexpNode::List(vec![SexpNode::Atom("lib_symbols".to_string()), node]),
    );
    true
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

    #[test]
    fn resolves_project_symbol_library_and_pin_numbers() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("ProjectSymbols.kicad_sym");
        std::fs::write(
            &lib,
            r#"(kicad_symbol_lib (version 20240108)
              (symbol "TEST"
                (symbol "TEST_1_1"
                  (pin input line (at -5.08 0 0) (length 2.54)
                    (name "IN") (number "1")))))"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("sym-lib-table"),
            format!(
                "(sym_lib_table\n  (lib (name \"ProjectSymbols\") (type \"KiCad\") (uri \"{}\") (options \"\") (descr \"\"))\n)\n",
                lib.to_string_lossy()
            ),
        )
        .unwrap();

        let resolved = resolve_lib_symbol_in_project("ProjectSymbols:TEST", Some(dir.path()))
            .expect("project symbol should resolve");
        assert!(resolved.contains("(symbol \"ProjectSymbols:TEST\""));
        assert_eq!(
            resolve_lib_symbol_pin_numbers_in_project("ProjectSymbols:TEST", Some(dir.path())),
            vec!["1"]
        );
    }

    #[test]
    fn refreshes_derived_symbol_as_a_flattened_cache_entry() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("ProjectSymbols.kicad_sym");
        std::fs::write(
            &lib,
            r#"(kicad_symbol_lib (version 20240108)
              (symbol "Parent"
                (symbol "Parent_1_1"
                  (pin input line (at -5.08 0 0) (length 2.54)
                    (name "IN") (number "1"))))
              (symbol "Child" (extends "Parent")))"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("sym-lib-table"),
            format!(
                "(sym_lib_table\n  (lib (name \"ProjectSymbols\") (type \"KiCad\") (uri \"{}\") (options \"\") (descr \"\"))\n)\n",
                lib.to_string_lossy()
            ),
        )
        .unwrap();
        let schematic_path = dir.path().join("derived.kicad_sch");
        std::fs::write(
            &schematic_path,
            r#"(kicad_sch (version 20231120) (generator eeschema)
              (uuid "00000000-0000-0000-0000-000000000001")
              (paper "A4")
              (lib_symbols))"#,
        )
        .unwrap();

        let mut schematic = Schematic::load(&schematic_path).unwrap();
        assert!(refresh_lib_symbol_in_project(
            &mut schematic,
            "ProjectSymbols:Child",
            Some(dir.path())
        ));
        schematic.overwrite().unwrap();
        let saved = std::fs::read_to_string(&schematic_path).unwrap();
        assert!(saved.contains("(symbol \"ProjectSymbols:Child\""));
        assert!(!saved.contains("(extends"));
        assert!(saved.contains("(symbol \"Child_1_1\""));
        assert!(saved.contains("(number \"1\""));
    }

    #[test]
    fn ensure_replaces_an_existing_alias_with_a_flattened_cache_entry() {
        let dir = tempfile::tempdir().unwrap();
        let lib = dir.path().join("ProjectSymbols.kicad_sym");
        std::fs::write(
            &lib,
            r#"(kicad_symbol_lib (version 20240108)
              (symbol "Parent"
                (property "Reference" "U" (at 0 0 0))
                (property "Value" "Parent" (at 0 1 0))
                (symbol "Parent_1_1"
                  (pin input line (at -5.08 0 0) (length 2.54)
                    (name "IN") (number "1"))))
              (symbol "Child"
                (extends "Parent")
                (property "Reference" "U" (at 0 0 0))
                (property "Value" "Child" (at 0 1 0))))"#,
        )
        .unwrap();
        std::fs::write(
            dir.path().join("sym-lib-table"),
            format!(
                "(sym_lib_table\n  (lib (name \"ProjectSymbols\") (type \"KiCad\") (uri \"{}\") (options \"\") (descr \"\"))\n)\n",
                lib.to_string_lossy()
            ),
        )
        .unwrap();
        let schematic_path = dir.path().join("derived.kicad_sch");
        std::fs::write(
            &schematic_path,
            r#"(kicad_sch (version 20231120) (generator eeschema)
              (uuid "00000000-0000-0000-0000-000000000001")
              (paper "A4")
              (lib_symbols
                (symbol "ProjectSymbols:Parent"
                  (symbol "Parent_1_1"
                    (pin input line (at -5.08 0 0) (length 2.54)
                      (name "IN") (number "1"))))
                (symbol "ProjectSymbols:Child"
                  (extends "ProjectSymbols:Parent"))))"#,
        )
        .unwrap();

        let mut schematic = Schematic::load(&schematic_path).unwrap();
        ensure_lib_symbol_in_project(&mut schematic, "ProjectSymbols:Child", Some(dir.path()));
        schematic.overwrite().unwrap();

        let saved = std::fs::read_to_string(&schematic_path).unwrap();
        assert!(saved.contains("(symbol \"ProjectSymbols:Child\""));
        assert!(!saved.contains("(extends"));
        assert!(saved.contains("(symbol \"Child_1_1\""));
        assert!(saved.contains("(number \"1\""));
    }
}
