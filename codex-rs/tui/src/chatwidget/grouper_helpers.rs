//! Tool call grouper integration for `ChatWidget`.
//!
//! The grouper intercepts tool-call lifecycle events and builds
//! `GroupedToolCallCell` entries directly from protocol data. Compatible
//! adjacent calls are merged; when a text break or incompatible call arrives,
//! the pending group is flushed as a single history cell.
//!
//! This operates purely at the display layer — approval, sandboxing, and
//! exec_policy are unaffected.

use super::*;
use crate::tool_grouper::GroupedToolCallCell;
use crate::tool_grouper::ToolCallEntry;
use crate::tool_grouper::ToolCategory;

impl ChatWidget {
    fn grouper_thread_key(&self) -> Option<String> {
        self.thread_id.map(|t| t.to_string())
    }

    // ── Shell exec ──────────────────────────────────────────────

    /// Try to route a command start through the grouper.
    /// Returns `true` if handled (caller should skip ExecCell creation).
    pub(super) fn grouper_try_handle_command_start(
        &mut self,
        call_id: &str,
        command: &[String],
        parsed: &[codex_protocol::parse_command::ParsedCommand],
    ) -> bool {
        if !self.tool_grouper.is_enabled() {
            return false;
        }

        let category = ToolCategory::from_parsed(parsed, command);
        // Only group read/search/list calls — write and shell-exec stay as
        // individual cells so their output is always visible.
        let is_groupable = matches!(
            category,
            ToolCategory::FileRead
                | ToolCategory::FileSearch
                | ToolCategory::FileList
                | ToolCategory::Hook
                | ToolCategory::WebSearch
        );
        if !is_groupable {
            // Flush any pending read group before showing a write/exec
            self.grouper_flush_pending();
            return false;
        }

        let display = command.join(" ");
        let mut entry =
            ToolCallEntry::new_shell(call_id.to_string(), display.clone(), display, category);

        // Extract paths & search terms from parsed
        for p in parsed {
            use codex_protocol::parse_command::ParsedCommand::*;
            match p {
                Read { path, .. } => {
                    entry.referenced_paths.push(path.display().to_string());
                }
                ListFiles {
                    path: Some(path), ..
                } => {
                    entry.referenced_paths.push(path.clone());
                }
                Search {
                    path: Some(path),
                    query,
                    ..
                } => {
                    entry.referenced_paths.push(path.clone());
                    if let Some(q) = query {
                        entry.search_terms.push(q.clone());
                    }
                }
                Search { query: Some(q), .. } => {
                    entry.search_terms.push(q.clone());
                }
                _ => {}
            }
        }

        self.grouper_append_or_start(entry);
        true
    }

    /// Route a command completion through the grouper.
    /// Returns `true` if the call was found in a pending group.
    pub(super) fn grouper_try_handle_command_end(
        &mut self,
        call_id: &str,
        duration: Duration,
        exit_code: i32,
        output: Option<&str>,
    ) -> bool {
        if !self.tool_grouper.is_enabled() {
            return false;
        }

        let found = if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
        {
            if let Some(out) = output {
                cell.append_output(call_id, out);
            }
            cell.complete_call(call_id, duration, exit_code)
        } else {
            false
        };

        if found {
            self.bump_active_cell_revision();
            // Check if should flush (all calls completed)
            let should_flush = self
                .transcript
                .active_cell
                .as_ref()
                .and_then(|c| c.as_any().downcast_ref::<GroupedToolCallCell>())
                .is_some_and(|g| g.should_flush());
            if should_flush {
                self.grouper_flush_pending();
            }
        }

        found
    }

    // ── MCP tools ───────────────────────────────────────────────

    pub(super) fn grouper_try_handle_mcp_start(
        &mut self,
        call_id: &str,
        server: &str,
        tool: &str,
    ) -> bool {
        if !self.tool_grouper.is_enabled() {
            return false;
        }

        let entry =
            ToolCallEntry::new_mcp(call_id.to_string(), server.to_string(), tool.to_string());
        self.grouper_append_or_start(entry);
        true
    }

    pub(super) fn grouper_try_handle_mcp_end(
        &mut self,
        call_id: &str,
        duration: Duration,
        is_error: bool,
    ) -> bool {
        if !self.tool_grouper.is_enabled() {
            return false;
        }

        let found = if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
        {
            cell.complete_call(call_id, duration, if is_error { 1 } else { 0 })
        } else {
            false
        };

        if found {
            self.bump_active_cell_revision();
            let should_flush = self
                .transcript
                .active_cell
                .as_ref()
                .and_then(|c| c.as_any().downcast_ref::<GroupedToolCallCell>())
                .is_some_and(|g| g.should_flush());
            if should_flush {
                self.grouper_flush_pending();
            }
        }

        found
    }

