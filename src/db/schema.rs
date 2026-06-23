use rusqlite::{Connection, Result};
use std::path::Path;

pub fn initialize_db<P: AsRef<Path>>(path: P) -> Result<Connection> {
    let conn = Connection::open(path)?;
    
    // Efficiency configuration
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    
    // Adaptive mmap sizing (assuming reasonable defaults for now; this can be made more dynamic later)
    // 500 MB mmap for now as a safe default for laptops.
    conn.pragma_update(None, "mmap_size", 500_000_000)?;
    
    create_tables(&conn)?;
    
    Ok(conn)
}

fn create_tables(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS projects (
            id            TEXT PRIMARY KEY,
            name          TEXT NOT NULL,
            root_path     TEXT NOT NULL UNIQUE,
            language      TEXT,
            framework     TEXT,
            last_indexed  INTEGER,
            index_version INTEGER DEFAULT 0,
            config        TEXT
        );

        CREATE TABLE IF NOT EXISTS files (
            id            TEXT PRIMARY KEY,
            project_id    TEXT NOT NULL REFERENCES projects(id),
            relative_path TEXT NOT NULL,
            language      TEXT,
            size_bytes    INTEGER,
            line_count    INTEGER,
            content_hash  TEXT,
            role_summary  TEXT,
            exports_summary TEXT,
            last_indexed  INTEGER,
            UNIQUE(project_id, relative_path)
        );

        CREATE TABLE IF NOT EXISTS symbols (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id       TEXT NOT NULL REFERENCES files(id),
            project_id    TEXT NOT NULL REFERENCES projects(id),
            name          TEXT NOT NULL,
            kind          TEXT NOT NULL,
            signature     TEXT,
            start_line    INTEGER,
            end_line      INTEGER,
            parent_symbol INTEGER REFERENCES symbols(id),
            docstring     TEXT,
            visibility    TEXT,
            is_exported   BOOLEAN DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS edges (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            source_id     INTEGER NOT NULL REFERENCES symbols(id),
            target_id     INTEGER REFERENCES symbols(id),
            target_name   TEXT,
            kind          TEXT NOT NULL,
            project_id    TEXT NOT NULL REFERENCES projects(id)
        );

        CREATE TABLE IF NOT EXISTS file_deps (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            source_file   TEXT NOT NULL REFERENCES files(id),
            target_file   TEXT REFERENCES files(id),
            target_path   TEXT,
            kind          TEXT NOT NULL,
            project_id    TEXT NOT NULL REFERENCES projects(id)
        );

        CREATE TABLE IF NOT EXISTS changes (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id    TEXT NOT NULL REFERENCES projects(id),
            file_id       TEXT REFERENCES files(id),
            relative_path TEXT NOT NULL,
            change_type   TEXT NOT NULL,
            commit_hash   TEXT,
            commit_message TEXT,
            author        TEXT,
            timestamp     INTEGER,
            diff_summary  TEXT,
            diff_content  TEXT,
            session_id    TEXT
        );

        CREATE TABLE IF NOT EXISTS sessions (
            id            TEXT PRIMARY KEY,
            project_id    TEXT NOT NULL REFERENCES projects(id),
            branch_name   TEXT,
            started_at    INTEGER NOT NULL,
            ended_at      INTEGER,
            summary       TEXT,
            files_touched TEXT
        );

        CREATE TABLE IF NOT EXISTS retrieval_feedback (
            id             INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id     TEXT REFERENCES sessions(id),
            file_id        TEXT REFERENCES files(id),
            task_category  TEXT,
            retrieved_score REAL,
            opened         BOOLEAN,
            used           BOOLEAN,
            dismissed      BOOLEAN,
            timestamp      INTEGER
        );

        CREATE TABLE IF NOT EXISTS chunks (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            file_id       TEXT NOT NULL REFERENCES files(id),
            start_line    INTEGER,
            end_line      INTEGER,
            summary       TEXT
        );

        CREATE TABLE IF NOT EXISTS todos (
            id            INTEGER PRIMARY KEY AUTOINCREMENT,
            project_id    TEXT NOT NULL REFERENCES projects(id),
            file_id       TEXT NOT NULL REFERENCES files(id),
            line_number   INTEGER,
            content       TEXT
        );

        CREATE VIRTUAL TABLE IF NOT EXISTS fts_symbols USING fts5(
            name, signature, docstring,
            content='symbols',
            content_rowid='id'
        );

        CREATE TRIGGER IF NOT EXISTS symbols_ai AFTER INSERT ON symbols BEGIN
            INSERT INTO fts_symbols(rowid, name, signature, docstring)
            VALUES (new.id, new.name, new.signature, new.docstring);
        END;

        CREATE TRIGGER IF NOT EXISTS symbols_ad AFTER DELETE ON symbols BEGIN
            INSERT INTO fts_symbols(fts_symbols, rowid, name, signature, docstring)
            VALUES ('delete', old.id, old.name, old.signature, old.docstring);
        END;

        CREATE TRIGGER IF NOT EXISTS symbols_au AFTER UPDATE ON symbols BEGIN
            INSERT INTO fts_symbols(fts_symbols, rowid, name, signature, docstring)
            VALUES ('delete', old.id, old.name, old.signature, old.docstring);
            INSERT INTO fts_symbols(rowid, name, signature, docstring)
            VALUES (new.id, new.name, new.signature, new.docstring);
        END;

        CREATE TABLE IF NOT EXISTS symbol_embeddings (
            symbol_id     INTEGER PRIMARY KEY REFERENCES symbols(id),
            embedding     BLOB NOT NULL
        );

        -- Indexes
        CREATE INDEX IF NOT EXISTS idx_files_project ON files(project_id);
        CREATE INDEX IF NOT EXISTS idx_symbols_file ON symbols(file_id);
        CREATE INDEX IF NOT EXISTS idx_symbols_name ON symbols(name);
        CREATE INDEX IF NOT EXISTS idx_symbols_kind ON symbols(kind);
        CREATE INDEX IF NOT EXISTS idx_changes_project ON changes(project_id);
        CREATE INDEX IF NOT EXISTS idx_changes_timestamp ON changes(timestamp);
        CREATE INDEX IF NOT EXISTS idx_edges_source ON edges(source_id);
        CREATE INDEX IF NOT EXISTS idx_edges_target ON edges(target_id);
        CREATE INDEX IF NOT EXISTS idx_sessions_project ON sessions(project_id);
        "
    )?;

    Ok(())
}
