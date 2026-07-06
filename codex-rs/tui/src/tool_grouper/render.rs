//! Rendering for grouped tool call cells (collapsed, expanded, and single-call modes).

use super::category::ToolCategory;
use super::cell::GroupedToolCallCell;
use super::entry::ToolCallEntry;
use crate::motion::MotionMode;
use crate::motion::ReducedMotionIndicator;
use crate::motion::activity_indicator;
use crate::text_formatting::truncate_text;
use ratatui::prelude::*;
use ratatui::style::Stylize;
use std::time::Duration;

/// Indent prefix for group rows.
const GROUP_INDENT: &str = "  ╷ ";

/// Render an activity indicator (spinner or static bullet) for a running call.
fn activity_marker(start_time: std::time::Instant, animations_enabled: bool) -> String {
    activity_indicator(
        Some(start_time),
        MotionMode::from_animations_enabled(animations_enabled),
        ReducedMotionIndicator::StaticBullet,
    )
    .map(|s| s.to_string())
    .unwrap_or_else(|| "•".to_string())
}

/// Render a single-call group (no collapse chrome, looks like a normal call).
pub(crate) fn render_single_call(cell: &GroupedToolCallCell, _width: u16) -> Vec<Line<'static>> {
    let Some(entry) = cell.entries().first() else {
        return Vec::new();
    };

    let mut lines = Vec::new();
    let icon = entry.category.icon();
    let running = entry.is_running();
    let status = if running {
        activity_marker(entry.start_time, cell.animations_enabled())
    } else if entry.failed {
        "✗".red().to_string()
    } else {
        "✓".green().to_string()
    };

    let duration_str = entry.duration.map(format_duration).unwrap_or_default();

    let title = format!(
        "{status} {icon} {name} {duration}",
        status = status,
        icon = icon,
        name = entry.display_name,
        duration = if duration_str.is_empty() {
            String::new()
        } else {
            format!("· {duration_str}")
        },
    );

    lines.push(Line::from(title));

    if let Some(preview) = &entry.output_preview {
        let preview_lines: Vec<&str> = preview.lines().collect();
        let show_lines = preview_lines.len().min(5);
        for line in &preview_lines[..show_lines] {
            lines.push(Line::from(vec![
                Span::from("  ").dim(),
                Span::from(truncate_text(line, 200)).dim(),
            ]));
        }
        if preview_lines.len() > show_lines {
            lines.push(Line::from(
                format!("  … +{} lines", preview_lines.len() - show_lines).dim(),
            ));
        }
    }

    lines
}

/// Render the collapsed header for a multi-call group.
pub(crate) fn render_collapsed(cell: &GroupedToolCallCell, _width: u16) -> Vec<Line<'static>> {
    let entries = cell.entries();
    let safety = cell.safety_signals();
    let total_duration = cell.total_duration();
    let title = semantic_title(entries, cell.dominant_category());
    let icon = cell.dominant_category().icon();

    let running = cell.is_active();
    let status = if running {
        activity_marker(
            entries
                .first()
                .map(|e| e.start_time)
                .unwrap_or(std::time::Instant::now()),
            cell.animations_enabled(),
        )
    } else if safety.fail_count > 0 {
        "✗".red().to_string()
    } else {
        "✓".green().to_string()
    };

    // Build safety signal badges
    let mut badges = Vec::new();
    if safety.read_count > 0 {
        badges.push(format!("{}R", safety.read_count));
    }
    if safety.write_count > 0 {
        badges.push(format!("{}W", safety.write_count).red().to_string());
    }
    if safety.fail_count > 0 {
        badges.push(format!("{}✗", safety.fail_count).red().to_string());
    }
    if safety.protected_path_hits > 0 {
        badges.push(
            format!("🔒{}", safety.protected_path_hits)
                .red()
                .to_string(),
        );
    }
    if safety.network_count > 0 {
        badges.push(format!("🌐{}", safety.network_count));
    }

    let duration_str = format_duration(total_duration);

    let header = format!(
        "{indent}{status} {icon} {title}  {badges} {duration}",
        indent = GROUP_INDENT,
        status = status,
        icon = icon,
        title = title,
        badges = badges.join(" "),
        duration = if duration_str.is_empty() {
            String::new()
        } else {
            format!("· {duration_str}")
        },
    );

    let mut lines = Vec::new();
    lines.push(Line::from(header));

    // Second line: tool summary
    let tool_summary = summarize_tools(entries);
    lines.push(Line::from(
        format!(
            "{indent}  {summary}",
            indent = GROUP_INDENT,
            summary = tool_summary
        )
        .dim(),
    ));

    lines
}

