//! Non-command tool lifecycle rendering for `ChatWidget`.
//!
//! This module handles patch, MCP, web search, image, and collaborator tool
//! events as transcript cells.

use super::*;
use crate::tool_grouper::GroupedToolCallCell;
use crate::tool_grouper::MAIN_AGENT_KEY;
use crate::tool_grouper::ToolCallEntry;
use crate::tool_grouper::ToolCategory;
use crate::tool_grouper::infer_from_tool_name;
use codex_utils_path_uri::LegacyAppPathString;

impl ChatWidget {
    pub(super) fn on_patch_apply_begin(&mut self, changes: HashMap<PathBuf, FileChange>) {
        self.record_visible_turn_activity();

        self.add_to_history(history_cell::new_patch_event(changes, &self.config.cwd));
    }

    pub(super) fn on_view_image_tool_call(&mut self, path: LegacyAppPathString) {
        self.record_visible_turn_activity();
        self.flush_answer_stream_with_separator();
        self.add_to_history(history_cell::new_view_image_tool_call(
            path,
            &self.config.cwd,
        ));
        self.request_redraw();
    }

    pub(super) fn on_image_generation_begin(&mut self) {
        self.record_visible_turn_activity();
        self.flush_answer_stream_with_separator();
    }

    pub(super) fn on_image_generation_end(
        &mut self,
        call_id: String,
        status: String,
        revised_prompt: Option<String>,
        saved_path: Option<AbsolutePathBuf>,
    ) {
        self.flush_answer_stream_with_separator();
        self.add_to_history(history_cell::new_image_generation_call(
            call_id,
            &status,
            revised_prompt,
            saved_path,
        ));
        self.request_redraw();
    }

    pub(super) fn on_file_change_completed(&mut self, item: ThreadItem) {
        let item2 = item.clone();
        self.defer_or_handle(
            |q| q.push_item_completed(item),
            |s| s.handle_file_change_completed_now(item2),
        );
    }

    pub(super) fn on_mcp_tool_call_started(&mut self, item: ThreadItem) {
        let item2 = item.clone();
        self.defer_or_handle(
            |q| q.push_item_started(item),
            |s| s.handle_mcp_tool_call_started_now(item2),
        );
    }

    pub(super) fn on_mcp_tool_call_completed(&mut self, item: ThreadItem) {
        let item2 = item.clone();
        self.defer_or_handle(
            |q| q.push_item_completed(item),
            |s| s.handle_mcp_tool_call_completed_now(item2),
        );
    }

    pub(super) fn on_web_search_begin(&mut self, call_id: String) {
        self.record_visible_turn_activity();

        // Feed to grouper and create grouped cell
        let entry = ToolCallEntry::new_running(
            call_id.clone(),
            "web_search".into(),
            ToolCategory::WebSearch,
        );

        use crate::tool_grouper::GroupAction;
        use crate::tool_grouper::GroupedToolCallCell;
        match self
            .tool_grouper
            .on_call_started(MAIN_AGENT_KEY, entry.clone())
        {
            GroupAction::Appended => {
                if let Some(group_cell) = self
                    .transcript
                    .active_cell
                    .as_mut()
                    .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
                {
                    group_cell.try_add_call(entry);
                    self.bump_active_cell_revision();
                } else {
                    let group = GroupedToolCallCell::new(vec![entry], ToolCategory::WebSearch);
                    self.tool_grouper.set_main_active_group_meta(
                        group.dominant_category.clone(),
                        group.call_count(),
                    );
                    self.transcript.active_cell = Some(Box::new(group));
                    self.bump_active_cell_revision();
                }
            }
            GroupAction::NewStandalone(new_entry) => {
                self.flush_active_cell();
                let dominant = new_entry.category.clone();
                let group = GroupedToolCallCell::new(vec![new_entry], dominant);
                self.tool_grouper.set_main_active_group_meta(
                    group.dominant_category.clone(),
                    group.call_count(),
                );
                self.transcript.active_cell = Some(Box::new(group));
                self.bump_active_cell_revision();
            }
            GroupAction::FlushAndNew { new_entry } => {
                if let Some(old_cell) = self.transcript.active_cell.take() {
                    if old_cell.as_any().is::<GroupedToolCallCell>() {
                        self.app_event_tx
                            .send(AppEvent::InsertHistoryCell(old_cell));
                        self.transcript.needs_final_message_separator = true;
                    } else {
                        self.transcript.active_cell = Some(old_cell);
                        self.flush_active_cell();
                    }
                }
                let dominant = new_entry.category.clone();
                let group = GroupedToolCallCell::new(vec![new_entry], dominant);
                self.tool_grouper.set_main_active_group_meta(
                    group.dominant_category.clone(),
                    group.call_count(),
                );
                self.transcript.active_cell = Some(Box::new(group));
                self.bump_active_cell_revision();
            }
        }

        self.flush_answer_stream_with_separator();
        self.request_redraw();
    }

