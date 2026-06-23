pub mod core;
pub mod db;
pub mod parsers;
pub mod tools;
pub mod utils;
pub mod server;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    tracing::info!("Starting CodeMemory MCP Server...");

    let args: Vec<String> = std::env::args().collect();
    if args.len() < 2 {
        tracing::error!("Usage: codememory-mcp <path_to_index>");
        return Ok(());
    }
    let target_path = &args[1];

    // Setup DB
    let db_path = ".codememory.db";
    let conn = db::connection::init(db_path)?;
    tracing::info!("SQLite Database connected and schema applied.");

    // Run Indexer test
    let indexer = core::indexer::ProjectIndexer::new(&conn);
    indexer.index_project(target_path)?;

    // Run Search test
    let results = tools::search::execute_search(&conn, "TreeSitter", 5)?;
    tracing::info!("Search results for 'TreeSitter':");
    for r in results {
        tracing::info!("  - [{}] {} in {} (L{}-L{})", r.kind, r.symbol_name, r.file_path, r.start_line, r.end_line);
    }

    // Test Git Provider
    let git = core::git::GitProvider::new(target_path)?;
    tracing::info!("Current Git Branch: {}", git.current_branch()?);
    let commits = git.recent_commits(3)?;
    for c in commits {
        tracing::info!("Recent Commit: {} - {}", c.hash, c.message.lines().next().unwrap_or(""));
    }

    // Start MCP Server (stdio by default)
    // server::start().await?;

    Ok(())
}
