use anyhow::{Result, Context};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::process::Command;
use std::path::Path;
use sha2::Digest;
use crate::core::git::GitProvider;
use crate::core::sessions::SessionManager;

#[derive(Serialize, Deserialize, Debug)]
pub struct WorkingSet {
    pub branch: String,
    pub session_id: String,
    pub active_task: Option<String>,
    pub changed_files: Vec<String>,
}

pub fn get_working_set(db: &Connection, repo_path: &Path) -> Result<WorkingSet> {
    let git = GitProvider::new(repo_path)?;
    let branch = git.current_branch()?;

    let project_id = hex::encode(sha2::Sha256::digest(repo_path.to_string_lossy().as_bytes()));

    let sessions = SessionManager::new(db);
    let session_id = sessions.get_or_create_session(&project_id, &branch)?;
    let active_task = sessions.get_active_task(&session_id)?;

    // Get uncommitted and untracked files
    let output = Command::new("git")
        .current_dir(repo_path)
        .args(&["ls-files", "--others", "--exclude-standard", "--modified"])
        .output()
        .context("Failed to run git ls-files")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut changed_files: Vec<String> = stdout
        .lines()
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect();

    // Get committed files that differ from master/main (if we aren't on master/main)
    if branch != "master" && branch != "main" {
        let diff_output = Command::new("git")
            .current_dir(repo_path)
            .args(&["diff", "--name-only", "master...HEAD"])
            .output();

        if let Ok(output) = diff_output {
            let diff_stdout = String::from_utf8_lossy(&output.stdout);
            for line in diff_stdout.lines() {
                if !line.is_empty() && !changed_files.contains(&line.to_string()) {
                    changed_files.push(line.to_string());
                }
            }
        }
    }

    Ok(WorkingSet {
        branch,
        session_id,
        active_task,
        changed_files,
    })
}
