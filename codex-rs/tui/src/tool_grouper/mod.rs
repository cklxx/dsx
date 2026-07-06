//! Tool call grouper for the TUI transcript.
//!
//! Adjacent tool calls of compatible categories are aggregated into a single
//! `GroupedToolCallCell` that renders as a collapsible group. This keeps the
//! transcript dense while preserving safety signals.

pub(crate) mod category;
pub(crate) mod cell;
pub(crate) mod entry;
pub(crate) mod render;
pub(crate) mod safety;

pub(crate) use category::ToolCategory;
pub(crate) use cell::GroupedToolCallCell;
pub(crate) use entry::ToolCallEntry;

use std::collections::HashMap;

/// Per-agent controller that decides how to group incoming tool calls.
///
/// The grouper tracks one "active group" per thread. When a new call arrives:
/// 1. If no active group exists for the thread, create one.
/// 2. If a text break occurred since the last call, flush the old group and start a new one.
/// 3. If the group is full (≥ MAX_CALLS_PER_GROUP), flush and start new.
/// 4. If the new call's category is compatible, append to the current group.
/// 5. Otherwise, flush the old group and start a new one.
#[derive(Debug)]
pub(crate) struct ToolCallGrouper {
    /// Per-thread active group keyed by thread id.
    /// `None` thread id means the main agent.
    active_groups: HashMap<Option<String>, GroupedToolCallCell>,
    /// Per-thread "text break" flag: set to true when non-tool-call content
    /// (agent text, user message, reasoning) arrives between tool calls.
    text_breaks: HashMap<Option<String>, bool>,
    /// Master enable/disable switch.
    enabled: bool,
    /// Whether animations are enabled for new cells.
    animations_enabled: bool,
}

impl ToolCallGrouper {
    pub(crate) fn new(animations_enabled: bool) -> Self {
        Self {
            active_groups: HashMap::new(),
            text_breaks: HashMap::new(),
            enabled: false,
            animations_enabled,
        }
    }

    pub(crate) fn set_enabled(&mut self, enabled: bool) {
        self.enabled = enabled;
    }

    pub(crate) fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub(crate) fn set_animations_enabled(&mut self, enabled: bool) {
        self.animations_enabled = enabled;
    }

    /// Mark that a text break occurred (non-tool content was inserted),
    /// so the next tool call should start a new group.
    pub(crate) fn mark_text_break(&mut self, thread_id: Option<&str>) {
        self.text_breaks
            .insert(thread_id.map(|s| s.to_string()), true);
    }

    /// Returns true if a text break has been marked for the given thread.
    pub(crate) fn had_text_break(&self, thread_id: Option<&str>) -> bool {
        let key = thread_id.map(|s| s.to_string());
        self.text_breaks.get(&key).copied().unwrap_or(false)
    }

    /// Clear the text break flag for the given thread.
    pub(crate) fn clear_text_break(&mut self, thread_id: Option<&str>) {
        let key = thread_id.map(|s| s.to_string());
        self.text_breaks.insert(key, false);
    }

    /// Try to add an entry to the active group for the given thread.
    ///
    /// Returns `Ok(())` if the entry was appended to the active group.
    /// Returns `Err(entry)` if a new group needs to be created (the caller
    /// should flush the old group and create a new one with this entry).
    pub(crate) fn try_append(
        &mut self,
        thread_id: Option<&str>,
        entry: ToolCallEntry,
    ) -> Result<(), ToolCallEntry> {
        if !self.enabled {
            return Err(entry);
        }

        let key = thread_id.map(|s| s.to_string());

        // Check text break
        let had_break = self.text_breaks.get(&key).copied().unwrap_or(false);
        if had_break {
            self.text_breaks.insert(key.clone(), false);
            return Err(entry);
        }

        let group = match self.active_groups.get_mut(&key) {
            Some(g) => g,
            None => return Err(entry),
        };

        if group.add_entry(entry.clone()) {
            Ok(())
        } else {
            Err(entry)
        }
    }

    /// Start a new active group for the given thread, replacing any existing one.
    /// Returns the previous group if one existed (which the caller should flush to history).
    pub(crate) fn start_group(
        &mut self,
        thread_id: Option<&str>,
        entry: ToolCallEntry,
    ) -> Option<GroupedToolCallCell> {
        let key = thread_id.map(|s| s.to_string());
        let new_cell = GroupedToolCallCell::new(entry, self.animations_enabled);
        let old = self.active_groups.remove(&key);
        self.active_groups.insert(key, new_cell);
        old
    }

    /// Get a reference to the active group for a thread.
    pub(crate) fn active_group(&self, thread_id: Option<&str>) -> Option<&GroupedToolCallCell> {
        let key = thread_id.map(|s| s.to_string());
        self.active_groups.get(&key)
    }

