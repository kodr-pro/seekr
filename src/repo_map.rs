use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};
use streaming_iterator::StreamingIterator;

struct LangConfig {
    language: Language,
    extensions: &'static [&'static str],
    symbol_query: &'static str,
    name: &'static str,
}

static LANGUAGES: LazyLock<Vec<LangConfig>> = LazyLock::new(|| {
    vec![
        LangConfig {
            language: tree_sitter_rust::LANGUAGE.into(),
            extensions: &["rs"],
            symbol_query: r#"
                [
                  (function_item name: (identifier) @symbol)
                  (struct_item name: (type_identifier) @symbol)
                  (enum_item name: (type_identifier) @symbol)
                  (impl_item type: (type_identifier) @symbol)
                  (trait_item name: (type_identifier) @symbol)
                  (type_item name: (type_identifier) @symbol)
                  (mod_item name: (identifier) @symbol)
                  (const_item name: (identifier) @symbol)
                  (static_item name: (identifier) @symbol)
                  (macro_definition name: (identifier) @symbol)
                ]
            "#,
            name: "Rust",
        },
        LangConfig {
            language: tree_sitter_python::LANGUAGE.into(),
            extensions: &["py"],
            symbol_query: r#"
                [
                  (function_definition name: (identifier) @symbol)
                  (class_definition name: (identifier) @symbol)
                ]
            "#,
            name: "Python",
        },
        LangConfig {
            language: tree_sitter_go::LANGUAGE.into(),
            extensions: &["go"],
            symbol_query: r#"
                [
                  (function_declaration name: (identifier) @symbol)
                  (method_declaration name: (field_identifier) @symbol)
                  (type_declaration (type_spec name: (type_identifier) @symbol))
                ]
            "#,
            name: "Go",
        },
        LangConfig {
            language: tree_sitter_javascript::LANGUAGE.into(),
            extensions: &["js", "jsx", "mjs"],
            symbol_query: r#"
                [
                  (function_declaration name: (identifier) @symbol)
                  (class_declaration name: (identifier) @symbol)
                  (method_definition name: (property_identifier) @symbol)
                ]
            "#,
            name: "JavaScript",
        },
        LangConfig {
            language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            extensions: &["ts"],
            symbol_query: r#"
                [
                  (function_declaration name: (identifier) @symbol)
                  (class_declaration name: (type_identifier) @symbol)
                  (interface_declaration name: (type_identifier) @symbol)
                  (type_alias_declaration name: (type_identifier) @symbol)
                  (enum_declaration name: (identifier) @symbol)
                ]
            "#,
            name: "TypeScript",
        },
        LangConfig {
            language: tree_sitter_typescript::LANGUAGE_TSX.into(),
            extensions: &["tsx"],
            symbol_query: r#"
                [
                  (function_declaration name: (identifier) @symbol)
                  (class_declaration name: (type_identifier) @symbol)
                  (interface_declaration name: (type_identifier) @symbol)
                  (type_alias_declaration name: (type_identifier) @symbol)
                  (enum_declaration name: (identifier) @symbol)
                ]
            "#,
            name: "TSX",
        },
        LangConfig {
            language: tree_sitter_c::LANGUAGE.into(),
            extensions: &["c", "h"],
            symbol_query: r#"
                [
                  (function_definition declarator: (function_declarator declarator: (identifier) @symbol))
                  (struct_specifier name: (type_identifier) @symbol)
                  (type_definition declarator: (type_identifier) @symbol)
                ]
            "#,
            name: "C",
        },
        LangConfig {
            language: tree_sitter_cpp::LANGUAGE.into(),
            extensions: &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
            symbol_query: r#"
                [
                  (function_definition declarator: (function_declarator declarator: (identifier) @symbol))
                  (class_specifier name: (type_identifier) @symbol)
                  (struct_specifier name: (type_identifier) @symbol)
                  (namespace_definition name: (identifier) @symbol)
                ]
            "#,
            name: "C++",
        },
        LangConfig {
            language: tree_sitter_java::LANGUAGE.into(),
            extensions: &["java"],
            symbol_query: r#"
                [
                  (class_declaration name: (identifier) @symbol)
                  (interface_declaration name: (identifier) @symbol)
                  (enum_declaration name: (identifier) @symbol)
                  (method_declaration name: (identifier) @symbol)
                  (constructor_declaration name: (identifier) @symbol)
                ]
            "#,
            name: "Java",
        },
        LangConfig {
            language: tree_sitter_ruby::LANGUAGE.into(),
            extensions: &["rb"],
            symbol_query: r#"
                [
                  (method name: (identifier) @symbol)
                  (singleton_method name: (identifier) @symbol)
                  (class name: (constant) @symbol)
                  (module name: (constant) @symbol)
                ]
            "#,
            name: "Ruby",
        },
    ]
});

fn lang_for_file(path: &Path) -> Option<&'static LangConfig> {
    let ext = path.extension()?.to_str()?;
    LANGUAGES.iter().find(|lang| lang.extensions.contains(&ext))
}

fn extract_node_line(node: &Node, source: &[u8]) -> String {
    let start = node.start_position().row;
    let end = node.end_position().row;
    let text = String::from_utf8_lossy(source);

    let lines: Vec<&str> = text.lines().collect();
    if start < lines.len() {
        let line = lines[start].trim();
        if end > start && line.ends_with('{') {
            return line.trim_end_matches('{').trim_end().to_string();
        }
        if line.len() > 120 {
            return format!("{}...", &line[..117]);
        }
        return line.to_string();
    }
    String::new()
}

struct FileSymbols {
    path_relative: String,
    symbols: Vec<String>,
}

