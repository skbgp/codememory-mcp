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

        let mut tx = self.db.unchecked_transaction()?;
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
        // Read content and hash
        let content = std::fs::read_to_string(full_path).unwrap_or_default();
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
                "unknown", // To be determined by Tree-sitter
                size_bytes,
                line_count,
                content_hash
            ],
        )?;

        // 2. Tree-sitter parse
        let symbols = parser.parse_file(full_path, &content)?;

        // 3. Store symbols
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

        // TODO: 4. Store imports
        
        Ok(())
    }
}