    pub(super) fn on_web_search_end(
        &mut self,
        call_id: String,
        query: String,
        action: codex_app_server_protocol::WebSearchAction,
    ) {
        // Notify grouper and update grouped cell
        let output_preview = format!("{:?}", action);
        self.tool_grouper.on_call_completed(
            MAIN_AGENT_KEY,
            &call_id,
            output_preview.clone(),
            0,
            std::time::Duration::from_millis(0),
        );
        // Update active grouped cell. Don't flush on completion.
        if let Some(group_cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
        {
            group_cell.complete_call(
                &call_id,
                output_preview.clone(),
                0,
                std::time::Duration::from_millis(0),
            );
            self.bump_active_cell_revision();
            self.request_redraw();
        }

        let active_is_grouped = self
            .transcript
            .active_cell
            .as_ref()
            .is_some_and(|c| c.as_any().is::<GroupedToolCallCell>());
        if active_is_grouped {
            self.transcript.had_work_activity = true;
            return;
        }

        self.flush_answer_stream_with_separator();
        let mut handled = false;
        if let Some(cell) = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<WebSearchCell>())
            && cell.call_id() == call_id
        {
            cell.update(action.clone(), query.clone());
            cell.complete();
            self.bump_active_cell_revision();
            self.flush_active_cell();
            handled = true;
        }

        if !handled {
            self.add_to_history(history_cell::new_web_search_call(call_id, query, action));
        }
        self.transcript.had_work_activity = true;
    }

    pub(super) fn on_collab_event(&mut self, cell: PlainHistoryCell) {
        self.flush_answer_stream_with_separator();
        // Insert directly into history without flushing active cell.
        // Sub-agent events should not interrupt the main agent's active tool group.
        if !cell.display_lines(u16::MAX).is_empty() {
            self.app_event_tx
                .send(AppEvent::InsertHistoryCell(Box::new(cell)));
            self.transcript.needs_final_message_separator = true;
        }
        self.request_redraw();
    }

    pub(super) fn on_collab_agent_tool_call(&mut self, item: ThreadItem) {
        self.record_visible_turn_activity();
        let ThreadItem::CollabAgentToolCall {
            id, tool, status, ..
        } = &item
        else {
            return;
        };
        if matches!(tool, CollabAgentTool::SpawnAgent)
            && let Some(spawn_request) = multi_agents::spawn_request_summary(&item)
        {
            self.pending_collab_spawn_requests
                .insert(id.clone(), spawn_request);
        }

        let cached_spawn_request = if matches!(tool, CollabAgentTool::SpawnAgent)
            && !matches!(status, CollabAgentToolCallStatus::InProgress)
        {
            self.pending_collab_spawn_requests.remove(id)
        } else {
            None
        };

        if let Some(cell) = multi_agents::tool_call_history_cell(
            &item,
            cached_spawn_request.as_ref(),
            |thread_id| self.collab_agent_metadata(thread_id),
        ) {
            self.on_collab_event(cell);
        }
    }

