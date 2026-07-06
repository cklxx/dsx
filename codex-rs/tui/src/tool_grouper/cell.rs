//! Grouped tool call history cell.

use super::category::ToolCategory;
use super::entry::ToolCallEntry;
use super::render;
use super::safety::SafetySignals;
use crate::history_cell::HistoryCell;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use std::time::Duration;
use std::time::Instant;

/// Maximum number of calls in a single group before auto-flush.
pub(crate) const MAX_CALLS_PER_GROUP: usize = 12;

/// A grouped tool call cell that can be collapsed or expanded.
///
/// When it contains only 1 call, it renders as a normal single call (no collapse chrome).
/// When it contains ≥2 calls, it renders as a collapsible group with safety signals.
#[derive(Debug)]
pub(crate) struct GroupedToolCallCell {
    /// The calls in this group.
    entries: Vec<ToolCallEntry>,
    /// Dominant category for display and compatibility checks.
    dominant_category: ToolCategory,
    /// Whether the group is currently expanded in the viewport.
    expanded: bool,
    /// Whether animations are enabled.
    animations_enabled: bool,
    /// Cached safety signals, recomputed when entries change.
    safety_signals: SafetySignals,
    /// Start time of the first call in the group (for total duration display).
    first_start: Instant,
}

impl GroupedToolCallCell {
    pub(crate) fn new(entry: ToolCallEntry, animations_enabled: bool) -> Self {
        let first_start = entry.start_time;
        let dominant_category = entry.category.clone();
        let mut cell = Self {
            entries: Vec::new(),
            dominant_category,
            expanded: false,
            animations_enabled,
            safety_signals: SafetySignals::default(),
            first_start,
        };
        cell.add_entry(entry);
        cell
    }

    /// Returns whether animations are enabled for this cell.
    pub(crate) fn animations_enabled(&self) -> bool {
        self.animations_enabled
    }

    /// Add an entry to the group. Returns true if successfully added.
    pub(crate) fn add_entry(&mut self, entry: ToolCallEntry) -> bool {
        if self.entries.len() >= MAX_CALLS_PER_GROUP {
            return false;
        }
        if !self.dominant_category.can_append(&entry.category) {
            return false;
        }
        self.entries.push(entry);
        self.recompute_safety();
        true
    }

    /// Try to complete a call by its id. Returns true if found.
    pub(crate) fn complete_call(
        &mut self,
        call_id: &str,
        duration: Duration,
        exit_code: i32,
    ) -> bool {
        let found = self.entries.iter_mut().rev().find(|e| e.call_id == call_id);
        if let Some(entry) = found {
            entry.complete(duration, exit_code);
            self.recompute_safety();
            true
        } else {
            false
        }
    }

    /// Append output to a running call by id.
    pub(crate) fn append_output(&mut self, call_id: &str, chunk: &str) -> bool {
        if chunk.is_empty() {
            return false;
        }
        let entry = self
            .entries
            .iter_mut()
            .rev()
            .find(|e| e.call_id == call_id && e.is_running());
        if let Some(entry) = entry {
            let preview = entry.output_preview.get_or_insert_with(String::new);
            if preview.len() < 2000 {
                preview.push_str(chunk);
            }
            true
        } else {
            false
        }
    }

    /// Returns true if the group contains a call with the given id.
    pub(crate) fn contains_call(&self, call_id: &str) -> bool {
        self.entries.iter().any(|e| e.call_id == call_id)
    }

    /// Returns true if all calls in the group are completed.
    pub(crate) fn all_completed(&self) -> bool {
        !self.entries.is_empty() && self.entries.iter().all(|e| !e.is_running())
    }

    /// Returns true if the group has any running calls.
    pub(crate) fn is_active(&self) -> bool {
        self.entries.iter().any(|e| e.is_running())
    }

    /// Returns true if the group should be flushed to history.
    pub(crate) fn should_flush(&self) -> bool {
        self.all_completed()
    }

    /// Number of calls in the group.
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Toggle expanded/collapsed state.
    pub(crate) fn toggle_expanded(&mut self) {
        self.expanded = !self.expanded;
    }

    pub(crate) fn set_expanded(&mut self, expanded: bool) {
        self.expanded = expanded;
    }

    pub(crate) fn is_expanded(&self) -> bool {
        self.expanded
    }

    pub(crate) fn entries(&self) -> &[ToolCallEntry] {
        &self.entries
    }

    pub(crate) fn dominant_category(&self) -> &ToolCategory {
        &self.dominant_category
    }

    pub(crate) fn safety_signals(&self) -> &SafetySignals {
        &self.safety_signals
    }

    /// Total elapsed time from first call start to last call completion (or now if still running).
    pub(crate) fn total_duration(&self) -> Duration {
        if self.entries.is_empty() {
            return Duration::ZERO;
        }
        let end = self
            .entries
            .iter()
            .filter_map(|e| e.duration.map(|d| e.start_time + d))
            .max()
            .unwrap_or_else(Instant::now);
        end.saturating_duration_since(self.first_start)
    }

    fn recompute_safety(&mut self) {
        self.safety_signals = SafetySignals::from_entries(&self.entries);
    }

    /// Returns true if this group renders as a single (non-grouped) call.
    fn is_single_call(&self) -> bool {
        self.entries.len() <= 1
    }

    /// Generate a semantic title for the group.
    fn semantic_title(&self) -> String {
        render::semantic_title(&self.entries, &self.dominant_category)
    }
}

impl HistoryCell for GroupedToolCallCell {
    fn display_lines(&self, width: u16) -> Vec<Line<'static>> {
        if self.is_single_call() {
            render::render_single_call(self, width)
        } else if self.expanded {
            render::render_expanded(self, width)
        } else {
            render::render_collapsed(self, width)
        }
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        let mut lines = Vec::new();
        for entry in &self.entries {
            let status = if entry.failed { "✗" } else { "✓" };
            lines.push(Line::from(vec![
                Span::from(format!("  {status} ")),
                Span::from(entry.display_name.clone()),
            ]));
            if let Some(preview) = &entry.output_preview {
                for out_line in preview.lines().take(5) {
                    lines.push(Line::from(format!("    {out_line}")));
                }
            }
        }
        lines
    }

    fn transcript_lines(&self, _width: u16) -> Vec<Line<'static>> {
        self.raw_lines()
    }

    fn desired_height(&self, width: u16) -> u16 {
        Paragraph::new(Text::from(self.display_lines(width)))
            .wrap(Wrap { trim: false })
            .line_count(width)
            .try_into()
            .unwrap_or(0)
    }

    fn transcript_animation_tick(&self) -> Option<u64> {
        if self.is_active() && self.animations_enabled {
            Some(self.first_start.elapsed().as_millis() as u64 / 100)
        } else {
            None
        }
    }

    fn is_stream_continuation(&self) -> bool {
        self.is_active()
    }
}
