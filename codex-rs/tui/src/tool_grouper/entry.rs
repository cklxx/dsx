//! Unified tool call entry used by the grouper.
//!
//! Every tool call — shell exec, MCP, web search, patch, hook — is converted
//! into a `ToolCallEntry` so the grouper can treat them uniformly.

use super::category::ToolCategory;
use super::safety::SafetySignals;
use std::time::Duration;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallStatus {
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone)]
pub struct ToolCallEntry {
    pub call_id: String,
    pub display_name: String,
    pub category: ToolCategory,
    pub input_preview: String,
    pub output_preview: String,
    pub output_lines: usize,
    pub output_truncated: bool,
    pub duration: Option<Duration>,
    pub exit_code: Option<i32>,
    pub status: CallStatus,
    pub started_at: Instant,
    /// Paths touched by this call (for safety detection).
    pub touched_paths: Vec<String>,
    /// Whether this call involved network activity.
    pub network_activity: bool,
}

impl ToolCallEntry {
    pub fn new_running(call_id: String, display_name: String, category: ToolCategory) -> Self {
        Self {
            call_id,
            display_name,
            category,
            input_preview: String::new(),
            output_preview: String::new(),
            output_lines: 0,
            output_truncated: false,
            duration: None,
            exit_code: None,
            status: CallStatus::Running,
            started_at: Instant::now(),
            touched_paths: Vec::new(),
            network_activity: false,
        }
    }

    pub fn complete(&mut self, output: String, exit_code: i32, duration: Duration) {
        let line_count = output.lines().count();
        let preview_lines: Vec<&str> = output.lines().take(3).collect();
        self.output_preview = preview_lines.join("\n");
        self.output_lines = line_count;
        self.output_truncated = line_count > 3;
        self.duration = Some(duration);
        self.exit_code = Some(exit_code);
        self.status = if exit_code == 0 {
            CallStatus::Completed
        } else {
            CallStatus::Failed
        };
    }

    pub fn fail(&mut self) {
        let elapsed = self.started_at.elapsed();
        self.duration = Some(elapsed);
        self.exit_code = Some(1);
        self.status = CallStatus::Failed;
    }

    pub fn is_running(&self) -> bool {
        matches!(self.status, CallStatus::Running)
    }

    pub fn is_write(&self) -> bool {
        self.category.is_write()
    }

    pub fn safety_contribution(&self) -> SafetySignals {
        use ToolCategory::*;
        let mut signals = SafetySignals::default();

        match self.category {
            FileRead | FileSearch | FileList => signals.read_count = 1,
            FileWrite => signals.write_count = 1,
            WebSearch => {
                signals.read_count = 1;
                signals.network_count = 1;
            }
            ShellExec => signals.shell_exec_count = 1,
            McpTool(_) => signals.read_count = 1,
            Plan | Hook | Other => {}
        }

        if matches!(self.status, CallStatus::Failed) {
            signals.fail_count = 1;
        }

        if self.network_activity {
            signals.network_count = signals.network_count.max(1);
        }

        signals
    }
}