    pub(super) fn on_sub_agent_activity(&mut self, item: ThreadItem) {
        self.record_visible_turn_activity();
        if let Some(cell) = multi_agents::sub_agent_activity_history_cell(&item) {
            self.on_collab_event(cell);
        }
    }

    pub(crate) fn handle_file_change_completed_now(&mut self, item: ThreadItem) {
        let ThreadItem::FileChange { status, .. } = item else {
            return;
        };
        // If the patch was successful, just let the "Edited" block stand.
        // Otherwise, add a failure block.
        if matches!(status, codex_app_server_protocol::PatchApplyStatus::Failed) {
            self.add_to_history(history_cell::new_patch_apply_failure(String::new()));
        }
        // Mark that actual work was done (patch applied)
        self.transcript.had_work_activity = true;
    }

    pub(crate) fn handle_mcp_tool_call_started_now(&mut self, item: ThreadItem) {
        self.record_visible_turn_activity();
        let ThreadItem::McpToolCall {
            id,
            server,
            tool,
            arguments,
            ..
        } = item
        else {
            return;
        };

        // Feed to grouper and create/update grouped cell
        let tool_name = format!("mcp:{}:{}", server, tool);
        let category = infer_from_tool_name(&tool_name, Some(&arguments));
        let mut entry = ToolCallEntry::new_running(id.clone(), tool_name, category.clone());
        entry.input_preview = arguments.to_string();

        use crate::tool_grouper::GroupAction;
        use crate::tool_grouper::GroupedToolCallCell;
        match self
            .tool_grouper
            .on_call_started(MAIN_AGENT_KEY, entry.clone())
        {
            GroupAction::Appended => {
                if let Some(group_cell) = self
                    .transcript
                    .active_cell
                    .as_mut()
                    .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
                {
                    group_cell.try_add_call(entry);
                    self.bump_active_cell_revision();
                } else {
                    let group = GroupedToolCallCell::new(vec![entry], category);
                    self.tool_grouper.set_main_active_group_meta(
                        group.dominant_category.clone(),
                        group.call_count(),
                    );
                    self.transcript.active_cell = Some(Box::new(group));
                    self.bump_active_cell_revision();
                }
            }
            GroupAction::NewStandalone(new_entry) => {
                self.flush_active_cell();
                let dominant = new_entry.category.clone();
                let group = GroupedToolCallCell::new(vec![new_entry], dominant);
                self.tool_grouper.set_main_active_group_meta(
                    group.dominant_category.clone(),
                    group.call_count(),
                );
                self.transcript.active_cell = Some(Box::new(group));
                self.bump_active_cell_revision();
            }
            GroupAction::FlushAndNew { new_entry } => {
                if let Some(old_cell) = self.transcript.active_cell.take() {
                    if old_cell.as_any().is::<GroupedToolCallCell>() {
                        self.app_event_tx
                            .send(AppEvent::InsertHistoryCell(old_cell));
                        self.transcript.needs_final_message_separator = true;
                    } else {
                        self.transcript.active_cell = Some(old_cell);
                        self.flush_active_cell();
                    }
                }
                let dominant = new_entry.category.clone();
                let group = GroupedToolCallCell::new(vec![new_entry], dominant);
                self.tool_grouper.set_main_active_group_meta(
                    group.dominant_category.clone(),
                    group.call_count(),
                );
                self.transcript.active_cell = Some(Box::new(group));
                self.bump_active_cell_revision();
            }
        }

        self.flush_answer_stream_with_separator();
        self.request_redraw();
    }

