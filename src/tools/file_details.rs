use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug)]
pub struct FileDetails {
    pub relative_path: String,
    pub size_bytes: i64,
    pub line_count: i64,
    pub symbols: Vec<FileSymbol>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct FileSymbol {
    pub name: String,
    pub kind: String,
    pub start_line: i64,
    pub end_line: i64,
}

pub fn get_file_details(db: &Connection, project_id: &str, file_path: &str) -> Result<Option<FileDetails>> {
    let mut stmt = db.prepare_cached(
        "SELECT id, size_bytes, line_count FROM files WHERE relative_path = ?1 AND project_id = ?2"
    )?;
    
    let mut rows = stmt.query(rusqlite::params![file_path, project_id])?;
    if let Some(row) = rows.next()? {
        let file_id: String = row.get(0)?;
        let size_bytes: i64 = row.get(1)?;
        let line_count: i64 = row.get(2)?;
        
        let mut sym_stmt = db.prepare_cached(
            "SELECT name, kind, start_line, end_line FROM symbols WHERE file_id = ?1 ORDER BY start_line ASC"
        )?;
        
        let sym_rows = sym_stmt.query_map(rusqlite::params![file_id], |row| {
            Ok(FileSymbol {
                name: row.get(0)?,
                kind: row.get(1)?,
                start_line: row.get(2)?,
                end_line: row.get(3)?,
            })
        })?;
        
        let mut symbols = Vec::new();
        for sym in sym_rows {
            if let Ok(s) = sym {
                symbols.push(s);
            }
        }
        
        Ok(Some(FileDetails {
            relative_path: file_path.to_string(),
            size_bytes,
            line_count,
            symbols,
        }))
    } else {
        Ok(None)
    }
}
