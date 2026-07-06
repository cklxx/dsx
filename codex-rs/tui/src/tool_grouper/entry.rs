//! Unified tool call entry used by the grouper.

use super::category::ToolCategory;
use std::time::Duration;
use std::time::Instant;

/// A single tool call captured into a uniform representation suitable for grouping and rendering.
#[derive(Debug, Clone)]
pub(crate) struct ToolCallEntry {
    /// Stable call identifier from the protocol layer.
    pub(crate) call_id: String,
    /// Human-readable display label (e.g. "rg foo", "MCP server/tool", "PostToolUse hook").
    pub(crate) display_name: String,
    /// Parsed category used for grouping decisions.
    pub(crate) category: ToolCategory,
    /// Optional raw command text (shell exec only).
    pub(crate) command_text: Option<String>,
    /// Optional MCP server name.
    pub(crate) mcp_server: Option<String>,
    /// Optional MCP tool name.
    pub(crate) mcp_tool: Option<String>,
    /// Optional hook event name.
    pub(crate) hook_event: Option<String>,
    /// Start time of the call.
    pub(crate) start_time: Instant,
    /// Duration, filled in after completion.
    pub(crate) duration: Option<Duration>,
    /// Exit code or success flag after completion.
    pub(crate) exit_code: Option<i32>,
    /// True if the call failed (non-zero exit or error status).
    pub(crate) failed: bool,
    /// True if this call involves network access.
    pub(crate) is_network: bool,
    /// Aggregated output text (truncated for display).
    pub(crate) output_preview: Option<String>,
    /// Paths referenced by the call (for protected-path detection and semantic titles).
    pub(crate) referenced_paths: Vec<String>,
    /// Search terms extracted (for semantic titles).
    pub(crate) search_terms: Vec<String>,
}

impl ToolCallEntry {
    pub(crate) fn new_shell(
        call_id: String,
        display_name: String,
        command_text: String,
        category: ToolCategory,
    ) -> Self {
        let is_network = matches!(category, ToolCategory::WebSearch);
        Self {
            call_id,
            display_name,
            category,
            command_text: Some(command_text),
            mcp_server: None,
            mcp_tool: None,
            hook_event: None,
            start_time: Instant::now(),
            duration: None,
            exit_code: None,
            failed: false,
            is_network,
            output_preview: None,
            referenced_paths: Vec::new(),
            search_terms: Vec::new(),
        }
    }

    pub(crate) fn new_mcp(call_id: String, server: String, tool: String) -> Self {
        let display_name = format!("{server}/{tool}");
        Self {
            call_id,
            display_name,
            category: ToolCategory::McpTool {
                server: server.clone(),
            },
            command_text: None,
            mcp_server: Some(server),
            mcp_tool: Some(tool),
            hook_event: None,
            start_time: Instant::now(),
            duration: None,
            exit_code: None,
            failed: false,
            is_network: false,
            output_preview: None,
            referenced_paths: Vec::new(),
            search_terms: Vec::new(),
        }
    }

    pub(crate) fn new_hook(call_id: String, event_name: String) -> Self {
        let display_name = format!("{event_name} hook");
        Self {
            call_id,
            display_name,
            category: ToolCategory::Hook,
            command_text: None,
            mcp_server: None,
            mcp_tool: None,
            hook_event: Some(event_name),
            start_time: Instant::now(),
            duration: None,
            exit_code: None,
            failed: false,
            is_network: false,
            output_preview: None,
            referenced_paths: Vec::new(),
            search_terms: Vec::new(),
        }
    }

    pub(crate) fn new_web(call_id: String, query: String) -> Self {
        let display_name = format!("web: {query}");
        Self {
            call_id,
            display_name,
            category: ToolCategory::WebSearch,
            command_text: None,
            mcp_server: None,
            mcp_tool: None,
            hook_event: None,
            start_time: Instant::now(),
            duration: None,
            exit_code: None,
            failed: false,
            is_network: true,
            output_preview: None,
            referenced_paths: Vec::new(),
            search_terms: vec![query],
        }
    }

    pub(crate) fn new_patch(call_id: String, file_count: usize) -> Self {
        let display_name = format!("patch ({file_count} files)");
        Self {
            call_id,
            display_name,
            category: ToolCategory::FileWrite,
            command_text: None,
            mcp_server: None,
            mcp_tool: None,
            hook_event: None,
            start_time: Instant::now(),
            duration: None,
            exit_code: None,
            failed: false,
            is_network: false,
            output_preview: None,
            referenced_paths: Vec::new(),
            search_terms: Vec::new(),
        }
    }

    /// Mark this call as completed with the given duration and exit code.
    pub(crate) fn complete(&mut self, duration: Duration, exit_code: i32) {
        self.duration = Some(duration);
        self.exit_code = Some(exit_code);
        self.failed = exit_code != 0;
    }

    /// Returns true if this call is still running.
    pub(crate) fn is_running(&self) -> bool {
        self.duration.is_none() && self.exit_code.is_none()
    }

    /// Extract a common path prefix from referenced paths.
    pub(crate) fn common_path(&self) -> Option<&str> {
        self.referenced_paths.first().map(|s| s.as_str())
    }
}
