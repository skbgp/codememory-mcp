pub mod core;
pub mod db;
pub mod parsers;
pub mod tools;
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

    // Run Indexer
    let indexer = core::indexer::ProjectIndexer::new(&conn);
    indexer.index_project(target_path)?;

    // Sync git history into changes table
    if let Err(e) = core::history::sync_git_history(&conn, std::path::Path::new(target_path), 100) {
        tracing::warn!("Git history sync skipped: {}", e);
    }

    // Start MCP Server (stdio JSON-RPC loop)
    tracing::info!("CodeMemory fully initialized. Listening on stdio...");
    server::start_stdio_server(&conn, std::path::Path::new(target_path))?;

    Ok(())
}
