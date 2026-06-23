use rusqlite::Connection;
use std::path::Path;
use anyhow::Result;
use sha2::Digest;

pub struct ProjectIndexer<'a> {
    db: &'a Connection,
}

impl<'a> ProjectIndexer<'a> {
    pub fn new(db: &'a Connection) -> Self {
        Self { db }
    }

    pub fn index_project<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let project_path = path.as_ref();
        let project_id = hex::encode(sha2::Sha256::digest(project_path.to_string_lossy().as_bytes()));
        
        tracing::info!("Starting indexing for project at: {:?}", project_path);

        // Ensure project exists in DB
        self.db.execute(
            "INSERT INTO projects (id, name, root_path, language, last_indexed, index_version)
             VALUES (?1, ?2, ?3, ?4, strftime('%s','now'), 1)
             ON CONFLICT(id) DO UPDATE SET last_indexed = strftime('%s','now')",
            rusqlite::params![
                project_id,
                project_path.file_name().unwrap_or_default().to_string_lossy(),
                project_path.to_string_lossy(),
                "mixed"
            ],
        )?;

        let walker = ignore::WalkBuilder::new(project_path)
            .hidden(true)
            .git_ignore(true)
            .build();

        let tx = self.db.unchecked_transaction()?;
        let mut parser = crate::parsers::tree_sitter::TreeSitterParser::new();

        for result in walker {
            match result {
                Ok(entry) => {
                    let path = entry.path();
                    if path.is_file() {
                        let rel_path = path.strip_prefix(project_path).unwrap_or(path);
                        if let Err(e) = self.index_file(&tx, &project_id, path, rel_path, &mut parser) {
                            tracing::error!("Failed to index {:?}: {}", rel_path, e);
                        }
                    }
                }
                Err(err) => tracing::error!("Error walking directory: {}", err),
            }
        }

        tx.commit()?;
        tracing::info!("Indexing complete.");
        Ok(())
    }

    fn index_file(&self, tx: &rusqlite::Transaction, project_id: &str, full_path: &Path, rel_path: &Path, parser: &mut crate::parsers::tree_sitter::TreeSitterParser) -> Result<()> {
        // 1. Check size before reading (skip > 500KB)
        if let Ok(metadata) = std::fs::metadata(full_path) {
            if metadata.len() > 500_000 {
                tracing::debug!("Skipping large file: {:?}", rel_path);
                return Ok(());
            }
        }

        // Read content and hash (skips non-UTF8 binary files)
        let content = match std::fs::read_to_string(full_path) {
            Ok(c) => c,
            Err(_) => {
                tracing::debug!("Skipping non-UTF8/binary file: {:?}", rel_path);
                return Ok(());
            }
        };
        
        let size_bytes = content.len() as i64;
        let line_count = content.lines().count() as i64;
        let content_hash = hex::encode(sha2::Sha256::digest(content.as_bytes()));
        
        let file_id = hex::encode(sha2::Sha256::digest(format!("{}:{}", project_id, rel_path.to_string_lossy()).as_bytes()));

        // Check if file is already indexed and unchanged
        let mut stmt = tx.prepare_cached("SELECT content_hash FROM files WHERE id = ?1")?;
        let mut rows = stmt.query(rusqlite::params![file_id])?;
        if let Some(row) = rows.next()? {
            let existing_hash: String = row.get(0)?;
            if existing_hash == content_hash {
                tracing::debug!("Skipping unchanged file: {:?}", rel_path);
                return Ok(());
            }
        }

        tracing::info!("Indexing file: {:?}", rel_path);

        let ext = full_path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let language = match ext {
            "rs" => "rust",
            "py" => "python",
            "ts" | "tsx" | "js" | "jsx" => "typescript",
            _ => "unknown",
        };

        tx.execute(
            "INSERT INTO files (id, project_id, relative_path, language, size_bytes, line_count, content_hash, last_indexed)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%s','now'))
             ON CONFLICT(id) DO UPDATE SET 
                size_bytes=excluded.size_bytes, 
                line_count=excluded.line_count, 
                content_hash=excluded.content_hash, 
                last_indexed=excluded.last_indexed",
            rusqlite::params![
                file_id,
                project_id,
                rel_path.to_string_lossy(),
                language,
                size_bytes,
                line_count,
                content_hash
            ],
        )?;

        // 2. Tree-sitter parse
        let symbols = parser.parse_file(full_path, &content)?;

        // 3. Store symbols
        // First delete any existing symbols for this file to prevent duplicates on update
        tx.execute("DELETE FROM symbols WHERE file_id = ?1", rusqlite::params![file_id])?;

        let mut stmt_sym = tx.prepare_cached(
            "INSERT INTO symbols (file_id, project_id, name, kind, start_line, end_line)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)"
        )?;

        for sym in symbols {
            stmt_sym.execute(rusqlite::params![
                file_id,
                project_id,
                sym.name,
                sym.kind,
                sym.start_line as i64,
                sym.end_line as i64
            ])?;
        }

        // 4. Extract TODOs and FIXMEs
        tx.execute("DELETE FROM todos WHERE file_id = ?1", rusqlite::params![file_id])?;
        {
            let mut todo_stmt = tx.prepare_cached(
                "INSERT INTO todos (project_id, file_id, line_number, content) VALUES (?1, ?2, ?3, ?4)"
            )?;
            for (i, line) in content.lines().enumerate() {
                let trimmed = line.trim();
                if trimmed.contains("TODO:") || trimmed.contains("FIXME:") || trimmed.contains("HACK:") {
                    todo_stmt.execute(rusqlite::params![
                        project_id,
                        file_id,
                        (i + 1) as i64,
                        trimmed.to_string()
                    ])?;
                }
            }
        }

        // 5. Extract imports and store as file_deps
        tx.execute("DELETE FROM file_deps WHERE source_file = ?1", rusqlite::params![file_id])?;
        {
            let mut dep_stmt = tx.prepare_cached(
                "INSERT INTO file_deps (source_file, target_path, kind, project_id) VALUES (?1, ?2, ?3, ?4)"
            )?;
            for line in content.lines() {
                let trimmed = line.trim();
                let import_path = match language {
                    "rust" => {
                        if trimmed.starts_with("use ") {
                            Some(trimmed.trim_start_matches("use ").trim_end_matches(';').to_string())
                        } else { None }
                    },
                    "python" => {
                        if trimmed.starts_with("import ") {
                            Some(trimmed.trim_start_matches("import ").to_string())
                        } else if trimmed.starts_with("from ") {
                            Some(trimmed.to_string())
                        } else { None }
                    },
                    "typescript" => {
                        if trimmed.contains("import ") && trimmed.contains("from ") {
                            // Extract the "from 'xxx'" part
                            if let Some(from_idx) = trimmed.rfind("from ") {
                                let after = &trimmed[from_idx + 5..];
                                let cleaned = after.trim().trim_matches(|c| c == '\'' || c == '"' || c == ';');
                                Some(cleaned.to_string())
                            } else { None }
                        } else { None }
                    },
                    _ => None,
                };

                if let Some(path) = import_path {
                    dep_stmt.execute(rusqlite::params![
                        file_id,
                        path,
                        "imports",
                        project_id
                    ])?;
                }
            }
        }
        
        Ok(())
    }
}