/// Render the expanded view of a multi-call group.
pub(crate) fn render_expanded(cell: &GroupedToolCallCell, _width: u16) -> Vec<Line<'static>> {
    let mut lines = Vec::new();

    // Header with expand indicator
    let collapsed = render_collapsed(cell, _width);
    if let Some(_first_line) = collapsed.into_iter().next() {
        // Replace the status glyph with an expanded marker
        let mut spans: Vec<Span> = Vec::new();
        spans.push(Span::from(GROUP_INDENT));
        spans.push(Span::from("▼ ".cyan()));
        // Skip the indent+status part from the collapsed line by rebuilding
        let entries = cell.entries();
        let title = semantic_title(entries, cell.dominant_category());
        let icon = cell.dominant_category().icon();
        let safety = cell.safety_signals();
        let total_duration = cell.total_duration();
        let duration_str = format_duration(total_duration);

        let mut badges = Vec::new();
        if safety.read_count > 0 {
            badges.push(format!("{}R", safety.read_count));
        }
        if safety.write_count > 0 {
            badges.push(format!("{}W", safety.write_count).red().to_string());
        }
        if safety.fail_count > 0 {
            badges.push(format!("{}✗", safety.fail_count).red().to_string());
        }
        if safety.protected_path_hits > 0 {
            badges.push(
                format!("🔒{}", safety.protected_path_hits)
                    .red()
                    .to_string(),
            );
        }
        if safety.network_count > 0 {
            badges.push(format!("🌐{}", safety.network_count));
        }

        let header_text = format!(
            "{icon} {title}  {badges} {duration}",
            icon = icon,
            title = title,
            badges = badges.join(" "),
            duration = if duration_str.is_empty() {
                String::new()
            } else {
                format!("· {duration_str}")
            },
        );
        lines.push(Line::from(vec![
            Span::from(GROUP_INDENT),
            Span::from("▼ ").cyan(),
            Span::from(header_text),
        ]));
    }

    // Individual call details
    for (idx, entry) in cell.entries().iter().enumerate() {
        let num = idx + 1;
        let running = entry.is_running();
        let status = if running {
            activity_marker(entry.start_time, cell.animations_enabled())
        } else if entry.failed {
            "✗".red().to_string()
        } else {
            "✓".green().to_string()
        };

        let dur = entry.duration.map(format_duration).unwrap_or_default();

        lines.push(Line::from(vec![
            Span::from(format!("{indent}  ┌ {num}. ", indent = GROUP_INDENT)),
            Span::from(format!("{status} ")),
            Span::from(entry.display_name.clone()),
            Span::from(if dur.is_empty() {
                String::new()
            } else {
                format!(" · {dur}")
            })
            .dim(),
        ]));

        if let Some(preview) = &entry.output_preview {
            let preview_lines: Vec<&str> = preview.lines().collect();
            let show = preview_lines.len().min(8);
            for pline in &preview_lines[..show] {
                lines.push(Line::from(vec![
                    Span::from(format!("{indent}  │ ", indent = GROUP_INDENT)).dim(),
                    Span::from(truncate_text(pline, 200)).dim(),
                ]));
            }
            if preview_lines.len() > show {
                lines.push(Line::from(
                    format!(
                        "{indent}  │ … +{remaining} lines",
                        indent = GROUP_INDENT,
                        remaining = preview_lines.len() - show
                    )
                    .dim(),
                ));
            }
            lines.push(Line::from(
                format!("{indent}  └", indent = GROUP_INDENT).dim(),
            ));
        }
    }

    lines
}

