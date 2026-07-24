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
                "references/layout-rules.md",
                include_str!(
                    "../assets/skills/konnect-kicad-pcb-layout/references/layout-rules.md"
                ),
            ),
            (
                "references/pcb-completion.md",
                include_str!(
                    "../assets/skills/konnect-kicad-pcb-layout/references/pcb-completion.md"
                ),
            ),
        ],
    },
];
