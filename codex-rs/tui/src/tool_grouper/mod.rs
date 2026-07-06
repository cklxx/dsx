//! Tool call grouper — unifies adjacent tool calls into collapsible groups.
//!
//! The grouper uses per-agent active groups so sub-agent calls don't mix with
//! the main agent's transcript. Sub-agent activity is routed to the bottom
//! pane status bar instead of the transcript.

pub mod category;
pub mod cell;
pub mod entry;
pub mod render;
pub mod safety;

pub use category::ToolCategory;
pub use category::dominant_category;
pub use category::infer_from_shell;
pub use category::infer_from_tool_name;
pub use cell::GroupedToolCallCell;
pub use entry::CallStatus;
pub use entry::ToolCallEntry;
pub use safety::SafetySignals;

use codex_protocol::ThreadId;
use std::collections::HashMap;
use std::time::Instant;

/// Maximum calls per group before auto-splitting.
const MAX_CALLS_PER_GROUP: usize = 12;

/// Special thread id for the main agent's tool calls.
///
/// We use a dedicated key rather than the session's primary thread id because
/// the ChatWidget doesn't always know its own thread id at construction time.
pub(crate) const MAIN_AGENT_KEY: &str = "__main__";

/// Lightweight grouping metadata for an active group.
#[derive(Debug, Clone)]
struct ActiveGroupMeta {
    dominant_category: ToolCategory,
    call_count: usize,
}

pub struct ToolCallGrouper {
    /// Per-agent active group metadata (not the renderable cell).
    active_groups: HashMap<String, ActiveGroupMeta>,
    /// Per-agent text break flags.
    text_breaks: HashMap<String, bool>,
    /// Config: max calls before auto-split.
    max_calls_per_group: usize,
    /// Config: whether grouping is enabled.
    pub enabled: bool,
    /// When the last call in the active group started (for animation).
    last_call_started_at: Option<Instant>,
}

impl Default for ToolCallGrouper {
    fn default() -> Self {
        Self::new()
    }
}

impl ToolCallGrouper {
    pub fn new() -> Self {
        Self {
            active_groups: HashMap::new(),
            text_breaks: HashMap::new(),
            max_calls_per_group: MAX_CALLS_PER_GROUP,
            enabled: true,
            last_call_started_at: None,
        }
    }

    /// Signal that text was emitted by this agent, breaking potential grouping.
    pub fn mark_text_break(&mut self, agent_key: &str) {
        self.text_breaks.insert(agent_key.to_string(), true);
    }

    /// Convenience: mark text break for the main agent.
    pub fn mark_main_text_break(&mut self) {
        self.mark_text_break(MAIN_AGENT_KEY);
    }

    /// Returns true if the grouper has an active group for this agent.
    pub fn has_active_group(&self, agent_key: &str) -> bool {
        self.active_groups.contains_key(agent_key)
    }

    /// Returns the dominant category of the active group for this agent.
    pub fn active_group_category(&self, agent_key: &str) -> Option<ToolCategory> {
        self.active_groups
            .get(agent_key)
            .map(|g| g.dominant_category.clone())
    }

    /// Returns the call count of the active group for this agent.
    pub fn active_group_call_count(&self, agent_key: &str) -> usize {
        self.active_groups
            .get(agent_key)
            .map(|g| g.call_count)
            .unwrap_or(0)
    }

    /// Convenience: get the main agent's active group category.
    pub fn main_active_group_category(&self) -> Option<ToolCategory> {
        self.active_group_category(MAIN_AGENT_KEY)
    }

