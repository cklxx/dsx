use super::*;
use crate::tool_grouper::Cat;
use crate::tool_grouper::GroupedToolCallCell;
use crate::tool_grouper::ToolCallEntry;
use crate::tool_grouper::is_read_like;

impl ChatWidget {
    pub(super) fn grouper_try_command(&mut self, call_id: &str, command: &[String]) -> bool {
        if !self.tool_grouper_enabled {
            return false;
        }
        let first = command.first().map(|s| s.as_str()).unwrap_or("");
        if !is_read_like(first) {
            return false;
        }
        let name = command.join(" ");
        let entry = ToolCallEntry::new(call_id.to_string(), name, Cat::Read);
        self.grouper_insert(entry);
        true
    }

    pub(super) fn grouper_try_mcp(&mut self, call_id: &str, server: &str, tool: &str) -> bool {
        if !self.tool_grouper_enabled {
            return false;
        }
        let entry = ToolCallEntry::new(
            call_id.to_string(),
            format!("{server}/{tool}"),
            Cat::Mcp(server.to_string()),
        );
        self.grouper_insert(entry);
        true
    }

    fn grouper_insert(&mut self, entry: ToolCallEntry) {
        let appended = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
            .is_some_and(|cell| cell.add_entry(&entry));
        if appended {
            self.bump_active_cell_revision();
        } else {
            self.flush_active_cell();
            self.transcript.active_cell = Some(Box::new(GroupedToolCallCell::new(
                entry,
                self.config.animations,
            )));
            self.bump_active_cell_revision();
        }
        self.request_redraw();
    }

    pub(super) fn grouper_complete(
        &mut self,
        call_id: &str,
        dur: Duration,
        code: i32,
        output: Option<&str>,
    ) -> bool {
        if !self.tool_grouper_enabled {
            return false;
        }
        let is_grouped = self
            .transcript
            .active_cell
            .as_ref()
            .is_some_and(|c| c.as_any().is::<GroupedToolCallCell>());
        if !is_grouped {
            return false;
        }

        // Apply output/complete to the cell, then check if we should flush.
        let should_flush = self
            .transcript
            .active_cell
            .as_mut()
            .and_then(|c| c.as_any_mut().downcast_mut::<GroupedToolCallCell>())
            .is_some_and(|cell| {
                if let Some(out) = output {
                    cell.append_output(call_id, out);
                }
                cell.complete(call_id, dur, code);
                cell.should_flush()
            });
        self.bump_active_cell_revision();
        if should_flush {
            self.flush_active_cell();
        }
        true
    }

    pub(super) fn grouper_flush(&mut self) {
        if self
            .transcript
            .active_cell
            .as_ref()
            .is_some_and(|c| c.as_any().is::<GroupedToolCallCell>())
        {
            self.flush_active_cell();
        }
    }
}
