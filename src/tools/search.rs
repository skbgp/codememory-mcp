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
}

pub fn execute_search(db: &Connection, query: &str, limit: i64) -> Result<Vec<SearchResult>> {
    tracing::info!("Executing FTS5 search for: '{}'", query);

    let mut stmt = db.prepare_cached(
        "SELECT s.name, s.kind, f.relative_path, s.start_line, s.end_line 
         FROM fts_symbols fts
         JOIN symbols s ON fts.rowid = s.id
         JOIN files f ON s.file_id = f.id
         WHERE fts_symbols MATCH ?1
         ORDER BY rank
         LIMIT ?2"
    )?;

    // Basic query transformation for FTS5 (append * for prefix matching)
    let fts_query = format!("\"{}\"*", query.replace("\"", ""));

    let rows = stmt.query_map(rusqlite::params![fts_query, limit], |row| {
        Ok(SearchResult {
            symbol_name: row.get(0)?,
            kind: row.get(1)?,
            file_path: row.get(2)?,
            start_line: row.get(3)?,
            end_line: row.get(4)?,
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
