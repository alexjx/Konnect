//! First-run installer for Konnect.
//!
//! Handles:
//! - `init` — install bundled Codex skills with console output
//! - `uninstall` — remove bundled Codex skills
//! - `status` — show install state with [+]/[-] markers
//! - Silent install on first MCP launch (no stdout, stderr logging only)
//! - KiCAD auto-detection on Windows

use crate::manifest::SKILLS;
use anyhow::{bail, Context, Result};
use std::fs;
use std::path::{Component, Path, PathBuf};

// ─── Public API ──────────────────────────────────────────────────────────────

/// Full install with console output. Called by `init` subcommand or double-click.
pub fn run_install() -> Result<()> {
    println!("Installing Konnect Codex skills...\n");

    let skills_dir = codex_skills_dir()?;
    fs::create_dir_all(&skills_dir)?;
    for skill in SKILLS {
        install_skill(&skills_dir, skill)?;
        println!("  [+] Skill: {}", skill.name);
    }

    // KiCAD detection
    if let Some(kicad_path) = detect_kicad() {
        println!("\n  [+] Found KiCAD at: {}", kicad_path.display());
    } else {
        println!("\n  [-] KiCAD not found in standard locations");
        println!("      Set kicad_cli path in your config file manually");
    }

    // Write marker
    let data = data_dir()?;
    fs::create_dir_all(&data)?;
    fs::write(data.join(".installed"), env!("CARGO_PKG_VERSION"))?;

    println!(
        "\nDone: {} skills installed to {}.",
        SKILLS.len(),
        skills_dir.display()
    );
    Ok(())
}

/// Silent install — no stdout output (safe for MCP pipe mode).
/// Logs to stderr via tracing.
pub fn run_install_silent() -> Result<()> {
    let skills_dir = codex_skills_dir()?;
    fs::create_dir_all(&skills_dir)?;
    for skill in SKILLS {
        install_skill(&skills_dir, skill)?;
    }

    // Marker
    let data = data_dir()?;
    fs::create_dir_all(&data)?;
    fs::write(data.join(".installed"), env!("CARGO_PKG_VERSION"))?;

    eprintln!(
        "[konnect] Silent install complete: {} Codex skills",
        SKILLS.len()
    );
    Ok(())
}

/// Remove all bundled Codex skill directories.
pub fn run_uninstall() -> Result<()> {
    println!("Uninstalling Konnect Codex skills...\n");

    let skills_dir = codex_skills_dir()?;
    for skill in SKILLS {
        let dest = skills_dir.join(skill.name);
        if dest.exists() {
            fs::remove_dir_all(&dest)?;
            println!("  [-] Removed skill: {}", skill.name);
        }
    }

    // Marker
    let data = data_dir()?;
    let marker = data.join(".installed");
    if marker.exists() {
        fs::remove_file(&marker)?;
    }

    println!("\nDone.");
    Ok(())
}

/// Print install status with [+]/[-] markers.
pub fn print_status() -> Result<()> {
    println!("Konnect v{} — Install Status\n", env!("CARGO_PKG_VERSION"));

    let skills_dir = codex_skills_dir()?;
    println!("Skills ({}):", skills_dir.display());
    for skill in SKILLS {
        let exists = skill
            .files
            .iter()
            .all(|(relative, _)| skills_dir.join(skill.name).join(relative).exists());
        let marker = if exists { "+" } else { "-" };
        println!("  [{}] {}", marker, skill.name);
    }

    // KiCAD detection
    println!("\nKiCAD:");
    if let Some(path) = detect_kicad() {
        println!("  [+] Found: {}", path.display());
    } else {
        println!("  [-] Not found in standard locations");
    }

    let data = data_dir()?;
    let marker = data.join(".installed");
    if marker.exists() {
        let ver = fs::read_to_string(&marker).unwrap_or_default();
        println!("\nInstall marker: v{}", ver.trim());
    } else {
        println!("\nInstall marker: not present (never installed)");
    }

    Ok(())
}

