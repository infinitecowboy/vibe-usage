use crate::api::ParsedUsage;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

static HISTORY: OnceLock<Mutex<Vec<HistoryEntry>>> = OnceLock::new();

const MAX_AGE_SECS: i64 = 48 * 3600; // 48 hours

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub ts: i64,
    pub session: f32,
    pub weekly: f32,
}

fn history_path() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let dir = PathBuf::from(home).join(".vibe-usage");
    std::fs::create_dir_all(&dir).ok();
    dir.join("history.jsonl")
}

fn load() -> Vec<HistoryEntry> {
    let path = history_path();
    let content = match std::fs::read_to_string(&path) {
        Ok(c) => c,
        Err(_) => return Vec::new(),
    };

    let cutoff = chrono::Utc::now().timestamp() - MAX_AGE_SECS;
    let entries: Vec<HistoryEntry> = content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter(|e: &HistoryEntry| e.ts >= cutoff)
        .collect();

    // Rewrite trimmed file
    if let Ok(json_lines) = entries
        .iter()
        .map(|e| serde_json::to_string(e))
        .collect::<Result<Vec<_>, _>>()
    {
        let trimmed = json_lines.join("\n") + "\n";
        std::fs::write(&path, trimmed).ok();
    }

    entries
}

/// Initialize the history store. Call once at startup.
pub fn init() {
    HISTORY.get_or_init(|| Mutex::new(load()));
}

/// Record a usage snapshot. Deduplicates if values haven't changed.
pub fn record(usage: &ParsedUsage) {
    let Some(m) = HISTORY.get() else { return };
    let mut entries = m.lock().unwrap();

    // Deduplicate: skip if last entry has same values
    if let Some(last) = entries.last() {
        if (last.session - usage.session_percent).abs() < 0.1
            && (last.weekly - usage.weekly_percent).abs() < 0.1
        {
            return;
        }
    }

    let entry = HistoryEntry {
        ts: chrono::Utc::now().timestamp(),
        session: usage.session_percent,
        weekly: usage.weekly_percent,
    };

    // Append to file
    let path = history_path();
    if let Ok(line) = serde_json::to_string(&entry) {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
        {
            writeln!(f, "{}", line).ok();
        }
    }

    entries.push(entry);

    // Trim in-memory
    let cutoff = chrono::Utc::now().timestamp() - MAX_AGE_SECS;
    entries.retain(|e| e.ts >= cutoff);
}

/// Get a copy of the history entries (sorted by timestamp).
pub fn get_history() -> Vec<HistoryEntry> {
    HISTORY
        .get()
        .map(|m| m.lock().unwrap().clone())
        .unwrap_or_default()
}