/// Generate a semantic title from a group of entries.
pub(crate) fn semantic_title(entries: &[ToolCallEntry], category: &ToolCategory) -> String {
    if entries.len() == 1 {
        return entries[0].display_name.clone();
    }

    // Collect common paths and search terms
    let all_paths: Vec<&str> = entries
        .iter()
        .flat_map(|e| e.referenced_paths.iter().map(|s| s.as_str()))
        .collect();
    let all_terms: Vec<&str> = entries
        .iter()
        .flat_map(|e| e.search_terms.iter().map(|s| s.as_str()))
        .collect();

    let common_path = longest_common_prefix(&all_paths);
    let common_term = all_terms
        .first()
        .copied()
        .filter(|t| all_terms.iter().filter(|tt| tt == &t).count() >= (entries.len() / 2));

    match category {
        ToolCategory::FileSearch | ToolCategory::FileRead | ToolCategory::FileList => {
            if let (Some(path), Some(term)) = (common_path.as_deref(), common_term) {
                format!("Explored {path} for \"{term}\"")
            } else if let Some(term) = common_term {
                format!("Searched for \"{term}\"")
            } else if let Some(path) = common_path.as_deref() {
                format!("Read files in {path}")
            } else {
                format!("{} file operations", entries.len())
            }
        }
        ToolCategory::FileWrite => {
            if let Some(path) = common_path.as_deref() {
                format!("Modified {path}")
            } else {
                format!("{} file writes", entries.len())
            }
        }
        ToolCategory::WebSearch => {
            if let Some(term) = common_term {
                format!("Researched \"{term}\"")
            } else {
                format!("{} web searches", entries.len())
            }
        }
        ToolCategory::McpTool { server } => {
            let tools: Vec<&str> = entries
                .iter()
                .filter_map(|e| e.mcp_tool.as_deref())
                .collect();
            format!("Used {server}: {}", tools.join(", "))
        }
        ToolCategory::Hook => {
            let events: Vec<&str> = entries
                .iter()
                .filter_map(|e| e.hook_event.as_deref())
                .collect();
            if events.len() == 1 {
                format!("{} hook ran", events[0])
            } else {
                format!("{} hooks ran", events.len())
            }
        }
        ToolCategory::ShellExec { command_name } => {
            format!("Ran {command_name} ×{}", entries.len())
        }
        ToolCategory::Plan => format!("{} plan updates", entries.len()),
        ToolCategory::Other => format!("{} tool calls", entries.len()),
    }
}

/// Summarize the tools used in a group for the second line.
fn summarize_tools(entries: &[ToolCallEntry]) -> String {
    use std::collections::HashMap;
    let mut counts: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        let label = match &entry.category {
            ToolCategory::McpTool { server } => {
                entry.mcp_tool.clone().unwrap_or_else(|| server.clone())
            }
            ToolCategory::Hook => entry
                .hook_event
                .clone()
                .unwrap_or_else(|| "hook".to_string()),
            other => other.label().to_string(),
        };
        *counts.entry(label).or_insert(0) += 1;
    }

    let mut items: Vec<(String, usize)> = counts.into_iter().collect();
    items.sort_by(|a, b| b.1.cmp(&a.1));

    items
        .iter()
        .map(|(name, count)| {
            if *count > 1 {
                format!("{name}×{count}")
            } else {
                name.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" · ")
}

/// Format a duration for display.
pub(crate) fn format_duration(d: Duration) -> String {
    let ms = d.as_millis();
    if ms == 0 {
        return String::new();
    }
    if ms < 1000 {
        format!("{}ms", ms)
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let secs = d.as_secs();
        format!("{}m{}s", secs / 60, secs % 60)
    }
}

/// Find the longest common path prefix from a list of paths.
fn longest_common_prefix(paths: &[&str]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let first = paths[0];
    let mut prefix_len = first.len();

    for path in &paths[1..] {
        prefix_len = prefix_len.min(
            first
                .chars()
                .zip(path.chars())
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }

    if prefix_len == 0 {
        return None;
    }

    let prefix: String = first.chars().take(prefix_len).collect();
    // Trim to last path separator
    if let Some(sep_pos) = prefix.rfind('/') {
        let trimmed = &prefix[..=sep_pos];
        if !trimmed.is_empty() && trimmed != "/" {
            return Some(trimmed.to_string());
        }
    }

    Some(prefix)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_duration() {
        assert_eq!(format_duration(Duration::from_millis(0)), "");
        assert_eq!(format_duration(Duration::from_millis(500)), "500ms");
        assert_eq!(format_duration(Duration::from_millis(2400)), "2.4s");
        assert_eq!(format_duration(Duration::from_secs(125)), "2m5s");
    }

    #[test]
    fn test_longest_common_prefix() {
        assert_eq!(
            longest_common_prefix(&["src/foo.rs", "src/bar.rs"]),
            Some("src/".to_string())
        );
        assert_eq!(
            longest_common_prefix(&["/a/b/c", "/a/b/d"]),
            Some("/a/b/".to_string())
        );
        assert_eq!(longest_common_prefix(&[]), None);
    }

    #[test]
    fn test_semantic_title_hook() {
        let e1 = ToolCallEntry::new_hook("h1".into(), "PostToolUse".into());
        let e2 = ToolCallEntry::new_hook("h2".into(), "PreToolUse".into());
        let title = semantic_title(&[e1, e2], &ToolCategory::Hook);
        assert!(title.contains("hooks ran"));
    }
}
