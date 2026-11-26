//! History of dictations, edits and transcripts. Newest first, capped at `MAX_ENTRIES`,
//! stored as JSON in the app data folder and written atomically (temp file + rename).
//! Recorded audio lives in `audio/` next to it and is removed with its entry.

use crate::types::HistoryEntry;
use anyhow::Context;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

pub const MAX_ENTRIES: usize = 500;

pub struct History {
    path: PathBuf,
    audio_dir: PathBuf,
    entries: Mutex<Vec<HistoryEntry>>,
}

impl History {
    pub fn open(data_dir: &Path) -> Self {
        let path = data_dir.join("history.json");
        let audio_dir = data_dir.join("audio");
        let mut entries = std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| match serde_json::from_str::<Vec<HistoryEntry>>(&s) {
                Ok(v) => Some(v),
                Err(e) => {
                    tracing::warn!("history.json unreadable ({e}), starting empty");
                    None
                }
            })
            .unwrap_or_default();
        // Entries saved before notes were shortened carry the full API error. Shorten them.
        for e in entries.iter_mut() {
            if let Some(n) = e.note.as_mut() {
                if n.starts_with("AI cleanup failed") || n.starts_with("AI cleanup was skipped") {
                    *n = crate::friendly::cleanup_note(n);
                }
            }
        }
        // Session recordings left by a crash or a forced quit belong to no entry.
        if let Ok(dir) = std::fs::read_dir(&audio_dir) {
            for f in dir.flatten() {
                let name = f.file_name().to_string_lossy().to_string();
                if name.starts_with(crate::audio::clip::SESSION_PREFIX) || name.ends_with(".wav.tmp") {
                    let _ = std::fs::remove_file(f.path());
                }
            }
        }
        Self { path, audio_dir, entries: Mutex::new(entries) }
    }

    /// Folder for recorded audio: session recordings while they run, and kept clips.
    pub fn audio_dir(&self) -> &Path {
        &self.audio_dir
    }

    fn remove_audio(&self, entries: &[HistoryEntry]) {
        for e in entries {
            if let Some(f) = &e.audio_file {
                // Only a bare file name is accepted, so an entry cannot point outside the folder.
                if Path::new(f).file_name().map(|n| n == f.as_str()).unwrap_or(false) {
                    let _ = std::fs::remove_file(self.audio_dir.join(f));
                }
            }
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Vec<HistoryEntry>> {
        self.entries.lock().unwrap_or_else(|p| p.into_inner())
    }

    pub fn list(&self) -> Vec<HistoryEntry> {
        self.lock().clone()
    }

    pub fn add(&self, entry: HistoryEntry) -> anyhow::Result<()> {
        let mut e = self.lock();
        e.insert(0, entry);
        if e.len() > MAX_ENTRIES {
            let dropped = e.split_off(MAX_ENTRIES);
            self.remove_audio(&dropped);
        }
        self.persist(&e)
    }

    /// Replace an entry with the same id, for example after it was inserted.
    pub fn update(&self, entry: HistoryEntry) -> anyhow::Result<()> {
        let mut e = self.lock();
        if let Some(slot) = e.iter_mut().find(|x| x.id == entry.id) {
            *slot = entry;
        }
        self.persist(&e)
    }

    pub fn delete(&self, id: &str) -> anyhow::Result<()> {
        let mut e = self.lock();
        let (gone, keep): (Vec<_>, Vec<_>) = e.drain(..).partition(|x| x.id == id);
        *e = keep;
        self.persist(&e)?;
        self.remove_audio(&gone);
        Ok(())
    }

    pub fn clear(&self) -> anyhow::Result<()> {
        let mut e = self.lock();
        let gone = std::mem::take(&mut *e);
        self.persist(&e)?;
        self.remove_audio(&gone);
        Ok(())
    }

    fn persist(&self, entries: &[HistoryEntry]) -> anyhow::Result<()> {
        if let Some(dir) = self.path.parent() {
            std::fs::create_dir_all(dir).context("create data dir")?;
        }
        let tmp = self.path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec(entries)?).context("write history")?;
        std::fs::rename(&tmp, &self.path).context("save history")?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{CleanupMode, HistoryKind};

    fn entry(id: &str) -> HistoryEntry {
        HistoryEntry {
            id: id.into(),
            kind: HistoryKind::Dictation,
            created_at: "2026-01-01T00:00:00Z".into(),
            raw: "um hello".into(),
            text: "Hello".into(),
            cleanup: CleanupMode::Basic,
            instructions: String::new(),
            app: None,
            mode: None,
            duration_ms: None,
            note: None,
            inserted: false,
            audio_file: None,
        }
    }

    #[test]
    fn audio_is_removed_with_its_entry_and_long_notes_are_shortened() {
        let dir = std::env::temp_dir().join(format!("relay-history-{}", uuid::Uuid::new_v4()));
        let h = History::open(&dir);
        std::fs::create_dir_all(h.audio_dir()).unwrap();
        let clip = h.audio_dir().join("a.wav");
        std::fs::write(&clip, b"x").unwrap();
        let leftover = h.audio_dir().join("rec-123.mic.wav");
        std::fs::write(&leftover, b"x").unwrap();

        let mut a = entry("a");
        a.audio_file = Some("a.wav".into());
        a.note = Some("AI cleanup failed, so Basic was used. Claude API returned HTTP 400: invalid_request_error: Your credit balance is too low to access the Anthropic API.".into());
        h.add(a).unwrap();

        let reopened = History::open(&dir);
        assert!(!leftover.exists(), "session leftovers are removed on open");
        assert_eq!(reopened.list()[0].note.as_deref(), Some("Used Basic cleanup. Claude has no credit left."));
        assert!(clip.exists());
        reopened.delete("a").unwrap();
        assert!(!clip.exists(), "the clip goes with its entry");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn add_persists_newest_first_and_caps() {
        let dir = std::env::temp_dir().join(format!("relay-history-{}", uuid::Uuid::new_v4()));
        let h = History::open(&dir);
        for i in 0..(MAX_ENTRIES + 3) {
            h.add(entry(&i.to_string())).unwrap();
        }
        let reopened = History::open(&dir);
        let list = reopened.list();
        assert_eq!(list.len(), MAX_ENTRIES);
        assert_eq!(list[0].id, (MAX_ENTRIES + 2).to_string());
        reopened.delete(&list[0].id).unwrap();
        assert_eq!(History::open(&dir).list().len(), MAX_ENTRIES - 1);
        reopened.clear().unwrap();
        assert!(History::open(&dir).list().is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }
}