fn parse_file(path: &Path, relative: &Path) -> Option<FileSymbols> {
    let lang_config = lang_for_file(path)?;
    let source = std::fs::read_to_string(path).ok()?;

    if source.len() > 500_000 {
        return None;
    }

    let mut parser = Parser::new();
    parser.set_language(&lang_config.language).ok()?;

    let tree = parser.parse(&source, None)?;
    let root = tree.root_node();
    let source_bytes = source.as_bytes();

    let query = Query::new(&lang_config.language, lang_config.symbol_query).ok()?;
    let mut cursor = QueryCursor::new();

    let mut symbols: Vec<String> = Vec::new();
    let mut matches = cursor.matches(&query, root, source_bytes);

    while let Some(m) = matches.next() {
        for capture in m.captures {
            let node = capture.node;
            let kind = node.kind();
            let name = node.utf8_text(source_bytes).unwrap_or("?");
            let line = extract_node_line(&node, source_bytes);
            symbols.push(format!("{}:{} // {}", kind, name, line));
        }
    }

    if symbols.is_empty() {
        return None;
    }

    Some(FileSymbols {
        path_relative: relative.to_string_lossy().to_string(),
        symbols,
    })
}

const SKIP_DIRS: &[&str] = &[
    "node_modules",
    ".git",
    "target",
    "dist",
    "build",
    "__pycache__",
    ".next",
    "vendor",
    ".cargo",
    "debug",
    "release",
    ".tox",
    ".venv",
    "venv",
    "env",
    ".env",
    ".idea",
    ".vscode",
    "bazel-out",
    "bazel-bin",
];

const SKIP_EXTENSIONS: &[&str] = &[
    "min.js", "min.css", "bundle.js", "bundle.css", "generated", "pb.go",
];

fn should_skip(path: &Path) -> bool {
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.starts_with('.') && name != ".rs" {
            return true;
        }
        if SKIP_DIRS.contains(&name) {
            return true;
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if ext == "lock" || ext == "map" {
                return true;
            }
            let full = name.to_lowercase();
            for skip_ext in SKIP_EXTENSIONS {
                if full.ends_with(skip_ext) {
                    return true;
                }
            }
        }
    }
    false
}

fn walk_source_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if should_skip(&path) {
                    continue;
                }
                if path.is_dir() {
                    stack.push(path);
                } else if lang_for_file(&path).is_some() {
                    files.push(path);
                }
            }
        }
    }

    files.sort();
    files
}

pub fn generate_repo_map(working_dir: &Path) -> String {
    let cache_key = working_dir.to_string_lossy().to_string();
    if let Some(cached) = load_cache(&cache_key) {
        return cached;
    }

    let files = walk_source_files(working_dir);
    let mut sections: Vec<String> = Vec::new();
    let mut total_symbols = 0;
    const MAX_SYMBOLS: usize = 500;

    for file_path in &files {
        let relative = file_path.strip_prefix(working_dir).unwrap_or(file_path);
        if let Some(file_symbols) = parse_file(file_path, relative) {
            let mut section = format!("{}\n", file_symbols.path_relative);
            for sym in &file_symbols.symbols {
                section.push_str(&format!("  {}\n", sym));
                total_symbols += 1;
                if total_symbols >= MAX_SYMBOLS {
                    section.push_str("  ... (truncated)\n");
                    sections.push(section);
                    let result = sections.join("\n");
                    save_cache(&cache_key, &result);
                    return result;
                }
            }
            sections.push(section);
        }
    }

    let result = sections.join("\n");
    save_cache(&cache_key, &result);
    result
}

fn cache_dir() -> Option<PathBuf> {
    dirs::cache_dir().map(|d| d.join("seekr").join("repo_map"))
}

fn cache_path(key: &str) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    let hash = hasher.finish();
    cache_dir().map(|d| d.join(format!("{:016x}.map", hash)))
}

fn load_cache(key: &str) -> Option<String> {
    let path = cache_path(key)?;
    let metadata = std::fs::metadata(&path).ok()?;
    let age = metadata.modified().ok()?.elapsed().ok()?;
    if age.as_secs() > 300 {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

fn save_cache(key: &str, content: &str) {
    if let Some(path) = cache_path(key) {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, content);
    }
}

pub fn invalidate_cache(working_dir: &Path) {
    let key = working_dir.to_string_lossy().to_string();
    if let Some(path) = cache_path(&key) {
        let _ = std::fs::remove_file(path);
    }
}

pub fn get_language_stats(working_dir: &Path) -> HashMap<String, usize> {
    let files = walk_source_files(working_dir);
    let mut stats: HashMap<String, usize> = HashMap::new();

    for file_path in &files {
        if let Some(lang) = lang_for_file(file_path) {
            *stats.entry(lang.name.to_string()).or_insert(0) += 1;
        }
    }

    let mut stats_vec: Vec<_> = stats.into_iter().collect();
    stats_vec.sort_by_key(|b| std::cmp::Reverse(b.1));
    stats_vec.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lang_detection() {
        let rust_file = Path::new("src/main.rs");
        assert!(lang_for_file(rust_file).is_some());

        let py_file = Path::new("app.py");
        assert!(lang_for_file(py_file).is_some());

        let txt_file = Path::new("readme.txt");
        assert!(lang_for_file(txt_file).is_none());
    }

    #[test]
    fn test_skip_dirs() {
        assert!(should_skip(Path::new("node_modules")));
        assert!(should_skip(Path::new("target")));
        assert!(should_skip(Path::new(".git")));
        assert!(!should_skip(Path::new("src")));
    }
}
