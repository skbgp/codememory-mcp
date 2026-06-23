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
            .filter_entry(|e| {
                let name = e.file_name().to_string_lossy();
                !matches!(name.as_ref(), "node_modules" | "venv" | ".venv" | "target" | "dist" | "build")
            })
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

        self.resolve_dependencies(&tx, &project_id)?;

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
        let parse_result = parser.parse_file(full_path, &content)?;

        // 3. Store symbols
        // First delete any existing symbols for this file to prevent duplicates on update
        tx.execute("DELETE FROM symbols WHERE file_id = ?1", rusqlite::params![file_id])?;

        let mut stmt_sym = tx.prepare_cached(
            "INSERT INTO symbols (file_id, project_id, name, kind, parent_symbol, start_line, end_line)
             VALUES (?1, ?2, ?3, ?4, (SELECT id FROM symbols WHERE name = ?5 AND file_id = ?1 LIMIT 1), ?6, ?7)"
        )?;

        for sym in parse_result.symbols {
            stmt_sym.execute(rusqlite::params![
                file_id,
                project_id,
                sym.name,
                sym.kind,
                sym.parent_name,
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
            for import_text in parse_result.imports {
                let cleaned = match language {
                    "rust" => import_text.trim_start_matches("pub ").trim_start_matches("use ").trim_end_matches(';').to_string(),
                    "python" => import_text.trim_start_matches("import ").trim_start_matches("from ").to_string(),
                    "typescript" => {
                        if let Some(from_idx) = import_text.rfind("from ") {
                            let after = &import_text[from_idx + 5..];
                            after.trim().trim_matches(|c| c == '\'' || c == '"' || c == ';').to_string()
                        } else {
                            import_text
                        }
                    },
                    _ => import_text,
                };
                
                if !cleaned.is_empty() {
                    dep_stmt.execute(rusqlite::params![
                        file_id,
                        cleaned,
                        "imports",
                        project_id
                    ])?;
                }
            }
        }
        
        Ok(())
    }

    fn resolve_dependencies(&self, tx: &rusqlite::Transaction, project_id: &str) -> Result<()> {
        tracing::info!("Resolving AST imports to concrete files...");
        
        // 1. Load all files into a hashmap for quick path matching
        let mut files: std::collections::HashMap<String, String> = std::collections::HashMap::new();
        {
            let mut stmt = tx.prepare_cached("SELECT relative_path, id FROM files WHERE project_id = ?1")?;
            let rows = stmt.query_map(rusqlite::params![project_id], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?;
            for row in rows.flatten() {
                files.insert(row.0, row.1);
            }
        }
        
        // 2. Load unresolved deps
        let mut deps_to_resolve = Vec::new();
        {
            let mut stmt = tx.prepare_cached(
                "SELECT id, source_file, target_path FROM file_deps 
                 WHERE project_id = ?1 AND target_file IS NULL"
            )?;
            let rows = stmt.query_map(rusqlite::params![project_id], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?;
            for row in rows.flatten() {
                deps_to_resolve.push(row);
            }
        }
        
        // 3. Resolve and update
        let mut update_stmt = tx.prepare_cached("UPDATE file_deps SET target_file = ?1 WHERE id = ?2")?;
        let mut sym_stmt = tx.prepare_cached("SELECT file_id FROM symbols WHERE project_id = ?1 AND name = ?2 LIMIT 1")?;
        
        for (dep_id, source_file_id, target_path) in deps_to_resolve {
            let mut resolved_id = None;
            
            // Look up the source file path to resolve relative imports
            let source_path = files.iter().find(|(_, id)| *id == &source_file_id).map(|(p, _)| p.clone()).unwrap_or_default();
            let source_dir = Path::new(&source_path).parent().unwrap_or(Path::new(""));
            
            // Heuristic 1: Module matching (Rust crate::, Python ., TS @/)
            let mod_path = if target_path.contains("::") { // Rust
                target_path.replace("crate::", "").replace("::", "/")
            } else if target_path.contains('.') && !target_path.starts_with('.') { // Python
                target_path.replace('.', "/")
            } else {
                target_path.clone()
            };
            
            // Heuristic 2: Try common extensions and index files
            let candidates = vec![
                // Direct relative
                source_dir.join(&target_path).to_string_lossy().to_string(),
                // Module absolute
                mod_path.clone(),
                format!("{}.rs", mod_path),
                format!("{}/mod.rs", mod_path),
                format!("{}.py", mod_path),
                format!("{}/__init__.py", mod_path),
                format!("{}.ts", mod_path),
                format!("{}.tsx", mod_path),
                format!("{}/index.ts", mod_path),
                format!("{}/index.js", mod_path),
                // JS/TS relative
                source_dir.join(format!("{}.ts", target_path)).to_string_lossy().to_string(),
                source_dir.join(format!("{}.tsx", target_path)).to_string_lossy().to_string(),
                source_dir.join(format!("{}/index.ts", target_path)).to_string_lossy().to_string(),
            ];
            
            for candidate in candidates {
                if let Some(id) = files.get(&candidate) {
                    resolved_id = Some(id.clone());
                    break;
                }
            }
            
            // Heuristic 3: Symbol fallback
            if resolved_id.is_none() {
                // We'll take the last segment of the import path as the probable symbol name
                let symbol_name = target_path.split("::").last()
                    .unwrap_or(&target_path).split('.').last()
                    .unwrap_or(&target_path).split('/').last()
                    .unwrap_or(&target_path).trim();
                    
                if let Ok(mut rows) = sym_stmt.query(rusqlite::params![project_id, symbol_name]) {
                    if let Ok(Some(row)) = rows.next() {
                        let id: String = row.get(0).unwrap_or_default();
                        resolved_id = Some(id);
                    }
                }
            }
            
            if let Some(id) = resolved_id {
                update_stmt.execute(rusqlite::params![id, dep_id])?;
            }
        }
        
        tracing::info!("Dependency resolution complete.");
        Ok(())
    }
}
