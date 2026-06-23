use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct SearchResult {
    pub symbol_name: String,
    pub kind: String,
    pub file_path: String,
    pub start_line: i64,
    pub end_line: i64,
    pub project_id: String,
}

pub fn execute_search(db: &Connection, query: &str, limit: i64) -> Result<Vec<SearchResult>> {
    tracing::info!("Executing FTS5 search for: '{}'", query);

    let mut stmt = db.prepare_cached(
        "SELECT s.name, s.kind, f.relative_path, s.start_line, s.end_line, f.project_id
         FROM fts_symbols fts
         JOIN symbols s ON fts.rowid = s.id
         JOIN files f ON s.file_id = f.id
         WHERE fts_symbols MATCH ?1
         ORDER BY rank
         LIMIT ?2"
    )?;

    // Query transformation for FTS5
    // Single word: prefix match ("Auth" -> "Auth"*)
    // Multi-word: OR-joined prefix terms ("auth login" -> "auth"* OR "login"*)
    let sanitized = query.replace("\"", "");
    let fts_query = if sanitized.contains(' ') {
        sanitized
            .split_whitespace()
            .map(|w| format!("\"{}\"*", w))
            .collect::<Vec<_>>()
            .join(" OR ")
    } else {
        format!("\"{}\"*", sanitized)
    };

    let rows = stmt.query_map(rusqlite::params![fts_query, limit], |row| {
        Ok(SearchResult {
            symbol_name: row.get(0)?,
            kind: row.get(1)?,
            file_path: row.get(2)?,
            start_line: row.get(3)?,
            end_line: row.get(4)?,
            project_id: row.get(5)?,
        })
    })?;

    let mut results = Vec::new();
    for row in rows {
        if let Ok(result) = row {
            results.push(result);
        }
    }

    Ok(results)
}
