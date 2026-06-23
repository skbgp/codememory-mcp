use anyhow::Result;
use std::path::Path;
use tree_sitter::Parser;
use streaming_iterator::StreamingIterator;

pub struct TreeSitterParser {
    parser: Parser,
}

impl TreeSitterParser {
    pub fn new() -> Self {
        Self {
            parser: Parser::new(),
        }
    }

    pub fn parse_file(&mut self, path: &Path, content: &str) -> Result<Vec<Symbol>> {
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        
        let (language_fn, query_str) = match ext {
            "rs" => (
                tree_sitter_rust::LANGUAGE,
                "(function_item name: (identifier) @name) @symbol
                 (struct_item name: (type_identifier) @name) @symbol
                 (impl_item type: (type_identifier) @name) @symbol"
            ),
            "py" => (
                tree_sitter_python::LANGUAGE,
                "(function_definition name: (identifier) @name) @symbol
                 (class_definition name: (identifier) @name) @symbol"
            ),
            "ts" | "tsx" | "js" | "jsx" => (
                tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
                "(function_declaration name: (identifier) @name) @symbol
                 (class_declaration name: (type_identifier) @name) @symbol
                 (method_definition name: (property_identifier) @name) @symbol"
            ),
            _ => return Ok(vec![]), // Unsupported language
        };

        let language: tree_sitter::Language = language_fn.into();
        self.parser.set_language(&language)?;
        
        let tree = self.parser.parse(content, None)
            .ok_or_else(|| anyhow::anyhow!("Failed to parse file: {:?}", path))?;

        let query = tree_sitter::Query::new(&language, query_str)
            .map_err(|e| anyhow::anyhow!("Query error: {}", e))?;
            
        let mut query_cursor = tree_sitter::QueryCursor::new();
        let mut matches = query_cursor.matches(&query, tree.root_node(), content.as_bytes());

        let mut symbols = Vec::new();
        let name_idx = query.capture_index_for_name("name").unwrap();
        let symbol_idx = query.capture_index_for_name("symbol").unwrap();

        while let Some(m) = matches.next() {
            let mut name = String::new();
            let mut kind = String::new();
            let mut start_line = 0;
            let mut end_line = 0;

            for cap in m.captures {
                if cap.index == name_idx {
                    if let Ok(text) = cap.node.utf8_text(content.as_bytes()) {
                        let text_str: &str = text;
                        name = text_str.to_string();
                    }
                } else if cap.index == symbol_idx {
                    kind = cap.node.kind().to_string();
                    start_line = cap.node.start_position().row;
                    end_line = cap.node.end_position().row;
                }
            }

            if !name.is_empty() {
                symbols.push(Symbol {
                    name,
                    kind,
                    start_line,
                    end_line,
                    parent_name: None,
                });
            }
        }
        
        Ok(symbols)
    }
}

pub struct Symbol {
    pub name: String,
    pub kind: String,
    pub start_line: usize,
    pub end_line: usize,
    pub parent_name: Option<String>,
}