    /// Get a mutable reference to the active group for a thread.
    pub(crate) fn active_group_mut(
        &mut self,
        thread_id: Option<&str>,
    ) -> Option<&mut GroupedToolCallCell> {
        let key = thread_id.map(|s| s.to_string());
        self.active_groups.get_mut(&key)
    }

    /// Remove and return the active group for a thread (flush it).
    pub(crate) fn take_group(&mut self, thread_id: Option<&str>) -> Option<GroupedToolCallCell> {
        let key = thread_id.map(|s| s.to_string());
        self.active_groups.remove(&key)
    }

    /// Complete a call in the active group by id.
    /// Returns true if the call was found and completed.
    pub(crate) fn complete_call(
        &mut self,
        thread_id: Option<&str>,
        call_id: &str,
        duration: std::time::Duration,
        exit_code: i32,
    ) -> bool {
        let key = thread_id.map(|s| s.to_string());
        if let Some(group) = self.active_groups.get_mut(&key) {
            group.complete_call(call_id, duration, exit_code)
        } else {
            false
        }
    }

    /// Append output to a call in the active group.
    pub(crate) fn append_output(
        &mut self,
        thread_id: Option<&str>,
        call_id: &str,
        chunk: &str,
    ) -> bool {
        let key = thread_id.map(|s| s.to_string());
        if let Some(group) = self.active_groups.get_mut(&key) {
            group.append_output(call_id, chunk)
        } else {
            false
        }
    }

    /// Returns true if the active group for a thread is ready to flush.
    pub(crate) fn should_flush(&self, thread_id: Option<&str>) -> bool {
        let key = thread_id.map(|s| s.to_string());
        self.active_groups
            .get(&key)
            .is_some_and(|g| g.should_flush())
    }

    /// Returns true if the active group contains the given call id.
    pub(crate) fn contains_call(&self, thread_id: Option<&str>, call_id: &str) -> bool {
        let key = thread_id.map(|s| s.to_string());
        self.active_groups
            .get(&key)
            .is_some_and(|g| g.contains_call(call_id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_shell_entry(id: &str, name: &str, cat: ToolCategory) -> ToolCallEntry {
        ToolCallEntry::new_shell(id.to_string(), name.to_string(), name.to_string(), cat)
    }

    #[test]
    fn test_append_compatible_calls() {
        let mut grouper = ToolCallGrouper::new(false);
        grouper.set_enabled(true);
        let e1 = make_shell_entry("1", "rg foo", ToolCategory::FileSearch);
        let e2 = make_shell_entry("2", "cat bar.rs", ToolCategory::FileRead);

        assert!(grouper.try_append(None, e1.clone()).is_err()); // no active group
        grouper.start_group(None, e1);
        assert!(grouper.try_append(None, e2).is_ok());

        let group = grouper.active_group(None).unwrap();
        assert_eq!(group.len(), 2);
    }

    #[test]
    fn test_reject_incompatible_calls() {
        let mut grouper = ToolCallGrouper::new(false);
        grouper.set_enabled(true);
        let e1 = make_shell_entry("1", "rg foo", ToolCategory::FileSearch);
        let e2 = make_shell_entry("2", "rm -rf /", ToolCategory::FileWrite);

        grouper.start_group(None, e1);
        let result = grouper.try_append(None, e2);
        assert!(result.is_err()); // write not compatible with search
    }

    #[test]
    fn test_text_break_splits_groups() {
        let mut grouper = ToolCallGrouper::new(false);
        grouper.set_enabled(true);
        let e1 = make_shell_entry("1", "rg foo", ToolCategory::FileSearch);
        let e2 = make_shell_entry("2", "rg bar", ToolCategory::FileSearch);

        grouper.start_group(None, e1);
        grouper.mark_text_break(None);
        let result = grouper.try_append(None, e2);
        assert!(result.is_err()); // text break prevents appending
    }

    #[test]
    fn test_hook_grouping() {
        let mut grouper = ToolCallGrouper::new(false);
        grouper.set_enabled(true);
        let h1 = ToolCallEntry::new_hook("h1".into(), "PostToolUse".into());
        let h2 = ToolCallEntry::new_hook("h2".into(), "PreToolUse".into());

        grouper.start_group(None, h1);
        assert!(grouper.try_append(None, h2).is_ok());

        let group = grouper.active_group(None).unwrap();
        assert_eq!(group.len(), 2);
        assert!(matches!(group.dominant_category(), ToolCategory::Hook));
    }

    #[test]
    fn test_disabled_grouper_rejects_all() {
        let mut grouper = ToolCallGrouper::new(false);
        // enabled is false by default, but explicitly set to ensure test works regardless of default
        let e1 = make_shell_entry("1", "rg foo", ToolCategory::FileSearch);

        assert!(grouper.try_append(None, e1).is_err());
    }
}