    pub(crate) fn handle_mcp_tool_call_completed_now(&mut self, item: ThreadItem) {
        self.flush_answer_stream_with_separator();

        let ThreadItem::McpToolCall {
            id,
            server,
            tool,
            arguments,
            result,
            error,
            duration_ms,
            ..
        } = item.clone()
        else {
            return;
        };

        // Notify grouper
        let output_str = match (&result, &error) {
            (_, Some(e)) => e.message.clone(),
            (Some(r), None) => {
                let content_items = &r.content;
                if content_items.is_empty() {
                    String::new()
                } else {
                    content_items
                        .iter()
                        .filter_map(|item| {
                            let obj = item.as_object()?;
                            let text = obj.get("text")?.as_str()?;
                            Some(text.to_string())
                        })
                        .next()
                        .unwrap_or_else(|| content_items[0].to_string())
                        .lines()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(
                            "
",
                        )
                }
            }
            _ => String::new(),
        };
        let exit_code = if error.is_some() { 1 } else { 0 };
        let dur = std::time::Duration::from_millis(duration_ms.unwrap_or_default().max(0) as u64);
        self.tool_grouper.on_call_completed(
            MAIN_AGENT_KEY,
            &id,
            output_str.clone(),
            exit_code,
            dur,
        );

        // Try to update the active grouped cell. If it's still active, we're done —
        // the grouper manages its lifecycle.
        let active_is_grouped = self
            .transcript
            .active_cell
            .as_ref()
            .is_some_and(|c| c.as_any().is::<GroupedToolCallCell>());
        if active_is_grouped {
            if let Some(group_cell) = self
                .transcript
                .active_cell
                .as_mut()
                .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
            {
                group_cell.complete_call(&id, output_str, exit_code, dur);
                self.bump_active_cell_revision();
                self.request_redraw();
            }
            self.transcript.had_work_activity = true;
            return;
        }

        // Fallback: legacy McpToolCallCell path (grouped cell was already flushed
        // to history by a text break before the async completion arrived).
        let invocation = McpInvocation {
            server,
            tool,
            arguments: Some(arguments),
        };
        let duration = Duration::from_millis(duration_ms.unwrap_or_default().max(0) as u64);
        let result = match (result, error) {
            (_, Some(error)) => Err(error.message),
            (Some(result), None) => {
                let result = *result;
                Ok(codex_protocol::mcp::CallToolResult {
                    content: result.content,
                    structured_content: result.structured_content,
                    is_error: Some(false),
                    meta: None,
                })
            }
            (None, None) => Err("MCP tool call completed without a result".to_string()),
        };

        let extra_cell = match self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|cell| cell.as_any_mut().downcast_mut::<McpToolCallCell>())
        {
            Some(cell) if cell.call_id() == id => cell.complete(duration, result),
            _ => {
                self.flush_active_cell();
                let mut cell =
                    history_cell::new_active_mcp_tool_call(id, invocation, self.config.animations);
                let extra_cell = cell.complete(duration, result);
                self.transcript.active_cell = Some(Box::new(cell));
                extra_cell
            }
        };

        self.flush_active_cell();
        if let Some(extra) = extra_cell {
            self.add_boxed_history(extra);
        }
        // Mark that actual work was done (MCP tool call)
        self.transcript.had_work_activity = true;
    }

    pub(crate) fn handle_queued_item_started_now(&mut self, item: ThreadItem) {
        match item {
            item @ ThreadItem::CommandExecution { .. } => {
                self.handle_command_execution_started_now(item);
            }
            item @ ThreadItem::McpToolCall { .. } => {
                self.handle_mcp_tool_call_started_now(item);
            }
            _ => {}
        }
    }

    pub(crate) fn handle_queued_item_completed_now(&mut self, item: ThreadItem) {
        match item {
            item @ ThreadItem::CommandExecution { .. } => {
                self.handle_command_execution_completed_now(item);
            }
            item @ ThreadItem::FileChange { .. } => self.handle_file_change_completed_now(item),
            item @ ThreadItem::McpToolCall { .. } => self.handle_mcp_tool_call_completed_now(item),
            _ => {}
        }
    }
}
