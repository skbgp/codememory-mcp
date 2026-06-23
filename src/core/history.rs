use anyhow::Result;
use rusqlite::Connection;
use crate::core::git::GitProvider;
use std::path::Path;
use sha2::Digest;

/// Populates the `changes` table from git log on startup.
/// Only inserts commits not already recorded.
pub fn sync_git_history(db: &Connection, repo_path: &Path, depth: usize) -> Result<()> {
    let git = GitProvider::new(repo_path)?;
    let project_id = hex::encode(sha2::Sha256::digest(repo_path.to_string_lossy().as_bytes()));
    let commits = git.recent_commits(depth)?;

    let mut insert_stmt = db.prepare_cached(
        "INSERT OR IGNORE INTO changes (project_id, relative_path, change_type, commit_hash, commit_message, timestamp)
         VALUES (?1, ?2, 'modified', ?3, ?4, strftime('%s','now'))"
    )?;

    // Check which commits we already have
    let mut check_stmt = db.prepare_cached(
        "SELECT COUNT(*) FROM changes WHERE commit_hash = ?1"
    )?;

    for commit in commits {
        let count: i64 = check_stmt.query_row(rusqlite::params![commit.hash], |row| row.get(0))?;
        if count > 0 {
            continue; // Already recorded
        }

        // Get changed files for this commit
        let output = std::process::Command::new("git")
            .current_dir(repo_path)
            .args(&["diff-tree", "--no-commit-id", "--name-only", "-r", &commit.hash])
            .output();

        if let Ok(output) = output {
            let stdout = String::from_utf8_lossy(&output.stdout);
            for file_path in stdout.lines() {
                if !file_path.is_empty() {
                    insert_stmt.execute(rusqlite::params![
                        project_id,
                        file_path,
                        commit.hash,
                        commit.message,
                    ])?;
                }
            }
        }
    }

    Ok(())
}
