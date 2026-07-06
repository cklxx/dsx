//! Tool call categorization for grouping decisions.

use codex_protocol::parse_command::ParsedCommand;

/// Broad category of a tool call, used to decide which calls can be grouped together.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ToolCategory {
    /// Read-only file exploration: cat, bat, head, tail, read_file
    FileRead,
    /// Search: rg, grep, find, fd, search_files
    FileSearch,
    /// Directory listing: ls, dir, list_files, tree
    FileList,
    /// Write operations: apply_patch, edit, write, mv, rm, cp, mkdir, >, >>
    FileWrite,
    /// Shell execution not matching the above: cargo, npm, make, python, etc.
    ShellExec { command_name: String },
    /// Web search or URL fetch: web_search, read_url, curl with URL
    WebSearch,
    /// MCP tool call from a specific server
    McpTool { server: String },
    /// Hook execution
    Hook,
    /// Plan tool
    Plan,
    /// Catch-all
    Other,
}

impl ToolCategory {
    /// Returns true if a new call of `other` category can be appended to a group
    /// that already has `self` as its dominant category.
    pub(crate) fn can_append(&self, other: &ToolCategory) -> bool {
        use ToolCategory::*;
        match (self, other) {
            (FileRead, FileRead) | (FileRead, FileSearch) | (FileRead, FileList) => true,
            (FileSearch, FileSearch) | (FileSearch, FileRead) | (FileSearch, FileList) => true,
            (FileList, FileList) | (FileList, FileRead) | (FileList, FileSearch) => true,
            (FileWrite, FileWrite) => true,
            (ShellExec { command_name: a }, ShellExec { command_name: b }) => a == b,
            (WebSearch, WebSearch) => true,
            (McpTool { server: a }, McpTool { server: b }) => a == b,
            (Hook, Hook) => true,
            (Plan, Plan) => true,
            (Other, Other) => true,
            _ => false,
        }
    }

    /// Infer category from a parsed shell command pipeline.
    pub(crate) fn from_parsed(parsed: &[ParsedCommand], raw_command: &[String]) -> Self {
        use ParsedCommand::*;

        let raw = raw_command.join(" ");
        if has_write_redirect(&raw) {
            return ToolCategory::FileWrite;
        }

        if parsed.is_empty() {
            return Self::from_raw_command(&raw);
        }

        let all_read_or_list_or_search = parsed
            .iter()
            .all(|p| matches!(p, Read { .. } | ListFiles { .. } | Search { .. }));

        if all_read_or_list_or_search {
            let has_search = parsed.iter().any(|p| matches!(p, Search { .. }));
            let has_read = parsed.iter().any(|p| matches!(p, Read { .. }));
            let has_list = parsed.iter().any(|p| matches!(p, ListFiles { .. }));

            if has_search {
                return ToolCategory::FileSearch;
            }
            if has_read {
                return ToolCategory::FileRead;
            }
            if has_list {
                return ToolCategory::FileList;
            }
        }

        if is_write_command(&raw) {
            return ToolCategory::FileWrite;
        }

        if is_web_command(&raw) {
            return ToolCategory::WebSearch;
        }

        let cmd_name = parsed
            .first()
            .map(|p| match p {
                Read { cmd, .. } | ListFiles { cmd, .. } | Search { cmd, .. } | Unknown { cmd } => {
                    cmd.clone()
                }
            })
            .unwrap_or_else(|| raw_command.first().cloned().unwrap_or_default());

        ToolCategory::ShellExec {
            command_name: cmd_name,
        }
    }

    fn from_raw_command(raw: &str) -> Self {
        if is_write_command(raw) {
            return ToolCategory::FileWrite;
        }
        if is_web_command(raw) {
            return ToolCategory::WebSearch;
        }
        let cmd_name = raw.split_whitespace().next().unwrap_or("").to_string();
        ToolCategory::ShellExec {
            command_name: cmd_name,
        }
    }

    /// Display label for the category, used in collapsed summary.
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ToolCategory::FileRead => "read",
            ToolCategory::FileSearch => "search",
            ToolCategory::FileList => "list",
            ToolCategory::FileWrite => "write",
            ToolCategory::ShellExec { .. } => "exec",
            ToolCategory::WebSearch => "web",
            ToolCategory::McpTool { .. } => "mcp",
            ToolCategory::Hook => "hook",
            ToolCategory::Plan => "plan",
            ToolCategory::Other => "tool",
        }
    }

    /// Emoji/glyph prefix for the category.
    pub(crate) fn icon(&self) -> &'static str {
        match self {
            ToolCategory::FileRead => "📖",
            ToolCategory::FileSearch => "🔍",
            ToolCategory::FileList => "📋",
            ToolCategory::FileWrite => "📝",
            ToolCategory::ShellExec { .. } => "⚡",
            ToolCategory::WebSearch => "🌐",
            ToolCategory::McpTool { .. } => "🔌",
            ToolCategory::Hook => "🪝",
            ToolCategory::Plan => "📐",
            ToolCategory::Other => "🔧",
        }
    }
}

fn has_write_redirect(raw: &str) -> bool {
    raw.contains('>') && !raw.contains("->") && !raw.contains("=>")
}

fn is_write_command(raw: &str) -> bool {
    let cmd = raw.split_whitespace().next().unwrap_or("");
    matches!(
        cmd,
        "apply_patch"
            | "edit"
            | "write"
            | "mv"
            | "rm"
            | "cp"
            | "mkdir"
            | "touch"
            | "chmod"
            | "chown"
            | "ln"
            | "unlink"
            | "rmdir"
    )
}

fn is_web_command(raw: &str) -> bool {
    if raw.contains("http://") || raw.contains("https://") {
        return true;
    }
    let cmd = raw.split_whitespace().next().unwrap_or("");
    matches!(cmd, "curl" | "wget" | "web_search" | "read_url")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_can_append_compatible() {
        assert!(ToolCategory::FileRead.can_append(&ToolCategory::FileSearch));
        assert!(ToolCategory::FileSearch.can_append(&ToolCategory::FileRead));
        assert!(ToolCategory::FileList.can_append(&ToolCategory::FileRead));
        assert!(ToolCategory::FileWrite.can_append(&ToolCategory::FileWrite));
        assert!(ToolCategory::Hook.can_append(&ToolCategory::Hook));
    }

    #[test]
    fn test_cannot_append_incompatible() {
        assert!(!ToolCategory::FileRead.can_append(&ToolCategory::FileWrite));
        assert!(!ToolCategory::FileWrite.can_append(&ToolCategory::FileRead));
        assert!(
            !ToolCategory::ShellExec {
                command_name: "cargo".into()
            }
            .can_append(&ToolCategory::ShellExec {
                command_name: "npm".into()
            })
        );
        assert!(!ToolCategory::Hook.can_append(&ToolCategory::FileRead));
    }

    #[test]
    fn test_mcp_same_server() {
        assert!(
            ToolCategory::McpTool { server: "a".into() }
                .can_append(&ToolCategory::McpTool { server: "a".into() })
        );
        assert!(
            !ToolCategory::McpTool { server: "a".into() }
                .can_append(&ToolCategory::McpTool { server: "b".into() })
        );
    }

    #[test]
    fn test_write_redirect_detected() {
        let cat =
            ToolCategory::from_parsed(&[], &["cat".into(), "foo".into(), ">".into(), "bar".into()]);
        assert_eq!(cat, ToolCategory::FileWrite);
    }
}
