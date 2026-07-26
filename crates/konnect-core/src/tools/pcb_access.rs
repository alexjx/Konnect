//! Capability types for PCB filesystem access.
//!
//! A board path can only yield a read capability. Mutations belong to a
//! document-bound KiCad IPC client; output artifacts must not alias a board.

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ReadOnlyBoardFile {
    canonical_path: PathBuf,
}

impl ReadOnlyBoardFile {
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if !has_extension(path, "kicad_pcb") {
            anyhow::bail!("expected a .kicad_pcb file: {}", path.display());
        }
        let canonical_path = std::fs::canonicalize(path)
            .with_context(|| format!("cannot open PCB for reading: {}", path.display()))?;
        if !canonical_path.is_file() {
            anyhow::bail!(
                "PCB path is not a regular file: {}",
                canonical_path.display()
            );
        }
        Ok(Self { canonical_path })
    }

    pub fn path(&self) -> &Path {
        &self.canonical_path
    }

    pub fn read_to_string(&self) -> Result<String> {
        std::fs::read_to_string(&self.canonical_path)
            .with_context(|| format!("failed to read PCB {}", self.canonical_path.display()))
    }

    pub fn read(&self) -> Result<Vec<u8>> {
        std::fs::read(&self.canonical_path)
            .with_context(|| format!("failed to read PCB {}", self.canonical_path.display()))
    }
}

pub fn validate_artifact_path(board: &Path, output: &Path) -> Result<()> {
    validate_non_pcb_write(output)?;
    let board = normalize_existing_or_absolute(board)?;
    let output = normalize_existing_or_absolute(output)?;
    if comparable(&board) == comparable(&output) {
        anyhow::bail!(
            "artifact output may not overwrite PCB input: {}",
            board.display()
        );
    }
    Ok(())
}

pub fn validate_non_pcb_write(output: &Path) -> Result<()> {
    if has_extension(output, "kicad_pcb") {
        anyhow::bail!(
            "artifact output may not be a .kicad_pcb path: {}",
            output.display()
        );
    }
    if output.exists() {
        let canonical = std::fs::canonicalize(output)?;
        if has_extension(&canonical, "kicad_pcb") {
            anyhow::bail!(
                "artifact output resolves to a .kicad_pcb file: {}",
                canonical.display()
            );
        }
    }
    Ok(())
}

fn has_extension(path: &Path, expected: &str) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case(expected))
}

fn normalize_existing_or_absolute(path: &Path) -> Result<PathBuf> {
    if path.exists() {
        return std::fs::canonicalize(path).map_err(Into::into);
    }
    if path.is_absolute() {
        return Ok(path.to_path_buf());
    }
    Ok(std::env::current_dir()?.join(path))
}

fn comparable(path: &Path) -> String {
    let value = path.to_string_lossy().replace('\\', "/");
    #[cfg(windows)]
    let value = value.to_ascii_lowercase();
    value
        .trim_start_matches("//?/")
        .trim_end_matches('/')
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn board_capability_reads_without_exposing_a_write_method() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.kicad_pcb");
        std::fs::write(&path, "(kicad_pcb)").unwrap();
        let board = ReadOnlyBoardFile::open(&path).unwrap();
        assert_eq!(board.read_to_string().unwrap(), "(kicad_pcb)");
    }

    #[test]
    fn artifact_cannot_alias_or_be_a_board() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("test.kicad_pcb");
        std::fs::write(&board, "board").unwrap();
        assert!(validate_artifact_path(&board, &board).is_err());
        assert!(validate_artifact_path(&board, &dir.path().join("other.kicad_pcb")).is_err());
        assert!(validate_artifact_path(&board, &dir.path().join("report.json")).is_ok());
    }
}