    // ── Web search ──────────────────────────────────────────────

    pub(super) fn grouper_try_handle_web_start(&mut self, call_id: &str, query: &str) -> bool {
        if !self.tool_grouper.is_enabled() {
            return false;
        }

        let entry = ToolCallEntry::new_web(call_id.to_string(), query.to_string());
        self.grouper_append_or_start(entry);
        true
    }

    pub(super) fn grouper_try_handle_web_end(&mut self, call_id: &str, duration: Duration) -> bool {
        self.grouper_try_handle_command_end(call_id, duration, 0, None)
    }

    // ── Hooks ───────────────────────────────────────────────────

    pub(super) fn grouper_try_handle_hook_start(&mut self, run_id: &str, event_name: &str) -> bool {
        if !self.tool_grouper.is_enabled() {
            return false;
        }

        let entry = ToolCallEntry::new_hook(run_id.to_string(), event_name.to_string());
        self.grouper_append_or_start(entry);
        true
    }

    pub(super) fn grouper_try_handle_hook_end(
        &mut self,
        run_id: &str,
        duration: Duration,
        failed: bool,
    ) -> bool {
        if !self.tool_grouper.is_enabled() {
            return false;
        }

        let found = if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
        {
            cell.complete_call(run_id, duration, if failed { 1 } else { 0 })
        } else {
            false
        };

        if found {
            self.bump_active_cell_revision();
            let should_flush = self
                .transcript
                .active_cell
                .as_ref()
                .and_then(|c| c.as_any().downcast_ref::<GroupedToolCallCell>())
                .is_some_and(|g| g.should_flush());
            if should_flush {
                self.grouper_flush_pending();
            }
        }

        found
    }

    // ── Patch apply ─────────────────────────────────────────────

    pub(super) fn grouper_try_handle_patch(&mut self, call_id: &str, file_count: usize) -> bool {
        if !self.tool_grouper.is_enabled() {
            return false;
        }

        // Patches are writes — flush any pending read group first
        self.grouper_flush_pending();

        let mut entry = ToolCallEntry::new_patch(call_id.to_string(), file_count);
        entry.complete(Duration::ZERO, 0);
        // Don't group patches — they are important to see individually
        // Just flush the pending group and let patch go through normal path
        false
    }

    // ── Group management ────────────────────────────────────────

    /// Append entry to pending group or start a new one.
    fn grouper_append_or_start(&mut self, entry: ToolCallEntry) {
        if !self.tool_grouper.is_enabled() {
            return;
        }

        let key = self.grouper_thread_key();

        // If a text break was marked, flush the current group first.
        if self.tool_grouper.had_text_break(key.as_deref()) {
            self.grouper_flush_pending();
            self.tool_grouper.clear_text_break(key.as_deref());
        }

        // Try to append to the existing active group in the transcript.
        if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
            && cell.add_entry(entry.clone())
        {
            self.bump_active_cell_revision();
            self.request_redraw();
            return;
        }

        // Can't append — flush old group and start a new one.
        self.grouper_flush_pending();
        let group = GroupedToolCallCell::new(entry, self.config.animations);
        self.transcript.active_cell = Some(Box::new(group));
        self.bump_active_cell_revision();
        self.request_redraw();
    }

    /// Flush the pending group to history as a GroupedToolCallCell.
    pub(super) fn grouper_flush_pending(&mut self) {
        if let Some(cell) = self.transcript.active_cell.take() {
            if cell.as_any().is::<GroupedToolCallCell>() {
                if !cell.display_lines(u16::MAX).is_empty() {
                    self.transcript.needs_final_message_separator = true;
                    self.app_event_tx.send(AppEvent::InsertHistoryCell(cell));
                    self.request_pending_usage_output_insertion();
                }
            } else {
                // Put it back if it's not a grouped cell
                self.transcript.active_cell = Some(cell);
            }
        }
    }

    /// Mark a text break (non-tool content arrived).
    pub(super) fn grouper_mark_text_break(&mut self) {
        let key = self.grouper_thread_key().map(|s| s.to_string());
        self.tool_grouper.mark_text_break(key.as_deref());
    }

    /// Toggle grouper enable/disable.
    pub(super) fn grouper_toggle_enabled(&mut self) {
        if self.tool_grouper.is_enabled() {
            self.grouper_flush_pending();
            self.tool_grouper.set_enabled(false);
        } else {
            self.tool_grouper.set_enabled(true);
        }
    }

    /// Returns true if the grouper has a pending (unflushed) group.
    pub(super) fn grouper_has_pending(&self) -> bool {
        self.transcript
            .active_cell
            .as_ref()
            .is_some_and(|c| c.as_any().is::<GroupedToolCallCell>())
    }
}
