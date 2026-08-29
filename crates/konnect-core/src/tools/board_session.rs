//! Process-lifetime safety memory for boards positively observed through KiCad IPC.
//!
//! An unreachable transport is ambiguous after a board was live: KiCad may have
//! crashed with unsaved state. Remembering that observation lets file-fallback
//! tools fail closed instead of editing a potentially stale save (#240).

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

#[derive(Clone, Default)]
pub(crate) struct BoardSessionMemory {
    observed_live: Arc<Mutex<HashSet<PathBuf>>>,
}

impl BoardSessionMemory {
    pub(crate) fn observe_live(&self, board: &Path) {
        self.observed_live
            .lock()
            .expect("board-session memory poisoned")
            .insert(board_key(board));
    }

    pub(crate) fn was_observed_live(&self, board: &Path) -> bool {
        self.observed_live
            .lock()
            .expect("board-session memory poisoned")
            .contains(&board_key(board))
    }
}

/// Prefer filesystem identity. If the path cannot be canonicalized, retain a
/// stable absolute lexical spelling instead of silently dropping the safety
/// observation.
fn board_key(board: &Path) -> PathBuf {
    board.canonicalize().unwrap_or_else(|_| {
        if board.is_absolute() {
            board.to_path_buf()
        } else {
            std::env::current_dir()
                .map(|cwd| cwd.join(board))
                .unwrap_or_else(|_| board.to_path_buf())
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn observations_are_sticky_idempotent_and_board_specific() {
        let dir = tempfile::tempdir().unwrap();
        let board_a = dir.path().join("a.kicad_pcb");
        let board_b = dir.path().join("b.kicad_pcb");
        std::fs::write(&board_a, "").unwrap();
        std::fs::write(&board_b, "").unwrap();
        let memory = BoardSessionMemory::default();

        assert!(!memory.was_observed_live(&board_a));
        memory.observe_live(&board_a);
        memory.observe_live(&board_a);

        assert!(memory.was_observed_live(&board_a));
        assert!(!memory.was_observed_live(&board_b));
    }

    #[test]
    fn canonical_equivalent_paths_share_one_observation() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board.kicad_pcb");
        std::fs::write(&board, "").unwrap();
        let equivalent = dir.path().join("subdir").join("..").join("board.kicad_pcb");
        std::fs::create_dir(dir.path().join("subdir")).unwrap();
        let memory = BoardSessionMemory::default();

        memory.observe_live(&equivalent);

        assert!(memory.was_observed_live(&board));
    }

    #[test]
    fn fresh_memory_does_not_inherit_observations() {
        let dir = tempfile::tempdir().unwrap();
        let board = dir.path().join("board.kicad_pcb");
        std::fs::write(&board, "").unwrap();
        let first = BoardSessionMemory::default();
        first.observe_live(&board);

        assert!(!BoardSessionMemory::default().was_observed_live(&board));
    }
}
