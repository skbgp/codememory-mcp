use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct OnboardingEntry {
    pub relative_path: String,
    pub language: String,
    pub symbol_count: i64,
    pub import_count: i64,
    pub reason: String,
}

/// Returns an ordered reading list of the most critical files for understanding a project.
/// Ranks by: most imported (other files depend on them), most symbols, entry points.
pub fn get_onboarding_path(db: &Connection, project_id: &str) -> Result<Vec<OnboardingEntry>> {
    // Find files that are most imported by other files (highest dependency fan-in)
    let mut stmt = db.prepare_cached(
        "SELECT f.relative_path, f.language,
                (SELECT COUNT(*) FROM symbols s WHERE s.file_id = f.id) as sym_count,
                (SELECT COUNT(*) FROM file_deps fd WHERE fd.source_file = f.id) as import_count
         FROM files f
         WHERE f.project_id = ?1
         ORDER BY sym_count DESC
         LIMIT 5"
    )?;

    let rows = stmt.query_map(rusqlite::params![project_id], |row| {
        Ok(OnboardingEntry {
            relative_path: row.get(0)?,
            language: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
            symbol_count: row.get(2)?,
            import_count: row.get(3)?,
            reason: String::new(),
        })
    })?;

    let mut entries: Vec<OnboardingEntry> = Vec::new();
    for row in rows.flatten() {
        entries.push(row);
    }

    // Add reasons
    for (i, entry) in entries.iter_mut().enumerate() {
        let mut reasons = Vec::new();
        if i == 0 { reasons.push("most symbols — likely the core module".to_string()); }
        if entry.import_count > 3 { reasons.push(format!("imports {} dependencies", entry.import_count)); }
        if entry.relative_path.contains("main") || entry.relative_path.contains("index") || entry.relative_path.contains("app") {
            reasons.push("entry point".to_string());
        }
        entry.reason = if reasons.is_empty() { "key module".to_string() } else { reasons.join(", ") };
    }

    Ok(entries)
}
