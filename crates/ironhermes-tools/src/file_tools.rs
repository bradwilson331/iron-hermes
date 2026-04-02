use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

use async_trait::async_trait;
use glob::glob;
use ironhermes_core::ToolSchema;
use regex::Regex;
use serde_json::json;
use tracing::debug;

use crate::registry::Tool;

// =============================================================================
// ReadFileTool
// =============================================================================

pub struct ReadFileTool;

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn toolset(&self) -> &str {
        "file"
    }

    fn description(&self) -> &str {
        "Read the contents of a file, returned with line numbers."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "read_file",
            "Read the contents of a file, returned with line numbers.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Absolute or relative path to the file to read."
                    }
                },
                "required": ["path"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: path"))?;

        debug!("Reading file: {}", path);

        let file = fs::File::open(path)
            .map_err(|e| anyhow::anyhow!("Failed to open '{}': {}", path, e))?;

        let reader = BufReader::new(file);
        let mut output = String::new();
        for (i, line) in reader.lines().enumerate() {
            let line = line.map_err(|e| anyhow::anyhow!("Failed to read line: {}", e))?;
            output.push_str(&format!("{:>6}\t{}\n", i + 1, line));
        }

        if output.is_empty() {
            output = "(empty file)".to_string();
        }

        Ok(output)
    }
}

// =============================================================================
// WriteFileTool
// =============================================================================

pub struct WriteFileTool;

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn toolset(&self) -> &str {
        "file"
    }

    fn description(&self) -> &str {
        "Write content to a file, creating it or overwriting it if it already exists."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "write_file",
            "Write content to a file, creating it or overwriting it if it already exists.",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to write."
                    },
                    "content": {
                        "type": "string",
                        "description": "Content to write to the file."
                    }
                },
                "required": ["path", "content"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: path"))?;
        let content = args["content"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: content"))?;

        debug!("Writing file: {}", path);

        if let Some(parent) = Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| anyhow::anyhow!("Failed to create directories for '{}': {}", path, e))?;
            }
        }

        fs::write(path, content)
            .map_err(|e| anyhow::anyhow!("Failed to write '{}': {}", path, e))?;

        let bytes = content.len();
        let lines = content.lines().count();
        Ok(format!("Successfully wrote {} bytes ({} lines) to '{}'.", bytes, lines, path))
    }
}

// =============================================================================
// PatchFileTool
// =============================================================================

pub struct PatchFileTool;

#[async_trait]
impl Tool for PatchFileTool {
    fn name(&self) -> &str {
        "patch"
    }

    fn toolset(&self) -> &str {
        "file"
    }

    fn description(&self) -> &str {
        "Replace an exact string in a file with new content (first occurrence)."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "patch",
            "Replace an exact string in a file with new content (first occurrence).",
            json!({
                "type": "object",
                "properties": {
                    "path": {
                        "type": "string",
                        "description": "Path to the file to patch."
                    },
                    "before": {
                        "type": "string",
                        "description": "The exact string to search for and replace."
                    },
                    "after": {
                        "type": "string",
                        "description": "The replacement string."
                    }
                },
                "required": ["path", "before", "after"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: path"))?;
        let before = args["before"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: before"))?;
        let after = args["after"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: after"))?;

        debug!("Patching file: {}", path);

        let original = fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("Failed to read '{}': {}", path, e))?;

        if !original.contains(before) {
            return Err(anyhow::anyhow!(
                "String not found in '{}'. The 'before' text must match exactly.",
                path
            ));
        }

        let patched = original.replacen(before, after, 1);

        fs::write(path, &patched)
            .map_err(|e| anyhow::anyhow!("Failed to write '{}': {}", path, e))?;

        Ok(format!("Successfully patched '{}'.", path))
    }
}

// =============================================================================
// SearchFilesTool
// =============================================================================

pub struct SearchFilesTool;

#[async_trait]
impl Tool for SearchFilesTool {
    fn name(&self) -> &str {
        "search_files"
    }

    fn toolset(&self) -> &str {
        "file"
    }

    fn description(&self) -> &str {
        "Search for a regex pattern across files in a directory tree."
    }

    fn schema(&self) -> ToolSchema {
        ToolSchema::new(
            "search_files",
            "Search for a regex pattern across files in a directory tree.",
            json!({
                "type": "object",
                "properties": {
                    "query": {
                        "type": "string",
                        "description": "Regex pattern to search for."
                    },
                    "path": {
                        "type": "string",
                        "description": "Directory to search in (default: current directory)."
                    },
                    "file_pattern": {
                        "type": "string",
                        "description": "Glob pattern to filter files (e.g. '*.rs', '**/*.ts')."
                    }
                },
                "required": ["query"]
            }),
        )
    }

    async fn execute(&self, args: serde_json::Value) -> anyhow::Result<String> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("Missing required parameter: query"))?;
        let search_path = args["path"].as_str().unwrap_or(".");
        let file_pattern = args["file_pattern"].as_str().unwrap_or("**/*");

        debug!("Searching files in '{}' for pattern '{}'", search_path, query);

        let re = Regex::new(query)
            .map_err(|e| anyhow::anyhow!("Invalid regex pattern '{}': {}", query, e))?;

        let glob_pattern = if file_pattern.starts_with('/') || file_pattern.starts_with("**/") {
            file_pattern.to_string()
        } else {
            format!("{}/{}", search_path.trim_end_matches('/'), file_pattern)
        };

        let mut results = Vec::new();
        let mut files_searched = 0usize;

        for entry in glob(&glob_pattern)
            .map_err(|e| anyhow::anyhow!("Invalid glob pattern '{}': {}", glob_pattern, e))?
        {
            let path = match entry {
                Ok(p) => p,
                Err(_) => continue,
            };

            if !path.is_file() {
                continue;
            }

            files_searched += 1;

            let content = match fs::read_to_string(&path) {
                Ok(c) => c,
                Err(_) => continue, // skip binary / unreadable files
            };

            for (line_num, line) in content.lines().enumerate() {
                if re.is_match(line) {
                    results.push(format!(
                        "{}:{}: {}",
                        path.display(),
                        line_num + 1,
                        line.trim()
                    ));
                }

                if results.len() >= 500 {
                    break;
                }
            }

            if results.len() >= 500 {
                break;
            }
        }

        if results.is_empty() {
            Ok(format!(
                "No matches found for '{}' in {} file(s).",
                query, files_searched
            ))
        } else {
            let header = format!(
                "{} match(es) across {} file(s) searched:\n\n",
                results.len(),
                files_searched
            );
            Ok(format!("{}{}", header, results.join("\n")))
        }
    }
}
