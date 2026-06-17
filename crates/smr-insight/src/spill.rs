use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::models::TraceTurn;

/// Disk-backed overflow when the in-memory insight queue is full.
pub struct SpillQueue {
    dir: PathBuf,
}

impl SpillQueue {
    pub fn open(data_dir: &Path) -> Result<Self> {
        let dir = data_dir.join("insight_spill");
        fs::create_dir_all(&dir).with_context(|| format!("create spill dir {}", dir.display()))?;
        Ok(Self { dir })
    }

    pub fn push(&self, turn: &TraceTurn) -> Result<()> {
        let name = spill_filename(&turn.audit_id);
        let path = self.dir.join(name);
        let json = serde_json::to_vec(turn).context("serialize spilled trace turn")?;
        fs::write(&path, json).with_context(|| format!("write spill {}", path.display()))?;
        Ok(())
    }

    pub fn pop_oldest(&self) -> Result<Option<TraceTurn>> {
        let mut files: Vec<PathBuf> = fs::read_dir(&self.dir)
            .with_context(|| format!("read spill dir {}", self.dir.display()))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort_by_key(|p| {
            fs::metadata(p)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::SystemTime::UNIX_EPOCH)
        });
        let Some(path) = files.into_iter().next() else {
            return Ok(None);
        };
        let bytes = fs::read(&path).with_context(|| format!("read spill {}", path.display()))?;
        let turn: TraceTurn =
            serde_json::from_slice(&bytes).context("deserialize spilled trace turn")?;
        let _ = fs::remove_file(&path);
        Ok(Some(turn))
    }

    pub fn pending_count(&self) -> u64 {
        fs::read_dir(&self.dir)
            .map(|rd| {
                rd.filter_map(|e| e.ok())
                    .filter(|e| {
                        e.path()
                            .extension()
                            .is_some_and(|x| x == "json")
                    })
                    .count() as u64
            })
            .unwrap_or(0)
    }

    pub fn oldest_pending_age_secs(&self) -> Option<u64> {
        let mut oldest: Option<std::time::SystemTime> = None;
        let Ok(rd) = fs::read_dir(&self.dir) else {
            return None;
        };
        for entry in rd.filter_map(|e| e.ok()) {
            if !entry.path().extension().is_some_and(|x| x == "json") {
                continue;
            }
            if let Ok(meta) = entry.metadata() {
                if let Ok(modified) = meta.modified() {
                    oldest = Some(match oldest {
                        Some(prev) => prev.min(modified),
                        None => modified,
                    });
                }
            }
        }
        oldest.map(|t| {
            std::time::SystemTime::now()
                .duration_since(t)
                .unwrap_or_default()
                .as_secs()
        })
    }
}

fn spill_filename(audit_id: &str) -> String {
    let safe: String = audit_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{safe}.json")
}
