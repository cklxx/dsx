//! Adjacent compatible tool calls are grouped into a collapsible cell.
//! Disabled by default; opt-in via `set_enabled(true)`.

use crate::history_cell::HistoryCell;
use ratatui::prelude::*;
use ratatui::widgets::{Paragraph, Wrap};
use std::time::{Duration, Instant};

// ── Category ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cat {
    Read,
    Search,
    Write,
    Shell(String),
    Web,
    Mcp(String),
    Hook,
    Plan,
    Other,
}

impl Cat {
    fn can_append(&self, other: &Cat) -> bool {
        match (self, other) {
            (Cat::Read | Cat::Search, Cat::Read | Cat::Search) => true,
            (Cat::Write, Cat::Write) => true,
            (Cat::Shell(a), Cat::Shell(b)) => a == b,
            (Cat::Mcp(a), Cat::Mcp(b)) => a == b,
            (Cat::Hook, Cat::Hook) | (Cat::Web, Cat::Web) => true,
            _ => false,
        }
    }
}

// ── Entry ─────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct ToolCallEntry {
    pub(crate) call_id: String,
    pub(crate) display_name: String,
    category: Cat,
    start: Instant,
    pub(crate) duration: Option<Duration>,
    failed: bool,
    output: Option<String>,
}

impl ToolCallEntry {
    pub(crate) fn new(call_id: String, display_name: String, category: Cat) -> Self {
        Self { call_id, display_name, category, start: Instant::now(), duration: None, failed: false, output: None }
    }
}

// ── Cell ──────────────────────────────────────────────────────────

const MAX_PER_GROUP: usize = 10;

#[derive(Debug)]
pub(crate) struct GroupedToolCallCell {
    entries: Vec<ToolCallEntry>,
    expanded: bool,
    animations: bool,
}

impl GroupedToolCallCell {
    pub(crate) fn new(entry: ToolCallEntry, animations: bool) -> Self {
        Self { entries: vec![entry], expanded: false, animations }
    }

    pub(crate) fn add_entry(&mut self, entry: &ToolCallEntry) -> bool {
        if self.entries.len() >= MAX_PER_GROUP { return false; }
        if !self.entries[0].category.can_append(&entry.category) { return false; }
        if self.entries.iter().any(|e| e.call_id == entry.call_id) { return false; }
        self.entries.push(entry.clone());
        true
    }

    pub(crate) fn complete(&mut self, id: &str, dur: Duration, code: i32) -> bool {
        for e in &mut self.entries {
            if e.call_id == id { e.duration = Some(dur); e.failed = code != 0; return true; }
        }
        false
    }

    pub(crate) fn append_output(&mut self, id: &str, chunk: &str) {
        if chunk.is_empty() { return; }
        if let Some(e) = self.entries.iter_mut().rev().find(|e| e.call_id == id) {
            let s = e.output.get_or_insert_default();
            if s.len() < 2000 { s.push_str(chunk); }
        }
    }

    pub(crate) fn should_flush(&self) -> bool { self.entries.iter().all(|e| e.duration.is_some()) }
    pub(crate) fn len(&self) -> usize { self.entries.len() }

    fn activity_marker(&self, _start: Instant) -> &'static str {
        if self.animations { "⏳" } else { "•" }
    }

    fn render_one(&self) -> Vec<Line<'static>> {
        let e = &self.entries[0];
        let status = if e.duration.is_none() { self.activity_marker(e.start) } else if e.failed { "✗" } else { "✓" };
        let dur = e.duration.map(|d| format_short(d)).unwrap_or_default();
        let mut lines = vec![Line::from(format!("{status} {name} {dur}", name = e.display_name))];
        if let Some(out) = &e.output {
            for l in out.lines().take(3) { lines.push(Line::from(format!("  {l}").dim())); }
        }
        lines
    }

    fn render_group(&self) -> Vec<Line<'static>> {
        let icon = match &self.entries[0].category {
            Cat::Read | Cat::Search => "📖", Cat::Write => "📝", Cat::Web => "🌐",
            Cat::Mcp(_) => "🔌", Cat::Hook => "🪝", Cat::Plan => "📐", _ => "⚡",
        };
        let all_done = self.should_flush();
        let status = if !all_done { self.activity_marker(self.entries[0].start) } else if self.entries.iter().any(|e| e.failed) { "✗" } else { "✓" };
        let n = self.entries.len();
        let dur = format_short(self.entries.iter().filter_map(|e| e.duration).sum());
        let mut lines = vec![Line::from(format!("{status} {icon} {n} calls {dur}"))];
        if self.expanded {
            for (i, e) in self.entries.iter().enumerate() {
                let s = if e.duration.is_none() { self.activity_marker(e.start) } else if e.failed { "✗" } else { "✓" };
                let d = e.duration.map(|d| format_short(d)).unwrap_or_default();
                lines.push(Line::from(format!("  {i}. {s} {name} {d}", name = e.display_name)));
                if let Some(out) = &e.output {
                    for l in out.lines().take(2) { lines.push(Line::from(format!("    {l}").dim())); }
                }
            }
        }
        lines
    }
}

impl HistoryCell for GroupedToolCallCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        if self.entries.len() == 1 { self.render_one() } else { self.render_group() }
    }
    fn raw_lines(&self) -> Vec<Line<'static>> { self.display_lines(80) }
    fn transcript_lines(&self, w: u16) -> Vec<Line<'static>> { self.display_lines(w) }
    fn desired_height(&self, w: u16) -> u16 {
        Paragraph::new(Text::from(self.display_lines(w))).wrap(Wrap { trim: false }).line_count(w) as u16
    }
    fn transcript_animation_tick(&self) -> Option<u64> {
        if !self.should_flush() && self.animations { Some(self.entries[0].start.elapsed().as_millis() as u64 / 100) } else { None }
    }
    fn is_stream_continuation(&self) -> bool { !self.should_flush() }
}

// ── Grouper ───────────────────────────────────────────────────────

pub(crate) struct ToolCallGrouper {
    enabled: bool,
}

impl ToolCallGrouper {
    pub(crate) fn new() -> Self { Self { enabled: false } }
    pub(crate) fn set_enabled(&mut self, v: bool) { self.enabled = v; }
    pub(crate) fn is_enabled(&self) -> bool { self.enabled }
}

// ── Public helpers ────────────────────────────────────────────────

pub(crate) fn infer_category(first_word: &str) -> Cat {
    match first_word {
        "cat" | "head" | "tail" | "bat" | "less" => Cat::Read,
        "grep" | "rg" | "find" | "fd" | "ag" => Cat::Search,
        "ls" | "dir" | "tree" | "du" => Cat::Search,
        "rm" | "mv" | "cp" | "mkdir" | "touch" | "chmod" | "chown" | "tee" => Cat::Write,
        "curl" | "wget" => Cat::Web,
        _ => Cat::Shell(first_word.to_string()),
    }
}

fn format_short(d: Duration) -> String {
    let ms = d.as_millis();
    if ms == 0 { String::new() } else if ms < 1000 { format!("{}ms", ms) } else if ms < 60_000 { format!("{:.1}s", ms as f64 / 1000.0) } else { format!("{}m{}s", d.as_secs() / 60, d.as_secs() % 60) }
}
