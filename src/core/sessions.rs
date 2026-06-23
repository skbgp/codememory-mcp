use anyhow::Result;
use rusqlite::Connection;

pub struct SessionManager<'a> {
    db: &'a Connection,
}

impl<'a> SessionManager<'a> {
    pub fn new(db: &'a Connection) -> Self {
        Self { db }
    }

    pub fn get_or_create_session(&self, project_id: &str, branch_name: &str) -> Result<String> {
        // Find existing session
        let mut stmt = self.db.prepare_cached(
            "SELECT id FROM sessions WHERE branch_name = ?1 AND project_id = ?2 ORDER BY started_at DESC LIMIT 1"
        )?;
        
        let mut rows = stmt.query(rusqlite::params![branch_name, project_id])?;
        if let Some(row) = rows.next()? {
            let id: String = row.get(0)?;
            return Ok(id);
        }

        // Create new session
        let session_id = uuid::Uuid::new_v4().to_string();
        self.db.execute(
            "INSERT INTO sessions (id, project_id, branch_name, started_at) VALUES (?1, ?2, ?3, strftime('%s','now'))",
            rusqlite::params![session_id, project_id, branch_name],
        )?;

        Ok(session_id)
    }

    pub fn set_active_task(&self, session_id: &str, task: &str) -> Result<()> {
        self.db.execute(
            "UPDATE sessions SET summary = ?1 WHERE id = ?2",
            rusqlite::params![task, session_id],
        )?;
        Ok(())
    }

    pub fn get_active_task(&self, session_id: &str) -> Result<Option<String>> {
        let mut stmt = self.db.prepare_cached(
            "SELECT summary FROM sessions WHERE id = ?1"
        )?;
        let mut rows = stmt.query(rusqlite::params![session_id])?;
        if let Some(row) = rows.next()? {
            let task: Option<String> = row.get(0)?;
            Ok(task)
        } else {
            Ok(None)
        }
    }
}
