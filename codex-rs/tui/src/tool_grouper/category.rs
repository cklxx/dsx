//! Tool category inference and compatibility rules for grouping.

use codex_protocol::parse_command::ParsedCommand;
use std::fmt;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum ToolCategory {
    FileRead,
    FileSearch,
    FileList,
    FileWrite,
    ShellExec,
    WebSearch,
    McpTool(String),
    Plan,
    Hook,
    Other,
}

impl ToolCategory {
    pub fn is_file_exploration(&self) -> bool {
        matches!(
            self,
            ToolCategory::FileRead | ToolCategory::FileSearch | ToolCategory::FileList
        )
    }

    pub fn is_write(&self) -> bool {
        matches!(self, ToolCategory::FileWrite)
    }

    pub fn display_icon(&self) -> &'static str {
        match self {
            ToolCategory::FileRead => "📖",
            ToolCategory::FileSearch => "🔍",
            ToolCategory::FileList => "📋",
            ToolCategory::FileWrite => "📝",
            ToolCategory::ShellExec => "⚙️",
            ToolCategory::WebSearch => "🌐",
            ToolCategory::McpTool(_) => "🔧",
            ToolCategory::Plan => "📋",
            ToolCategory::Hook => "🪝",
            ToolCategory::Other => "🔹",
        }
    }

    pub fn display_name(&self) -> String {
        match self {
            ToolCategory::FileRead => "read".into(),
            ToolCategory::FileSearch => "search".into(),
            ToolCategory::FileList => "list".into(),
            ToolCategory::FileWrite => "write".into(),
            ToolCategory::ShellExec => "exec".into(),
            ToolCategory::WebSearch => "web".into(),
            ToolCategory::McpTool(s) => format!("mcp:{s}"),
            ToolCategory::Plan => "plan".into(),
            ToolCategory::Hook => "hook".into(),
            ToolCategory::Other => "other".into(),
        }
    }

    /// Returns true if a call of `other` category can be appended to a group
    /// whose dominant category is `self`.
    pub fn compatible_with(&self, other: &ToolCategory) -> bool {
        use ToolCategory::*;
        match (self, other) {
            // File exploration categories are mutually compatible.
            (FileRead, FileSearch) | (FileRead, FileList) => true,
            (FileSearch, FileRead) | (FileSearch, FileList) => true,
            (FileList, FileRead) | (FileList, FileSearch) => true,
            // Same-category is always compatible.
            (a, b) if a == b => true,
            // McpTool: only compatible with same server.
            (McpTool(a), McpTool(b)) => a == b,
            // FileWrite is never compatible with reads.
            (FileWrite, _) | (_, FileWrite) if self != other => false,
            // Plan and Hook only group with themselves (handled by a==b).
            _ => false,
        }
    }
}

impl fmt::Display for ToolCategory {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.display_name())
    }
}

/// Infer tool category from a shell command name and its arguments.
pub fn infer_from_shell(cmd_name: &str, args: &[String], parsed: &[ParsedCommand]) -> ToolCategory {
    let full_args = args.join(" ");
    let has_redirect = full_args.contains('>') || full_args.contains("tee");

    // Check parsed commands first — they carry semantic info.
    let all_parsed_are_read = !parsed.is_empty()
        && parsed.iter().all(|p| {
            matches!(
                p,
                ParsedCommand::Read { .. }
                    | ParsedCommand::ListFiles { .. }
                    | ParsedCommand::Search { .. }
            )
        });
    if all_parsed_are_read {
        let has_search = parsed
            .iter()
            .any(|p| matches!(p, ParsedCommand::Search { .. }));
        let has_list = parsed
            .iter()
            .any(|p| matches!(p, ParsedCommand::ListFiles { .. }));
        if has_search {
            return ToolCategory::FileSearch;
        }
        if has_list {
            return ToolCategory::FileList;
        }
        return ToolCategory::FileRead;
    }

    // Fallback: command-name heuristics.
    let base = cmd_name.split('/').last().unwrap_or(cmd_name);
    match base {
        "cat" | "bat" | "head" | "tail" | "less" | "more" | "nl" | "od" | "xxd" | "hexdump" => {
            if has_redirect {
                ToolCategory::FileWrite
            } else {
                ToolCategory::FileRead
            }
        }
        "rg" | "grep" | "egrep" | "fgrep" | "ag" | "ack" | "pt" | "sift" => {
            ToolCategory::FileSearch
        }
        "find" | "fd" | "fdfind" | "locate" | "which" | "whereis" => ToolCategory::FileSearch,
        "ls" | "ll" | "la" | "dir" | "tree" | "exa" | "eza" | "lsd" => ToolCategory::FileList,
        "stat" | "file" | "wc" | "du" | "df" => ToolCategory::FileRead,
        "apply_patch" | "edit" | "write" | "sed" if has_redirect => ToolCategory::FileWrite,
        "mv" | "cp" | "rm" | "rmdir" | "mkdir" | "touch" | "chmod" | "chown" | "ln" => {
            ToolCategory::FileWrite
        }
        "cargo" | "rustc" | "npm" | "pnpm" | "yarn" | "make" | "cmake" | "bazel" | "just"
        | "pip" | "pip3" | "uv" | "poetry" | "go" | "gcc" | "clang" | "python" | "python3"
        | "node" | "bun" | "deno" | "ruby" | "java" | "mvn" | "gradle" | "dotnet" => {
            ToolCategory::ShellExec
        }
        "curl" | "wget" | "httpie" | "http" if looks_like_url(&full_args) => {
            ToolCategory::WebSearch
        }
        _ => ToolCategory::Other,
    }
}

