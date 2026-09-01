//! Skill and hook manifests embedded at compile time.
//!
//! The client-aware `init` subcommand installs the same namespaced skills for
//! Claude or Codex. Claude-specific hooks remain scoped to Claude's settings.

/// A skill and every file bundled below its installation directory.
pub struct SkillManifest {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

/// A hook-bound skill: triggers before/after specific MCP tool calls.
/// Installed as a hook entry in `~/.claude/settings.json` that runs
/// `konnect.exe hook <name>` to emit Claude's structured hook JSON.
pub struct HookSkillManifest {
    pub name: &'static str,
    pub content: &'static str,
    pub board_access: konnect_core::tools::BoardAccess,
    pub event: &'static str, // "PreToolUse" or "PostToolUse"
}

pub const SKILLS: &[SkillManifest] = &[
    SkillManifest {
        name: "konnect-kicad-layout-review",
        files: &[
            (
                "SKILL.md",
                include_str!("../assets/skills/konnect-kicad-layout-review/SKILL.md"),
            ),
            (
                "agents/openai.yaml",
                include_str!("../assets/skills/konnect-kicad-layout-review/agents/openai.yaml"),
            ),
            (
                "references/general-and-two-layer.md",
                include_str!(
                    "../assets/skills/konnect-kicad-layout-review/references/general-and-two-layer.md"
                ),
            ),
            (
                "references/buck-converter.md",
                include_str!(
                    "../assets/skills/konnect-kicad-layout-review/references/buck-converter.md"
                ),
            ),
        ],
    },
    SkillManifest {
        name: "konnect-kicad-schematic",
        files: &[
            (
                "SKILL.md",
                include_str!("../assets/skills/konnect-kicad-schematic/SKILL.md"),
            ),
            (
                "agents/openai.yaml",
                include_str!("../assets/skills/konnect-kicad-schematic/agents/openai.yaml"),
            ),
        ],
    },
    SkillManifest {
        name: "konnect-kicad-package-audit",
        files: &[
            (
                "SKILL.md",
                include_str!("../assets/skills/konnect-kicad-package-audit/SKILL.md"),
            ),
            (
                "agents/openai.yaml",
                include_str!("../assets/skills/konnect-kicad-package-audit/agents/openai.yaml"),
            ),
            (
                "references/configuration.md",
                include_str!(
                    "../assets/skills/konnect-kicad-package-audit/references/configuration.md"
                ),
            ),
            (
                "scripts/generate_package_audit.py",
                include_str!(
                    "../assets/skills/konnect-kicad-package-audit/scripts/generate_package_audit.py"
                ),
            ),
            (
                "scripts/review_markdown.py",
                include_str!(
                    "../assets/skills/konnect-kicad-package-audit/scripts/review_markdown.py"
                ),
            ),
        ],
    },
    SkillManifest {
        name: "konnect-kicad-pcb-layout",
        files: &[
            (
                "SKILL.md",
                include_str!("../assets/skills/konnect-kicad-pcb-layout/SKILL.md"),
            ),
            (
                "agents/openai.yaml",
                include_str!("../assets/skills/konnect-kicad-pcb-layout/agents/openai.yaml"),
            ),
            (
                "references/design-document-contract.md",
                include_str!(
                    "../assets/skills/konnect-kicad-pcb-layout/references/design-document-contract.md"
                ),
            ),
            (
                "references/pcb-completion.md",
                include_str!(
                    "../assets/skills/konnect-kicad-pcb-layout/references/pcb-completion.md"
                ),
            ),
            (
                "references/layout-rules.md",
                include_str!(
                    "../assets/skills/konnect-kicad-pcb-layout/references/layout-rules.md"
                ),
            ),
        ],
    },
];

pub const HOOK_SKILLS: &[HookSkillManifest] = &[
    HookSkillManifest {
        name: "pre-pcb-ipc",
        content: "This operation is live-IPC-only. KiCad must be running with the exact requested .kicad_pcb board open. If IPC or board identity fails, ask the user to open that board and retry once; do not bypass the server-side guard or edit the board file behind KiCad.",
        board_access: konnect_core::tools::BoardAccess::LiveOnly,
        event: "PreToolUse",
    },
    HookSkillManifest {
        name: "pre-pcb-fallback",
        content: "This operation prefers live KiCad IPC but has a guarded closed-board file fallback. Let the tool select the safe path. Never force a file edit while KiCad holds this board open, and treat the server-side board identity and revision checks as authoritative.",
        board_access: konnect_core::tools::BoardAccess::LivePreferredWithFallback,
        event: "PreToolUse",
    },
    HookSkillManifest {
        name: "pre-pcb-closed",
        content: "This operation requires the requested board to be closed in KiCad because it edits the saved file directly. Ask the user to close that board before applying the change; do not bypass the server-side open-board refusal.",
        board_access: konnect_core::tools::BoardAccess::ClosedBoardOnly,
        event: "PreToolUse",
    },
    HookSkillManifest {
        name: "pre-pcb-conditional",
        content: "This tool has different board-state requirements for planning and applying. Dry-run or planning is non-mutating; before apply, follow the tool description and returned plan exactly. Do not reuse a stale plan revision or bypass the server-side board-state guard.",
        board_access: konnect_core::tools::BoardAccess::ApplyModeDependent,
        event: "PreToolUse",
    },
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::path::{Component, Path};

    #[test]
    fn namespaced_skill_manifests_are_complete_and_safe() {
        let expected = BTreeMap::from([
            (
                "konnect-kicad-layout-review",
                BTreeSet::from([
                    "SKILL.md",
                    "agents/openai.yaml",
                    "references/buck-converter.md",
                    "references/general-and-two-layer.md",
                ]),
            ),
            (
                "konnect-kicad-schematic",
                BTreeSet::from(["SKILL.md", "agents/openai.yaml"]),
            ),
            (
                "konnect-kicad-package-audit",
                BTreeSet::from([
                    "SKILL.md",
                    "agents/openai.yaml",
                    "references/configuration.md",
                    "scripts/generate_package_audit.py",
                    "scripts/review_markdown.py",
                ]),
            ),
            (
                "konnect-kicad-pcb-layout",
                BTreeSet::from([
                    "SKILL.md",
                    "agents/openai.yaml",
                    "references/design-document-contract.md",
                    "references/layout-rules.md",
                    "references/pcb-completion.md",
                ]),
            ),
        ]);

        assert_eq!(SKILLS.len(), expected.len());
        let mut names = BTreeSet::new();
        for skill in SKILLS {
            assert!(names.insert(skill.name), "duplicate skill: {}", skill.name);
            let paths = skill
                .files
                .iter()
                .map(|(path, _)| *path)
                .collect::<BTreeSet<_>>();
            assert_eq!(
                paths.len(),
                skill.files.len(),
                "duplicate file in {}",
                skill.name
            );
            assert_eq!(
                Some(&paths),
                expected.get(skill.name),
                "files for {}",
                skill.name
            );

            for path in &paths {
                let path = Path::new(path);
                assert!(!path.is_absolute(), "absolute path in {}", skill.name);
                assert!(
                    path.components()
                        .all(|component| matches!(component, Component::Normal(_))),
                    "unsafe path in {}: {}",
                    skill.name,
                    path.display()
                );
            }

            let skill_md = skill
                .files
                .iter()
                .find_map(|(path, content)| (*path == "SKILL.md").then_some(*content))
                .expect("SKILL.md must be embedded");
            assert!(
                skill_md.contains(&format!("name: {}", skill.name)),
                "frontmatter name for {}",
                skill.name
            );
        }
        assert_eq!(names, expected.keys().copied().collect());
    }
}
