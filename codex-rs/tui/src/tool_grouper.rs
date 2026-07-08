//! Adjacent compatible tool calls are grouped into a single cell.

use crate::history_cell::HistoryCell;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;
use ratatui::widgets::Wrap;
use std::time::Duration;
use std::time::Instant;

// ── Category ──────────────────────────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Cat {
    Read,
    Mcp(String),
}

impl Cat {
    fn can_append(&self, other: &Cat) -> bool {
        match (self, other) {
            (Cat::Read, Cat::Read) => true,
            (Cat::Mcp(a), Cat::Mcp(b)) => a == b,
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
        Self {
            call_id,
            display_name,
            category,
            start: Instant::now(),
            duration: None,
            failed: false,
            output: None,
        }
    }
}

// ── Cell ──────────────────────────────────────────────────────────

const MAX_PER_GROUP: usize = 10;

#[derive(Debug)]
pub(crate) struct GroupedToolCallCell {
    entries: Vec<ToolCallEntry>,
    animations: bool,
}

impl GroupedToolCallCell {
    pub(crate) fn new(entry: ToolCallEntry, animations: bool) -> Self {
        Self {
            entries: vec![entry],
            animations,
        }
    }

    pub(crate) fn add_entry(&mut self, entry: &ToolCallEntry) -> bool {
        if self.entries.len() >= MAX_PER_GROUP {
            return false;
        }
        if !self.entries[0].category.can_append(&entry.category) {
            return false;
        }
        if self.entries.iter().any(|e| e.call_id == entry.call_id) {
            return false;
        }
        self.entries.push(entry.clone());
        true
    }

    pub(crate) fn complete(&mut self, id: &str, dur: Duration, code: i32) -> bool {
        for e in &mut self.entries {
            if e.call_id == id {
                e.duration = Some(dur);
                e.failed = code != 0;
                return true;
            }
        }
        false
    }

    /// Output capped at ~2 KB per entry.
    pub(crate) fn append_output(&mut self, id: &str, chunk: &str) {
        if chunk.is_empty() {
            return;
        }
        if let Some(e) = self.entries.iter_mut().rev().find(|e| e.call_id == id) {
            let s = e.output.get_or_insert_default();
            if s.len() < 2000 {
                s.push_str(chunk);
            }
        }
    }

    pub(crate) fn should_flush(&self) -> bool {
        self.entries.iter().all(|e| e.duration.is_some())
    }
}

// ── Free helpers ──────────────────────────────────────────────────

fn marker(done: bool, failed: bool) -> &'static str {
    if !done {
        "•"
    } else if failed {
        "✗"
    } else {
        "✓"
    }
}

fn fmt_dur(d: Duration) -> String {
    let ms = d.as_millis();
    if ms == 0 {
        String::new()
    } else if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m{}s", d.as_secs() / 60, d.as_secs() % 60)
    }
}

fn icon(cat: &Cat) -> &'static str {
    match cat {
        Cat::Read => "📖",
        Cat::Mcp(_) => "🔌",
    }
}

fn out_lines(prefix: &str, out: &str, max: usize) -> Vec<Line<'static>> {
    if out.trim().is_empty() {
        return vec![];
    }
    out.lines()
        .take(max)
        .map(|l| Line::from(format!("{prefix}{l}").dim()))
        .collect()
}

// ── Rendering ─────────────────────────────────────────────────────

impl HistoryCell for GroupedToolCallCell {
    fn display_lines(&self, _width: u16) -> Vec<Line<'static>> {
        if self.entries.len() == 1 {
            let e = &self.entries[0];
            let m = marker(e.duration.is_some(), e.failed);
            let d = e.duration.map(fmt_dur).unwrap_or_default();
            let mut lines = vec![Line::from(format!("{m} {} {d}", e.display_name))];
            if let Some(out) = &e.output {
                lines.extend(out_lines("  ", out, 3));
            }
            return lines;
        }

        let any_failed = self.entries.iter().any(|e| e.failed);
        let all_done = self.should_flush();
        let n = self.entries.len();
        let names: Vec<&str> = self
            .entries
            .iter()
            .take(3)
            .map(|e| e.display_name.as_str())
            .collect();
        let extra = if n > 3 {
            format!(" +{}", n - 3)
        } else {
            String::new()
        };

        let mut lines = vec![Line::from(format!(
            "{m} {ic} {n} calls · {summary} {total}",
            m = marker(all_done, any_failed),
            ic = icon(&self.entries[0].category),
            summary = format!("{}{}", names.join(", "), extra),
            total = fmt_dur(self.entries.iter().filter_map(|e| e.duration).sum()),
        ))];

        if any_failed {
            for (i, e) in self.entries.iter().enumerate() {
                lines.push(Line::from(format!(
                    "  {i}. {m} {name} {d}",
                    m = marker(e.duration.is_some(), e.failed),
                    name = e.display_name,
                    d = e.duration.map(fmt_dur).unwrap_or_default(),
                )));
                if let Some(out) = &e.output {
                    lines.extend(out_lines("    ", out, 2));
                }
            }
        }

        lines
    }

    fn raw_lines(&self) -> Vec<Line<'static>> {
        self.display_lines(80)
    }
    fn transcript_lines(&self, w: u16) -> Vec<Line<'static>> {
        self.display_lines(w)
    }
    fn desired_height(&self, w: u16) -> u16 {
        Paragraph::new(Text::from(self.display_lines(w)))
            .wrap(Wrap { trim: false })
            .line_count(w) as u16
    }
    fn transcript_animation_tick(&self) -> Option<u64> {
        if !self.should_flush() && self.animations {
            Some(self.entries[0].start.elapsed().as_millis() as u64 / 100)
        } else {
            None
        }
    }
    fn is_stream_continuation(&self) -> bool {
        !self.should_flush()
    }
}

// ── Public helpers ────────────────────────────────────────────────

pub(crate) fn is_read_like(first_word: &str) -> bool {
    matches!(
        first_word,
        "cat" | "head" | "tail" | "bat" | "less"
            | "grep" | "rg" | "find" | "fd" | "ag"
            | "ls" | "dir" | "tree" | "du"
    )
}
