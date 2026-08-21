//! Codex skill manifests embedded at compile time.
//!
//! The `init` subcommand installs these under `$CODEX_HOME/skills/`, falling
//! back to `~/.codex/skills/`.

pub struct SkillManifest {
    pub name: &'static str,
    pub files: &'static [(&'static str, &'static str)],
}

pub const SKILLS: &[SkillManifest] = &[
    SkillManifest {
        name: "kicad-layout-review",
        files: &[
            (
                "SKILL.md",
                include_str!("../assets/skills/kicad-layout-review/SKILL.md"),
            ),
            (
                "agents/openai.yaml",
                include_str!("../assets/skills/kicad-layout-review/agents/openai.yaml"),
            ),
            (
                "references/general-and-two-layer.md",
                include_str!(
                    "../assets/skills/kicad-layout-review/references/general-and-two-layer.md"
                ),
            ),
            (
                "references/buck-converter.md",
                include_str!(
                    "../assets/skills/kicad-layout-review/references/buck-converter.md"
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
                include_str!(
                    "../assets/skills/konnect-kicad-schematic/agents/openai.yaml"
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
                include_str!(
                    "../assets/skills/konnect-kicad-pcb-layout/agents/openai.yaml"
                ),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn layout_review_skill_is_complete_and_safely_packaged() {
        let skill = SKILLS
            .iter()
            .find(|skill| skill.name == "kicad-layout-review")
            .expect("kicad-layout-review manifest entry");
        let mut paths = HashSet::new();
        for (path, _) in skill.files {
            assert!(paths.insert(*path), "duplicate embedded path: {path}");
            assert!(!path.starts_with('/') && !path.starts_with('\\'));
            assert!(!path
                .split(|character| character == '/' || character == '\\')
                .any(|part| part == ".."));
        }
        let skill_md = skill
            .files
            .iter()
            .find_map(|(path, content)| (*path == "SKILL.md").then_some(*content))
            .expect("embedded SKILL.md");
        assert!(skill_md.contains("name: kicad-layout-review"));
        assert!(!skill_md.contains("[TODO"));
    }

    #[test]
    fn pcb_layout_skill_packages_its_layout_rules() {
        let skill = SKILLS
            .iter()
            .find(|skill| skill.name == "konnect-kicad-pcb-layout")
            .expect("konnect-kicad-pcb-layout manifest entry");
        let skill_md = skill
            .files
            .iter()
            .find_map(|(path, content)| (*path == "SKILL.md").then_some(*content))
            .expect("embedded SKILL.md");

        assert!(skill
            .files
            .iter()
            .any(|(path, _)| *path == "references/layout-rules.md"));
        assert!(skill_md.contains("references/layout-rules.md"));
    }
}
