use anyhow::{Result, Context};
use std::path::{Path, PathBuf};
use std::process::Command;

pub struct GitProvider {
    repo_path: PathBuf,
}

impl GitProvider {
    pub fn new<P: AsRef<Path>>(path: P) -> Result<Self> {
        let repo_path = path.as_ref().to_path_buf();
        Ok(Self { repo_path })
    }

    pub fn current_branch(&self) -> Result<String> {
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .args(&["branch", "--show-current"])
            .output()
            .context("Failed to run git command")?;
            
        let branch = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if branch.is_empty() {
            Ok("detached".to_string())
        } else {
            Ok(branch)
        }
    }

    pub fn recent_commits(&self, limit: usize) -> Result<Vec<CommitSummary>> {
        let output = Command::new("git")
            .current_dir(&self.repo_path)
            .args(&["log", &format!("-{}", limit), "--pretty=format:%H|%s"])
            .output()?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let mut commits = Vec::new();
        
        for line in stdout.lines() {
            if let Some((hash, msg)) = line.split_once('|') {
                commits.push(CommitSummary {
                    hash: hash.to_string(),
                    message: msg.to_string(),
                });
            }
        }
        
        Ok(commits)
    }
}

pub struct CommitSummary {
    pub hash: String,
    pub message: String,
}
