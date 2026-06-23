use anyhow::Result;
use rusqlite::Connection;

pub struct FeedbackEvent {
    pub session_id: String,
    pub file_path: String,
    pub task_category: String,
    pub retrieved_score: f64,
    pub opened: bool,
    pub used: bool,
    pub dismissed: bool,
}

pub fn record_feedback(db: &Connection, event: FeedbackEvent) -> Result<()> {
    tracing::info!("Recording feedback for {}: dismissed={}", event.file_path, event.dismissed);

    // Resolve file_id from relative_path
    let mut stmt = db.prepare_cached("SELECT id FROM files WHERE relative_path = ?1 LIMIT 1")?;
    let mut rows = stmt.query(rusqlite::params![event.file_path])?;
    let file_id: Option<String> = if let Some(row) = rows.next()? {
        Some(row.get(0)?)
    } else {
        None
    };

    if let Some(f_id) = file_id {
        db.execute(
            "INSERT INTO retrieval_feedback 
             (session_id, file_id, task_category, retrieved_score, opened, used, dismissed, timestamp)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, strftime('%s','now'))",
            rusqlite::params![
                event.session_id,
                f_id,
                event.task_category,
                event.retrieved_score,
                event.opened,
                event.used,
                event.dismissed
            ],
        )?;
    } else {
        tracing::warn!("Feedback recorded for unknown file: {}", event.file_path);
    }

    Ok(())
}
