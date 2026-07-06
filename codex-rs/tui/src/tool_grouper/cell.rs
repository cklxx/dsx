//! GroupedToolCallCell — a HistoryCell that renders multiple adjacent tool calls
//! as a single collapsible unit.

use super::category::ToolCategory;
use super::entry::ToolCallEntry;
use super::render;
use super::safety::SafetySignals;
use crate::history_cell::HistoryCell;
use crate::history_cell::plain_lines;
use crate::terminal_hyperlinks::HyperlinkLine;
use crate::terminal_hyperlinks::plain_hyperlink_lines;
use ratatui::prelude::*;
use std::any::Any;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct GroupedToolCallCell {
    pub calls: Vec<ToolCallEntry>,
    pub dominant_category: ToolCategory,
    pub expanded: bool,
    pub title: String,
    pub safety_signals: SafetySignals,
    pub total_duration: Option<Duration>,
}

impl GroupedToolCallCell {
    pub fn new(calls: Vec<ToolCallEntry>, dominant_category: ToolCategory) -> Self {
        let title = render::generate_title(&calls, &dominant_category);
        let safety_signals = render::aggregate_safety(&calls);
        let total_duration = render::total_duration(&calls);
        Self {
            calls,
            dominant_category,
            expanded: false,
            title,
            safety_signals,
            total_duration,
        }
    }

    /// Recompute cached fields after calls change.
    pub fn recompute(&mut self) {
        self.title = render::generate_title(&self.calls, &self.dominant_category);
        self.safety_signals = render::aggregate_safety(&self.calls);
        self.total_duration = render::total_duration(&self.calls);
    }

    /// Add a call to this group. Returns true if the call was compatible and added.
    pub fn try_add_call(&mut self, call: ToolCallEntry) -> bool {
        if !self.dominant_category.compatible_with(&call.category) {
            return false;
        }
        let is_write = call.category.is_write();
        self.calls.push(call);
        // Re-evaluate dominant category (write takes precedence).
        if is_write && !self.dominant_category.is_write() {
            self.dominant_category = ToolCategory::FileWrite;
        }
        self.recompute();
        true
    }

    /// Mark a call as completed by its call_id. Returns true if found.
    pub fn complete_call(
        &mut self,
        call_id: &str,
        output: String,
        exit_code: i32,
        duration: Duration,
    ) -> bool {
        if let Some(call) = self.calls.iter_mut().find(|c| c.call_id == call_id) {
            call.complete(output, exit_code, duration);
            self.recompute();
            true
        } else {
            false
        }
    }

    /// Append output to a running call. Returns true if found.
    pub fn append_output(&mut self, call_id: &str, delta: &str) -> bool {
        if let Some(call) = self
            .calls
            .iter_mut()
            .find(|c| c.call_id == call_id && c.is_running())
        {
            let output = call.output_preview.clone();
            call.output_preview = format!("{output}{delta}");
            call.output_lines = call.output_preview.lines().count();
            true
        } else {
            false
        }
    }

    /// Mark all running calls as failed.
    pub fn mark_all_failed(&mut self) {
        for call in self.calls.iter_mut() {
            if call.is_running() {
                call.fail();
            }
        }
        self.recompute();
    }

    pub fn is_active(&self) -> bool {
        self.calls.iter().any(|c| c.is_running())
    }

    pub fn call_count(&self) -> usize {
        self.calls.len()
    }

    pub fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    pub fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    /// Returns true if this group has only one call (should render as single call).
    pub fn is_single_call(&self) -> bool {
        self.calls.len() == 1
    }
}

impl HistoryCell for GroupedToolCallCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.is_single_call() {
            // Single call: render with full detail (no collapsing).
            return render::render_expanded(
                &self.title,
                &self.dominant_category,
                &self.safety_signals,
                self.total_duration,
                &self.calls,
                width,
            );
        }

        if self.expanded {
            render::render_expanded(
                &self.title,
                &self.dominant_category,
                &self.safety_signals,
                self.total_duration,
                &self.calls,
                width,
            )
        } else {
            render::render_collapsed_header(
                &self.title,
                &self.dominant_category,
                &self.safety_signals,
                self.total_duration,
                &self.calls,
                width,
            )
        }
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        // Raw mode: always show all calls with full output.
        for (idx, call) in self.calls.iter().enumerate() {
            let header = format!(
                "{}. {} {} {}",
                idx + 1,
                call.display_name,
                call.input_preview,
                call.duration
                    .map(|d| format!("({})", codex_utils_elapsed::format_duration(d)))
                    .unwrap_or_default()
            );
            lines.push(Line::from(header));
            if !call.output_preview.is_empty() {
                for output_line in call.output_preview.lines() {
                    lines.push(Line::from(format!("  {output_line}")));
                }
            }
        }
        plain_lines(lines)
    }

    fn display_hyperlink_lines(&self, width: u16) -> Vec<HyperlinkLine> {
        plain_hyperlink_lines(self.display_lines(width))
    }

    fn transcript_lines(&self, width: u16) -> Vec<Line<'static>> {
        // Transcript always shows expanded view.
        render::render_expanded(
            &self.title,
            &self.dominant_category,
            &self.safety_signals,
            self.total_duration,
            &self.calls,
            width,
        )
    }
}