/// Check if install has been completed.
pub fn needs_install() -> bool {
    let marker_current = data_dir()
        .ok()
        .and_then(|d| fs::read_to_string(d.join(".installed")).ok())
        .is_some_and(|version| version.trim() == env!("CARGO_PKG_VERSION"));

    let skills_present = codex_skills_dir().ok().is_some_and(|root| {
        SKILLS.iter().all(|skill| {
            skill
                .files
                .iter()
                .all(|(relative, _)| root.join(skill.name).join(relative).exists())
        })
    });

    !marker_current || !skills_present
}

/// Friendly double-click install: shows banner, runs install, prints config snippet.
pub fn run_double_click_install() -> Result<()> {
    println!("===========================================");
    println!("  Konnect v{}", env!("CARGO_PKG_VERSION"));
    println!("  First-time Setup");
    println!("===========================================\n");

    run_install()?;

    let skills_dir = codex_skills_dir()?;
    let exe = std::env::current_exe()?;

    println!("\n-------------------------------------------");
    println!("Codex integration");
    println!("-------------------------------------------\n");
    println!("  Skills: {}", skills_dir.display());
    println!("  MCP executable: {}", exe.display());
    println!("\nRegister the executable as the `konnect` MCP server in Codex,");
    println!("then restart Codex so it discovers the installed skills.\n");

    println!("Press Enter to close...");
    let mut buf = String::new();
    let _ = std::io::stdin().read_line(&mut buf);
    Ok(())
}

// ─── Internal Helpers ────────────────────────────────────────────────────────

fn home_dir() -> Result<PathBuf> {
    dirs::home_dir().context("could not locate home directory")
}

fn data_dir() -> Result<PathBuf> {
    Ok(home_dir()?.join(".konnect"))
}

fn codex_skills_dir() -> Result<PathBuf> {
    let codex_home = std::env::var_os("CODEX_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or(home_dir()?.join(".codex"));
    Ok(codex_home.join("skills"))
}

fn install_skill(skills_dir: &Path, skill: &crate::manifest::SkillManifest) -> Result<()> {
    let skill_dir = skills_dir.join(skill.name);
    for (relative, content) in skill.files {
        let relative_path = Path::new(relative);
        if relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            bail!("invalid bundled skill path: {relative}");
        }

        let destination = skill_dir.join(relative_path);
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(destination, content)?;
    }
    Ok(())
}

/// Auto-detect KiCAD installation on Windows.
/// Checks registry and standard paths for kicad-cli.exe.
pub fn detect_kicad() -> Option<PathBuf> {
    // Standard paths (check these first — faster than registry)
    let standard_paths = [
        r"C:\Program Files\KiCad\10.0\bin\kicad-cli.exe",
        r"C:\Program Files (x86)\KiCad\10.0\bin\kicad-cli.exe",
        r"C:\Program Files\KiCad\9.0\bin\kicad-cli.exe",
        r"C:\Program Files (x86)\KiCad\9.0\bin\kicad-cli.exe",
    ];

    for path_str in &standard_paths {
        let path = Path::new(path_str);
        if path.exists() {
            return Some(path.to_path_buf());
        }
    }

    // Try registry on Windows
    #[cfg(target_os = "windows")]
    {
        if let Some(path) = detect_kicad_from_registry() {
            return Some(path);
        }
    }

    None
}

#[cfg(target_os = "windows")]
fn detect_kicad_from_registry() -> Option<PathBuf> {
    use std::process::Command;

    // Use reg.exe to query the registry (avoids winreg dependency)
    let output = Command::new("reg")
        .args(["query", r"HKLM\SOFTWARE\KiCad\10.0", "/ve"])
        .output()
        .ok()?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        // Parse the default value which contains the install path
        for line in stdout.lines() {
            if line.contains("REG_SZ") {
                let path_str = line.split("REG_SZ").last()?.trim();
                let cli_path = Path::new(path_str).join("bin").join("kicad-cli.exe");
                if cli_path.exists() {
                    return Some(cli_path);
                }
            }
        }
    }

    None
}