/// Infer from a non-shell tool name (MCP, built-in, etc.) and its arguments JSON.
pub fn infer_from_tool_name(tool_name: &str, args: Option<&serde_json::Value>) -> ToolCategory {
    let lower = tool_name.to_lowercase();
    let args_str = args.map(|a| a.to_string()).unwrap_or_default();

    if lower.starts_with("mcp:") || lower.starts_with("mcp_") {
        let server = extract_mcp_server(&lower);
        return ToolCategory::McpTool(server);
    }

    match lower.as_str() {
        "web_search" | "search_web" | "duckduckgo_search" | "tavily_search" => {
            ToolCategory::WebSearch
        }
        "read_url" | "fetch_url" | "get_url" | "web_fetch" => ToolCategory::WebSearch,
        "apply_patch" | "edit_file" | "write_file" | "create_file" => ToolCategory::FileWrite,
        "read_file" | "view_file" | "cat_file" => ToolCategory::FileRead,
        "search_files" | "grep_files" | "find_files" => ToolCategory::FileSearch,
        "list_files" | "list_directory" | "read_directory" | "ls" => ToolCategory::FileList,
        "update_plan" | "plan" => ToolCategory::Plan,
        "run_hook" | "fire_hook" | "execute_hook" => ToolCategory::Hook,
        _ => {
            if looks_like_url(&args_str) {
                ToolCategory::WebSearch
            } else {
                ToolCategory::Other
            }
        }
    }
}

fn extract_mcp_server(name: &str) -> String {
    // "mcp:server_name:tool" or "mcp_server_name_tool"
    if let Some(rest) = name.strip_prefix("mcp:") {
        rest.split(':').next().unwrap_or("unknown").to_string()
    } else if let Some(rest) = name.strip_prefix("mcp_") {
        rest.split('_').next().unwrap_or("unknown").to_string()
    } else {
        "unknown".to_string()
    }
}

fn looks_like_url(s: &str) -> bool {
    s.contains("http://") || s.contains("https://") || s.contains("ftp://")
}

/// Pick the dominant category from a list of calls.
/// File exploration categories are merged into the most-specific one.
pub fn dominant_category(categories: &[ToolCategory]) -> Option<ToolCategory> {
    if categories.is_empty() {
        return None;
    }

    let mut counts = std::collections::HashMap::new();
    for cat in categories {
        *counts.entry(cat.clone()).or_insert(0u32) += 1;
    }

    // If all are file exploration, pick the most frequent specific one.
    let all_exploration = categories.iter().all(|c| c.is_file_exploration());
    if all_exploration {
        return counts
            .into_iter()
            .max_by_key(|(_, count)| *count)
            .map(|(cat, _)| cat);
    }

    // Otherwise, pick the most frequent, but writes always dominate if present.
    if let Some(write_count) = counts.get(&ToolCategory::FileWrite) {
        if *write_count > 0 {
            return Some(ToolCategory::FileWrite);
        }
    }

    counts
        .into_iter()
        .max_by_key(|(_, count)| *count)
        .map(|(cat, _)| cat)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_exploration_compatible() {
        assert!(ToolCategory::FileRead.compatible_with(&ToolCategory::FileSearch));
        assert!(ToolCategory::FileSearch.compatible_with(&ToolCategory::FileList));
        assert!(ToolCategory::FileList.compatible_with(&ToolCategory::FileRead));
    }

    #[test]
    fn write_not_compatible_with_read() {
        assert!(!ToolCategory::FileWrite.compatible_with(&ToolCategory::FileRead));
        assert!(!ToolCategory::FileRead.compatible_with(&ToolCategory::FileWrite));
    }

    #[test]
    fn mcp_same_server_compatible() {
        assert!(
            ToolCategory::McpTool("openmax".into())
                .compatible_with(&ToolCategory::McpTool("openmax".into()))
        );
        assert!(
            !ToolCategory::McpTool("openmax".into())
                .compatible_with(&ToolCategory::McpTool("browser".into()))
        );
    }

    #[test]
    fn infer_rg_is_search() {
        let cat = infer_from_shell("rg", &["fn.*model".into(), "src/".into()], &[]);
        assert_eq!(cat, ToolCategory::FileSearch);
    }

    #[test]
    fn infer_cat_with_redirect_is_write() {
        let cat = infer_from_shell(
            "cat",
            &["file.txt".into(), ">".into(), "out.txt".into()],
            &[],
        );
        assert_eq!(cat, ToolCategory::FileWrite);
    }

    #[test]
    fn infer_cargo_is_exec() {
        let cat = infer_from_shell("cargo", &["build".into()], &[]);
        assert_eq!(cat, ToolCategory::ShellExec);
    }

    #[test]
    fn infer_mcp_tool() {
        let cat = infer_from_tool_name("mcp:openmax:list_files", None);
        assert_eq!(cat, ToolCategory::McpTool("openmax".into()));
    }

    #[test]
    fn dominant_picks_write() {
        let cats = vec![
            ToolCategory::FileRead,
            ToolCategory::FileRead,
            ToolCategory::FileWrite,
        ];
        assert_eq!(dominant_category(&cats), Some(ToolCategory::FileWrite));
    }
}
