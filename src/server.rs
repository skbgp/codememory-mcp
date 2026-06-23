use anyhow::Result;
use std::io::{self, BufRead, Write};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::Digest;

pub fn start_stdio_server(db: &Connection, repo_path: &std::path::Path) -> Result<()> {
    let project_id = hex::encode(sha2::Sha256::digest(repo_path.to_string_lossy().as_bytes()));

    let stdin = io::stdin();
    let mut stdout = io::stdout();
    let lock = stdin.lock();

    for line in lock.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        if let Ok(req) = serde_json::from_str::<Value>(&line) {
            let id = req.get("id").cloned().unwrap_or(json!(null));
            let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
            
            let result = match method {
                "initialize" => {
                    json!({
                        "protocolVersion": "2024-11-05",
                        "capabilities": { "tools": { "listChanged": false } },
                        "serverInfo": { "name": "codememory-mcp", "version": "0.1.0" }
                    })
                },
                "tools/list" => {
                    json!({ "tools": [
                        {
                            "name": "search",
                            "description": "Search the codebase for symbols by name or keyword.",
                            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" }, "limit": { "type": "number" } }, "required": ["query"] }
                        },
                        {
                            "name": "get_smart_context",
                            "description": "The killer feature. Given a task description, returns a ranked bundle of relevant files with confidence scores and explainability.",
                            "inputSchema": { "type": "object", "properties": { "query": { "type": "string" } }, "required": ["query"] }
                        },
                        {
                            "name": "get_working_set",
                            "description": "Get the developer's current context: branch, changed files, session state.",
                            "inputSchema": { "type": "object", "properties": {}, "required": [] }
                        },
                        {
                            "name": "get_file_details",
                            "description": "Get detailed metadata and symbols for a specific file.",
                            "inputSchema": { "type": "object", "properties": { "file_path": { "type": "string" } }, "required": ["file_path"] }
                        },
                        {
                            "name": "get_onboarding_path",
                            "description": "Get an ordered reading list of the most critical files for understanding the project.",
                            "inputSchema": { "type": "object", "properties": {}, "required": [] }
                        },
                        {
                            "name": "end_session",
                            "description": "End the current session and save a summary for future retrieval.",
                            "inputSchema": { "type": "object", "properties": { "summary": { "type": "string" } }, "required": ["summary"] }
                        },
                        {
                            "name": "record_feedback",
                            "description": "Record AI retrieval performance feedback for learning.",
                            "inputSchema": { "type": "object", "properties": {
                                "session_id": { "type": "string" }, "file_path": { "type": "string" },
                                "task_category": { "type": "string" }, "retrieved_score": { "type": "number" },
                                "opened": { "type": "boolean" }, "used": { "type": "boolean" }, "dismissed": { "type": "boolean" }
                            }, "required": ["session_id", "file_path"] }
                        }
                    ]})
                },
                "tools/call" => {
                    let empty = json!({});
                    let params = req.get("params").unwrap_or(&empty);
                    let tool_name = params.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    let args = params.get("arguments").unwrap_or(&empty);

                    let content = match tool_name {
                        "search" => {
                            let q = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
                            let limit = args.get("limit").and_then(|l| l.as_i64()).unwrap_or(10);
                            match crate::tools::search::execute_search(db, q, limit) {
                                Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
                                Err(e) => format!("Error: {}", e),
                            }
                        },
                        "get_working_set" => {
                            match crate::tools::working_set::get_working_set(db, repo_path) {
                                Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
                                Err(e) => format!("Error: {}", e),
                            }
                        },
                        "get_smart_context" => {
                            let q = args.get("query").and_then(|q| q.as_str()).unwrap_or("");
                            match crate::tools::smart_context::get_smart_context(db, repo_path, q) {
                                Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
                                Err(e) => format!("Error: {}", e),
                            }
                        },
                        "get_file_details" => {
                            let fp = args.get("file_path").and_then(|s| s.as_str()).unwrap_or("");
                            match crate::tools::file_details::get_file_details(db, &project_id, fp) {
                                Ok(Some(res)) => serde_json::to_string(&res).unwrap_or_default(),
                                Ok(None) => "File not found in index".to_string(),
                                Err(e) => format!("Error: {}", e),
                            }
                        },
                        "get_onboarding_path" => {
                            match crate::tools::onboarding::get_onboarding_path(db, &project_id) {
                                Ok(res) => serde_json::to_string(&res).unwrap_or_default(),
                                Err(e) => format!("Error: {}", e),
                            }
                        },
                        "end_session" => {
                            let summary = args.get("summary").and_then(|s| s.as_str()).unwrap_or("");
                            let sessions = crate::core::sessions::SessionManager::new(db);
                            let git = crate::core::git::GitProvider::new(repo_path);
                            let branch = git.map(|g| g.current_branch().unwrap_or_default()).unwrap_or_default();
                            match sessions.get_or_create_session(&project_id, &branch) {
                                Ok(sid) => {
                                    let _ = sessions.set_active_task(&sid, summary);
                                    // Mark session ended
                                    let _ = db.execute(
                                        "UPDATE sessions SET ended_at = strftime('%s','now') WHERE id = ?1",
                                        rusqlite::params![sid],
                                    );
                                    format!("Session {} ended. Summary saved.", sid)
                                },
                                Err(e) => format!("Error: {}", e),
                            }
                        },
                        "record_feedback" => {
                            let event = crate::tools::feedback::FeedbackEvent {
                                session_id: args.get("session_id").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                                file_path: args.get("file_path").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                                task_category: args.get("task_category").and_then(|s| s.as_str()).unwrap_or("").to_string(),
                                retrieved_score: args.get("retrieved_score").and_then(|s| s.as_f64()).unwrap_or(0.0),
                                opened: args.get("opened").and_then(|s| s.as_bool()).unwrap_or(false),
                                used: args.get("used").and_then(|s| s.as_bool()).unwrap_or(false),
                                dismissed: args.get("dismissed").and_then(|s| s.as_bool()).unwrap_or(false),
                            };
                            match crate::tools::feedback::record_feedback(db, event) {
                                Ok(_) => "Feedback recorded".to_string(),
                                Err(e) => format!("Error: {}", e),
                            }
                        },
                        _ => "Unknown tool".to_string()
                    };

                    json!({
                        "content": [{ "type": "text", "text": content }],
                        "isError": content.starts_with("Error")
                    })
                },
                "notifications/initialized" => { continue; },
                _ => json!({ "error": { "code": -32601, "message": "Method not found" } })
            };

            if id != json!(null) {
                let response = json!({ "jsonrpc": "2.0", "id": id, "result": result });
                writeln!(stdout, "{}", response)?;
                stdout.flush()?;
            }
        }
    }

    Ok(())
}
