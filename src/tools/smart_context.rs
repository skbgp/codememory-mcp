use anyhow::Result;
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::path::Path;
use sha2::Digest;

use super::search::execute_search;
use super::file_details::{get_file_details, FileDetails};
use super::working_set::get_working_set;

#[derive(Serialize, Deserialize, Debug)]
pub struct SmartContext {
    pub files: Vec<RankedFile>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct RankedFile {
    pub details: FileDetails,
    pub confidence_score: f64,
    pub reasons: Vec<String>,
}

/// The 5-stage smart context ranking pipeline:
/// 1. FTS5 keyword search
/// 2. AST expansion (find related files via file_deps)
/// 3. Git relevance boost (recently changed files matching the query)
/// 4. Working-set boost (files touched in current session)
/// 5. Token-budget pruning (cap output)
pub fn get_smart_context(db: &Connection, repo_path: &Path, query: &str) -> Result<SmartContext> {
    let project_id = hex::encode(sha2::Sha256::digest(repo_path.to_string_lossy().as_bytes()));

    // Accumulator: file_path -> (score, reasons)
    let mut scores: std::collections::HashMap<String, (f64, Vec<String>)> = std::collections::HashMap::new();

    // === Stage 1: FTS5 Keyword Search ===
    let search_results = execute_search(db, query, 10)?;
    for (i, result) in search_results.iter().enumerate() {
        let boost = 1.0 - (i as f64 * 0.08); // Rank decay
        let entry = scores.entry(result.file_path.clone()).or_insert((0.0, Vec::new()));
        entry.0 += boost;
        entry.1.push(format!("FTS5 match: {} ({})", result.symbol_name, result.kind));
    }

    // === Stage 2: AST Expansion via file_deps ===
    // For each file found in stage 1, find files that import it or that it imports
    let matched_files: Vec<String> = scores.keys().cloned().collect();
    for file_path in &matched_files {
        // Find the file_id
        let mut stmt = db.prepare_cached(
            "SELECT id FROM files WHERE relative_path = ?1 AND project_id = ?2"
        )?;
        let mut rows = stmt.query(rusqlite::params![file_path, project_id])?;
        if let Some(row) = rows.next()? {
            let file_id: String = row.get(0)?;

            // Files this file imports
            let mut dep_stmt = db.prepare_cached(
                "SELECT f.relative_path FROM file_deps fd
                 JOIN files f ON fd.target_file = f.id
                 WHERE fd.source_file = ?1"
            )?;
            let dep_rows = dep_stmt.query_map(rusqlite::params![file_id], |row| {
                row.get::<_, String>(0)
            })?;
            for dep in dep_rows.flatten() {
                let entry = scores.entry(dep.clone()).or_insert((0.0, Vec::new()));
                entry.0 += 0.3;
                entry.1.push(format!("imported by {}", file_path));
            }

            // Files that import this file
            let mut rev_stmt = db.prepare_cached(
                "SELECT f.relative_path FROM file_deps fd
                 JOIN files f ON fd.source_file = f.id
                 WHERE fd.target_file = ?1"
            )?;
            let rev_rows = rev_stmt.query_map(rusqlite::params![file_id], |row| {
                row.get::<_, String>(0)
            })?;
            for dep in rev_rows.flatten() {
                let entry = scores.entry(dep.clone()).or_insert((0.0, Vec::new()));
                entry.0 += 0.2;
                entry.1.push(format!("imports {}", file_path));
            }
        }
    }

    // === Stage 3: Git Relevance Boost ===
    // Files recently changed in commits whose message matches the query get boosted
    {
        let mut git_stmt = db.prepare_cached(
            "SELECT DISTINCT relative_path FROM changes
             WHERE project_id = ?1 AND commit_message LIKE ?2
             ORDER BY timestamp DESC LIMIT 10"
        )?;
        let like_query = format!("%{}%", query);
        let git_rows = git_stmt.query_map(rusqlite::params![project_id, like_query], |row| {
            row.get::<_, String>(0)
        })?;
        for path in git_rows.flatten() {
            let entry = scores.entry(path.clone()).or_insert((0.0, Vec::new()));
            entry.0 += 0.4;
            entry.1.push("recently changed in matching commit".to_string());
        }
    }

    // Also boost any file that was recently changed at all (last 50 commits)
    {
        let mut recent_stmt = db.prepare_cached(
            "SELECT relative_path, COUNT(*) as freq FROM changes
             WHERE project_id = ?1
             GROUP BY relative_path
             ORDER BY MAX(timestamp) DESC LIMIT 20"
        )?;
        let recent_rows = recent_stmt.query_map(rusqlite::params![project_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        for row in recent_rows.flatten() {
            if let Some(entry) = scores.get_mut(&row.0) {
                let git_boost = 0.1 * (row.1 as f64).min(3.0); // Cap at 0.3
                entry.0 += git_boost;
                entry.1.push(format!("changed {} times recently", row.1));
            }
        }
    }

    // === Stage 4: Working-Set Boost ===
    if let Ok(ws) = get_working_set(db, repo_path) {
        for file_path in &ws.changed_files {
            let entry = scores.entry(file_path.clone()).or_insert((0.0, Vec::new()));
            entry.0 += 0.5;
            entry.1.push("in current working set".to_string());
        }
    }

    // === Stage 5: Token-Budget Pruning ===
    // Sort by score descending, take top 10
    let mut ranked: Vec<(String, f64, Vec<String>)> = scores
        .into_iter()
        .map(|(path, (score, reasons))| (path, score, reasons))
        .collect();
    ranked.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    ranked.truncate(10);

    // Build final result with file details
    let mut files = Vec::new();
    for (path, score, reasons) in ranked {
        if let Ok(Some(details)) = get_file_details(db, &project_id, &path) {
            files.push(RankedFile {
                details,
                confidence_score: (score * 100.0).round() / 100.0, // Round to 2 decimals
                reasons,
            });
        }
    }

    Ok(SmartContext { files })
}
