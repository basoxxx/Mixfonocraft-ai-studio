//! Undo/redo basato su snapshot: semplice, robusto e indipendente dal tipo di comando.

use crate::command::{Command, Outcome};
use crate::project::Project;
use crate::CoreError;

/// Cronologia delle modifiche con undo/redo.
#[derive(Debug, Clone)]
pub struct History {
    undo: Vec<(String, Project)>,
    redo: Vec<(String, Project)>,
    limit: usize,
}

impl Default for History {
    fn default() -> Self {
        Self::new(200)
    }
}

impl History {
    pub fn new(limit: usize) -> Self {
        Self {
            undo: Vec::new(),
            redo: Vec::new(),
            limit: limit.max(1),
        }
    }

    /// Esegue un comando in modo atomico: se fallisce, il progetto resta invariato.
    pub fn execute(&mut self, project: &mut Project, cmd: &Command) -> Result<Outcome, CoreError> {
        let snapshot = project.clone();
        match cmd.apply(project) {
            Ok(out) => {
                self.undo.push((cmd.label(), snapshot));
                if self.undo.len() > self.limit {
                    self.undo.remove(0);
                }
                self.redo.clear();
                Ok(out)
            }
            Err(e) => {
                *project = snapshot;
                Err(e)
            }
        }
    }

    pub fn undo(&mut self, project: &mut Project) -> Option<String> {
        let (label, prev) = self.undo.pop()?;
        let cur = std::mem::replace(project, prev);
        self.redo.push((label.clone(), cur));
        Some(label)
    }

    pub fn redo(&mut self, project: &mut Project) -> Option<String> {
        let (label, next) = self.redo.pop()?;
        let cur = std::mem::replace(project, next);
        self.undo.push((label.clone(), cur));
        Some(label)
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }
    pub fn undo_labels(&self) -> impl Iterator<Item = &str> {
        self.undo.iter().rev().map(|(l, _)| l.as_str())
    }
    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undo_redo_and_atomicity() {
        let mut p = Project::new("h");
        let mut h = History::default();
        h.execute(&mut p, &Command::SetTempo { bpm: 128.0 }).unwrap();
        assert_eq!(p.tempo, 128.0);
        // Batch che fallisce a metà non lascia tracce.
        let bad = Command::Batch {
            commands: vec![Command::SetTempo { bpm: 90.0 }, Command::SetTempo { bpm: 5000.0 }],
        };
        assert!(h.execute(&mut p, &bad).is_err());
        assert_eq!(p.tempo, 128.0);
        assert_eq!(h.undo(&mut p).as_deref(), Some("set_tempo"));
        assert_eq!(p.tempo, 140.0);
        h.redo(&mut p);
        assert_eq!(p.tempo, 128.0);
        assert!(!h.can_redo());
    }
}