    /// Try to add a call to an agent's active group. Returns the action the
    /// caller should take.
    pub fn on_call_started(&mut self, agent_key: &str, entry: ToolCallEntry) -> GroupAction {
        if !self.enabled {
            return GroupAction::NewStandalone(entry);
        }

        // If there's a text break, flush the current group first.
        let has_text_break = self.text_breaks.get(agent_key).copied().unwrap_or(false);

        if has_text_break {
            self.text_breaks.insert(agent_key.to_string(), false);
            self.active_groups.remove(agent_key);
            return GroupAction::FlushAndNew { new_entry: entry };
        }

        let group_is_full = self
            .active_groups
            .get(agent_key)
            .is_some_and(|g| g.call_count >= self.max_calls_per_group);

        if group_is_full {
            self.active_groups.remove(agent_key);
            return GroupAction::FlushAndNew { new_entry: entry };
        }

        // Try to append to the current active group.
        if let Some(meta) = self.active_groups.get(agent_key) {
            if meta.dominant_category.compatible_with(&entry.category) {
                let meta = self.active_groups.get_mut(agent_key).unwrap();
                meta.call_count += 1;
                if entry.category.is_write() && !meta.dominant_category.is_write() {
                    meta.dominant_category = ToolCategory::FileWrite;
                }
                self.last_call_started_at = Some(Instant::now());
                return GroupAction::Appended;
            }
            // Not compatible — flush current and start new.
            self.active_groups.remove(agent_key);
            return GroupAction::FlushAndNew { new_entry: entry };
        }

        // No active group — caller should create one.
        GroupAction::NewStandalone(entry)
    }

    /// Convenience: start a call for the main agent.
    pub fn on_main_call_started(&mut self, entry: ToolCallEntry) -> GroupAction {
        self.on_call_started(MAIN_AGENT_KEY, entry)
    }

    /// Called when a call completes. Updates the agent's active group.
    pub fn on_call_completed(
        &mut self,
        _agent_key: &str,
        _call_id: &str,
        _output: String,
        _exit_code: i32,
        _duration: std::time::Duration,
    ) -> bool {
        // The actual cell lives in transcript.active_cell — caller updates it directly.
        // This method exists for API completeness.
        true
    }

    /// Convenience: complete a call for the main agent.
    pub fn on_main_call_completed(
        &mut self,
        call_id: &str,
        output: String,
        exit_code: i32,
        duration: std::time::Duration,
    ) -> bool {
        self.on_call_completed(MAIN_AGENT_KEY, call_id, output, exit_code, duration)
    }

    /// Append output to a running call in the agent's active group.
    pub fn append_output(&mut self, _agent_key: &str, _call_id: &str, _delta: &str) -> bool {
        // The actual cell lives in transcript.active_cell — caller updates it directly.
        true
    }

    /// Convenience: append output for the main agent.
    pub fn append_main_output(&mut self, call_id: &str, delta: &str) -> bool {
        self.append_output(MAIN_AGENT_KEY, call_id, delta)
    }

    /// Clear the active group metadata for an agent (after flushing to history).
    pub fn clear_active_group(&mut self, agent_key: &str) {
        self.last_call_started_at = None;
        self.active_groups.remove(agent_key);
    }

    /// Convenience: clear the main agent's active group metadata.
    pub fn clear_main_active_group(&mut self) {
        self.clear_active_group(MAIN_AGENT_KEY);
    }

    /// Set the active group metadata for an agent.
    pub fn set_active_group_meta(
        &mut self,
        agent_key: &str,
        dominant_category: ToolCategory,
        call_count: usize,
    ) {
        self.last_call_started_at = Some(Instant::now());
        self.active_groups.insert(
            agent_key.to_string(),
            ActiveGroupMeta {
                dominant_category,
                call_count,
            },
        );
    }

    /// Convenience: set the main agent's active group metadata.
    pub fn set_main_active_group_meta(
        &mut self,
        dominant_category: ToolCategory,
        call_count: usize,
    ) {
        self.set_active_group_meta(MAIN_AGENT_KEY, dominant_category, call_count);
    }

    /// Clear the active group on failure (caller handles actual cell failure).
    pub fn fail_active_group(&mut self, agent_key: &str) {
        self.clear_active_group(agent_key);
    }

    /// Convenience: clear the main agent's active group on failure.
    pub fn fail_main_active_group(&mut self) {
        self.fail_active_group(MAIN_AGENT_KEY);
    }

    /// Returns true if the agent has an active group.
    pub fn has_running_calls(&self, agent_key: &str) -> bool {
        self.active_groups.contains_key(agent_key)
    }

    /// Note: expand/collapse is handled by the transcript's active cell directly.

