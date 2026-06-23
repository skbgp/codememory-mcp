# CodeMemory MCP

A local, privacy-first context engine for AI coding assistants. It sits between your codebase and your AI tool, giving the AI a structured understanding of your project instead of dumping raw files into its context window.

Built as an [MCP (Model Context Protocol)](https://modelcontextprotocol.io/) server, it works with any MCP-compatible app: Claude Desktop, Cursor, Antigravity IDE, and others.

---

## The Problem

AI coding assistants are powerful, but they have a fundamental limitation: they don't understand your project's structure. When you ask "help me fix the authentication flow," the AI either gets no context at all, or it gets a massive dump of irrelevant files. This wastes tokens, slows down responses, and produces generic advice.

## What This Does

CodeMemory parses your entire codebase using tree-sitter ASTs, stores the structural data in a local SQLite database, and exposes a set of tools that any AI assistant can call to retrieve exactly the files and symbols it needs.

When the AI asks "what's relevant to authentication?", CodeMemory doesn't just do a keyword search. It runs a **5-stage ranking pipeline**:

1. **FTS5 keyword search** across all indexed symbols
2. **AST dependency expansion** — finds files connected through imports
3. **Git relevance boost** — prioritises files changed in commits related to the query
4. **Working-set boost** — prioritises files you're actively working on
5. **Token-budget pruning** — caps the output so the AI's context window isn't wasted

Each returned file comes with a confidence score and human-readable reasons explaining why it was selected.

---

## Tools Exposed via MCP

| Tool | What it does |
|------|-------------|
| `search` | Full-text search across all symbols (functions, classes, structs) in the codebase |
| `get_smart_context` | The core feature. Given a task description, returns a ranked bundle of relevant files with confidence scores |
| `get_working_set` | Returns the current git branch, session info, and recently changed files |
| `get_file_details` | Returns metadata, size, line count, and all symbols for a specific file |
| `get_onboarding_path` | Returns an ordered reading list of the most important files for understanding the project |
| `end_session` | Saves a session summary for future retrieval |
| `record_feedback` | Logs retrieval quality data so the system can improve over time |

---

## Supported Languages

CodeMemory uses tree-sitter grammars to parse source files. Currently supported:

- **Rust** (.rs)
- **Python** (.py)
- **TypeScript / JavaScript** (.ts, .tsx, .js, .jsx)

Files in other languages are still indexed (tracked by path, size, hash) but without symbol extraction.

---

## Architecture

```
Your Codebase
     |
     v
[Indexer] ---> tree-sitter AST parsing
     |              |
     |         Extracts: functions, classes, structs,
     |                   imports, TODOs, FIXMEs
     v
[SQLite DB]  (WAL mode, local, ~2-5 MB)
     |
     |--- files table (path, hash, size, language)
     |--- symbols table (name, kind, line range)
     |--- fts_symbols (FTS5 full-text search)
     |--- file_deps (import graph)
     |--- todos (TODO/FIXME/HACK comments)
     |--- changes (git commit history)
     |--- sessions (developer session tracking)
     |--- feedback (retrieval quality logs)
     |
     v
[MCP Server]  (JSON-RPC over stdio)
     |
     v
Claude / Cursor / Antigravity / Any MCP Client
```

The entire system runs locally. No network calls, no cloud, no telemetry. The database is a single `.codememory.db` file that sits alongside your project.

---

## Setup

### Prerequisites

- macOS, Linux, or Windows
- [Rust toolchain](https://rustup.rs/) (only needed if building from source)

### Build from Source

```bash
cd codememory-mcp
cargo build --release
```

The compiled binary will be at `target/release/codememory-mcp`. You can move it anywhere and delete the `target/` directory afterwards to reclaim disk space.

### Pre-built Binary

If a `codememory-mcp-executable` file is already present in the project root, it is a pre-compiled release binary. No build step needed.

---

## Configuration

CodeMemory is a standard MCP server that communicates over stdio. Most MCP-compatible apps have a built-in UI to add servers — you just need two pieces of information:

- **Command:** the absolute path to the `codememory-mcp` binary (or `codememory-mcp-executable` if you have the pre-built one)
- **Arguments:** the path to the project folder you want to index

**Tip: use `.` as the argument** instead of a hardcoded project path. This tells CodeMemory to index whatever project is currently open as the active workspace in your IDE. That way you never need to edit the config when switching projects.

If you want to always index one specific project regardless of what you have open, use the full absolute path instead (e.g. `/Users/you/projects/my-app`).

### Antigravity IDE

Open the app and go to **Settings > MCP Servers > Add Server**. Fill in:

| Field | Value |
|-------|-------|
| Name | `codememory` |
| Command | `/absolute/path/to/codememory-mcp-executable` |
| Arguments | `.` |

Alternatively, you can edit the config file manually at `~/.gemini/antigravity-ide/mcp_config.json`:

```json
{
  "mcpServers": {
    "codememory": {
      "command": "/absolute/path/to/codememory-mcp-executable",
      "args": ["."]
    }
  }
}
```

### Claude Desktop

Open **Settings > Developer > Edit Config**, or manually edit `~/Library/Application Support/Claude/claude_desktop_config.json`:

```json
{
  "mcpServers": {
    "codememory": {
      "command": "/absolute/path/to/codememory-mcp-executable",
      "args": ["."]
    }
  }
}
```

### Cursor

Open **Settings > MCP** and click **Add MCP Server**, or add to `.cursor/mcp.json` in your project root:

```json
{
  "mcpServers": {
    "codememory": {
      "command": "/absolute/path/to/codememory-mcp-executable",
      "args": ["."]
    }
  }
}
```

### Any Other MCP Client

Any app that supports the MCP standard can connect. You just need to point it at the binary with the project path as the first argument. The server communicates over stdin/stdout using JSON-RPC.

After connecting, restart the app. CodeMemory will automatically index the project on first launch (takes 1-2 seconds for most codebases) and start serving tool calls.

---

## How It Works Under the Hood

### Indexing

On startup, CodeMemory walks the project directory (respecting `.gitignore`), reads each source file, and:

1. Skips binary files and files larger than 500 KB
2. Computes a SHA-256 hash of the file contents
3. Compares against the stored hash to skip unchanged files (incremental indexing)
4. Parses the file with tree-sitter to extract symbols (functions, classes, structs)
5. Extracts TODO, FIXME, and HACK comments with line numbers
6. Detects import statements and stores them as dependency edges
7. Syncs recent git commit history into the changes table

Re-indexing on subsequent runs only processes files that have actually changed.

### The Smart Context Pipeline

When the AI calls `get_smart_context` with a query like "fix the login validation":

1. **Stage 1 (FTS5):** Searches the full-text index for matching symbols. Results are ranked with decay — the first match scores 1.0, the second 0.92, and so on.

2. **Stage 2 (AST Expansion):** For each file found in Stage 1, looks up the import graph. Files imported by a match get a 0.3 boost. Files that import a match get a 0.2 boost.

3. **Stage 3 (Git Boost):** Searches commit messages for the query keywords. Files changed in matching commits get a 0.4 boost. Files with high recent churn also get a small boost.

4. **Stage 4 (Working-Set):** Files currently modified in the git working tree (uncommitted changes) get a 0.5 boost, since they are likely what the developer is actively working on.

5. **Stage 5 (Pruning):** All scores are aggregated, files are sorted by total score, and the top 10 are returned with full metadata.

Each returned file includes the confidence score and a list of reasons, so the AI (and you) can understand why it was selected.

---

## Example Output

Calling `get_smart_context` with query `"indexer hashing"` on this project itself:

```json
{
  "files": [
    {
      "details": {
        "relative_path": "src/core/indexer.rs",
        "size_bytes": 8405,
        "line_count": 215,
        "symbols": [
          { "name": "ProjectIndexer", "kind": "struct_item" },
          { "name": "index_project", "kind": "function_item" },
          { "name": "index_file", "kind": "function_item" }
        ]
      },
      "confidence_score": 4.02,
      "reasons": [
        "FTS5 match: ProjectIndexer (struct_item)",
        "FTS5 match: index_project (function_item)",
        "FTS5 match: index_file (function_item)",
        "in current working set"
      ]
    }
  ]
}
```

---

## Project Structure

```
src/
  core/
    indexer.rs      # File walking, hashing, tree-sitter parsing, symbol extraction
    git.rs          # Git CLI wrapper (branch, diff, log)
    history.rs      # Syncs git commit history into the database
    sessions.rs     # Developer session tracking
  db/
    connection.rs   # SQLite connection setup (WAL mode)
    schema.rs       # All table definitions, triggers, and FTS5 config
  parsers/
    tree_sitter.rs  # Language-specific AST queries for Rust, Python, TypeScript
  tools/
    search.rs       # FTS5 full-text search with multi-word OR-join
    smart_context.rs # The 5-stage ranking pipeline
    working_set.rs  # Git working-set retrieval
    file_details.rs # Single-file metadata and symbol listing
    onboarding.rs   # Project onboarding reading list
    feedback.rs     # Retrieval quality logging
  server.rs         # MCP protocol handler (JSON-RPC over stdio)
  main.rs           # Entry point: init DB, index, start server
```

---

## Performance

Tested against the [ripgrep](https://github.com/BurntSushi/ripgrep) codebase (a large Rust project with hundreds of files):

| Metric | Value |
|--------|-------|
| Full index time | 1.65 seconds |
| Re-index (no changes) | < 0.1 seconds |
| Database size | ~3 MB |
| Binary size | ~8 MB |

---

## License

MIT