    /// Clear all active group metadata.
    pub fn clear_all_active_groups(&mut self) {
        self.last_call_started_at = None;
        self.active_groups.clear();
    }
}

/// The action the caller should take after reporting a new call to the grouper.
#[derive(Debug)]
pub enum GroupAction {
    /// Call was appended to the current active group. No new cell needed.
    Appended,
    /// Call should start a new standalone group. The caller should create a
    /// new `GroupedToolCallCell` and set it as active.
    NewStandalone(ToolCallEntry),
    /// The previous active group should be flushed to history, and a new
    /// group should be started with `new_entry`. The caller takes the actual
    /// cell from `transcript.active_cell`.
    FlushAndNew { new_entry: ToolCallEntry },
}

/// Helper: build an agent key from a ThreadId.
pub fn agent_key(thread_id: &ThreadId) -> String {
    format!("agent:{}", thread_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, name: &str, cat: ToolCategory) -> ToolCallEntry {
        let mut e = ToolCallEntry::new_running(id.into(), name.into(), cat);
        e.input_preview = format!("{name} some args");
        e
    }

    #[test]
    fn first_call_creates_new_standalone() {
        let mut grouper = ToolCallGrouper::new();
        let entry = make_entry("1", "rg", ToolCategory::FileSearch);
        let action = grouper.on_main_call_started(entry);
        assert!(matches!(action, GroupAction::NewStandalone(_)));
    }

    #[test]
    fn compatible_call_appended() {
        let mut grouper = ToolCallGrouper::new();
        grouper.set_main_active_group_meta(ToolCategory::FileSearch, 1);

        let e2 = make_entry("2", "cat", ToolCategory::FileRead);
        let action = grouper.on_main_call_started(e2);
        assert!(matches!(action, GroupAction::Appended));
    }

    #[test]
    fn write_not_appended_to_read_group() {
        let mut grouper = ToolCallGrouper::new();
        grouper.set_main_active_group_meta(ToolCategory::FileSearch, 1);

        let e2 = make_entry("2", "apply_patch", ToolCategory::FileWrite);
        let action = grouper.on_main_call_started(e2);
        assert!(matches!(action, GroupAction::FlushAndNew { .. }));
    }

    #[test]
    fn text_break_flushes_group() {
        let mut grouper = ToolCallGrouper::new();
        grouper.set_main_active_group_meta(ToolCategory::FileSearch, 1);
        grouper.mark_main_text_break();

        let e2 = make_entry("2", "cat", ToolCategory::FileRead);
        let action = grouper.on_main_call_started(e2);
        assert!(matches!(action, GroupAction::FlushAndNew { .. }));
    }

    #[test]
    fn per_agent_isolation() {
        let mut grouper = ToolCallGrouper::new();
        grouper.set_main_active_group_meta(ToolCategory::FileSearch, 1);

        // Sub-agent should NOT get the main agent's group.
        assert!(!grouper.has_active_group("agent:worker-1"));
        assert!(grouper.has_active_group(MAIN_AGENT_KEY));

        // Adding to sub-agent should not affect main.
        let sub_entry = make_entry("sub-1", "cat", ToolCategory::FileRead);
        let action = grouper.on_call_started("agent:worker-1", sub_entry);
        assert!(matches!(action, GroupAction::NewStandalone(_)));
    }

    #[test]
    fn disabled_grouper_always_standalone() {
        let mut grouper = ToolCallGrouper::new();
        grouper.enabled = false;
        grouper.set_main_active_group_meta(ToolCategory::FileSearch, 1);

        let e2 = make_entry("2", "cat", ToolCategory::FileRead);
        let action = grouper.on_main_call_started(e2);
        assert!(matches!(action, GroupAction::NewStandalone(_)));
    }

    #[test]
    fn max_calls_splits_group() {
        let mut grouper = ToolCallGrouper::new();
        grouper.max_calls_per_group = 2;
        grouper.set_main_active_group_meta(ToolCategory::FileSearch, 2);

        let e3 = make_entry("3", "ls", ToolCategory::FileList);
        let action = grouper.on_main_call_started(e3);
        assert!(matches!(action, GroupAction::FlushAndNew { .. }));
    }
}
