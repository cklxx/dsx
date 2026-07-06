//! Semantic title generation and rendering for grouped tool call cells.

use super::category::ToolCategory;
use super::entry::ToolCallEntry;
use super::safety::SafetySignals;
use crate::render::line_utils::push_owned_lines;
use crate::style::*;
use crate::wrapping::RtOptions;
use crate::wrapping::adaptive_wrap_line;
use codex_utils_elapsed::format_duration;
use ratatui::prelude::*;
use ratatui::style::Stylize;
use std::time::Duration;

const OUTPUT_PREVIEW_LINES: usize = 3;

/// Extract common features from a group of calls for title generation.
pub struct GroupFeatures {
    pub shared_path_prefix: Option<String>,
    pub search_terms: Vec<String>,
    pub actions: Vec<String>,
    pub targets: Vec<String>,
    pub file_types: Vec<String>,
    pub total_files_touched: usize,
}

fn extract_path_from_input(input: &str) -> Option<String> {
    // Try to find path-like tokens in the input preview.
    // Skip the first word (the command name itself).
    for word in input.split_whitespace().skip(1) {
        if word.contains('/') || word.contains('.') || word.starts_with('~') {
            let cleaned =
                word.trim_matches(|c: char| c.is_ascii_punctuation() && c != '/' && c != '.');
            if cleaned.len() > 2 && (cleaned.contains('/') || cleaned.contains('.')) {
                return Some(cleaned.to_string());
            }
        }
    }
    None
}

fn extract_search_pattern(input: &str) -> Option<String> {
    // For rg/grep: the first non-flag argument is the pattern.
    let parts: Vec<&str> = input.split_whitespace().collect();
    for (i, part) in parts.iter().enumerate() {
        if i == 0 {
            continue; // skip command name
        }
        if !part.starts_with('-') && !part.contains('/') && part.len() > 2 {
            return Some(part.trim_matches('"').trim_matches('\'').to_string());
        }
    }
    None
}

fn action_word(category: &ToolCategory) -> &'static str {
    match category {
        ToolCategory::FileRead => "read",
        ToolCategory::FileSearch => "searched",
        ToolCategory::FileList => "listed",
        ToolCategory::FileWrite => "wrote",
        ToolCategory::ShellExec => "ran",
        ToolCategory::WebSearch => "researched",
        ToolCategory::McpTool(_) => "used",
        ToolCategory::Plan => "planned",
        ToolCategory::Hook => "hooked",
        ToolCategory::Other => "called",
    }
}

fn longest_common_prefix(strs: &[String]) -> Option<String> {
    if strs.is_empty() {
        return None;
    }
    let first = &strs[0];
    let mut prefix_len = first.len();
    for s in &strs[1..] {
        prefix_len = prefix_len.min(
            first
                .chars()
                .zip(s.chars())
                .take_while(|(a, b)| a == b)
                .count(),
        );
    }
    if prefix_len == 0 {
        return None;
    }
    // Trim to last path separator.
    let prefix: String = first.chars().take(prefix_len).collect();
    if let Some(sep_pos) = prefix.rfind('/') {
        Some(prefix[..=sep_pos].to_string())
    } else if prefix.len() > 3 {
        Some(prefix)
    } else {
        None
    }
}

pub fn extract_features(calls: &[ToolCallEntry]) -> GroupFeatures {
    let paths: Vec<String> = calls
        .iter()
        .filter_map(|c| extract_path_from_input(&c.input_preview))
        .collect();
    let search_terms: Vec<String> = calls
        .iter()
        .filter(|c| c.category == ToolCategory::FileSearch)
        .filter_map(|c| extract_search_pattern(&c.input_preview))
        .collect();
    let actions: Vec<String> = calls
        .iter()
        .map(|c| action_word(&c.category).to_string())
        .collect();
    let targets: Vec<String> = calls
        .iter()
        .filter_map(|c| {
            let name = c.display_name.rsplit('/').next().unwrap_or(&c.display_name);
            if name.len() > 1 {
                Some(name.to_string())
            } else {
                None
            }
        })
        .collect();

    let shared_path_prefix = longest_common_prefix(&paths);

    GroupFeatures {
        shared_path_prefix,
        search_terms,
        actions,
        targets,
        file_types: Vec::new(),
        total_files_touched: paths.len(),
    }
}

/// Generate a one-line semantic title for the group.
pub fn generate_title(calls: &[ToolCallEntry], dominant: &ToolCategory) -> String {
    let features = extract_features(calls);
    let total_calls = calls.len();
    let total_duration: Duration = calls
        .iter()
        .filter_map(|c| c.duration)
        .fold(Duration::from_millis(0), |acc, d| acc + d);

    match dominant {
        ToolCategory::FileRead | ToolCategory::FileSearch | ToolCategory::FileList => {
            generate_file_exploration_title(&features, total_calls, &total_duration)
        }
        ToolCategory::FileWrite => generate_file_write_title(&features, total_calls),
        ToolCategory::WebSearch => generate_web_search_title(&features, total_calls),
        ToolCategory::McpTool(server) => generate_mcp_title(&features, total_calls, server),
        ToolCategory::ShellExec => generate_shell_exec_title(&features, total_calls, calls),
        ToolCategory::Plan => format!("Updated plan ({total_calls} items)"),
        ToolCategory::Hook => format!("Fired {total_calls} hooks"),
        ToolCategory::Other => format!("{total_calls} tool calls"),
    }
}

fn generate_file_exploration_title(
    features: &GroupFeatures,
    total: usize,
    _duration: &Duration,
) -> String {
    let has_search = !features.search_terms.is_empty();
    let has_path = features.shared_path_prefix.is_some();

    if has_search && has_path {
        let path = features.shared_path_prefix.as_ref().unwrap();
        let term = &features.search_terms[0];
        return format!("Explored {path} for \"{term}\"");
    }
    if has_search {
        let terms: Vec<&str> = features.search_terms.iter().map(|s| s.as_str()).collect();
        if terms.len() == 1 {
            return format!("Searched for \"{}\"", terms[0]);
        } else {
            return format!("Searched for \"{}\" and {} more", terms[0], terms.len() - 1);
        }
    }
    if let Some(path) = &features.shared_path_prefix {
        if total <= 3 {
            return format!("Read files in {path}");
        }
        return format!("Explored {path} ({total} calls)");
    }

    // No common features — use action summary.
    let action_counts = count_unique(&features.actions);
    if action_counts.len() == 1 {
        let (action, count) = &action_counts[0];
        return format!("{} {} files", capitalize(action), count);
    }

    format!("Explored {total} files")
}

fn generate_file_write_title(features: &GroupFeatures, total: usize) -> String {
    if let Some(path) = &features.shared_path_prefix {
        return format!("Modified files in {path} ({total})");
    }
    if total == 1 {
        "Applied patch".into()
    } else {
        format!("Modified {total} files")
    }
}

fn generate_web_search_title(features: &GroupFeatures, total: usize) -> String {
    if !features.search_terms.is_empty() {
        let topic = &features.search_terms[0];
        return format!("Researched \"{topic}\"");
    }
    if total == 1 {
        "Searched the web".into()
    } else {
        format!("Searched the web ({total} queries)")
    }
}

fn generate_mcp_title(features: &GroupFeatures, total: usize, server: &str) -> String {
    let tool_names: Vec<&str> = features.targets.iter().map(|t| t.as_str()).collect();
    if tool_names.len() == 1 {
        format!("Used {server}: {}", tool_names[0])
    } else if !tool_names.is_empty() {
        let unique: Vec<&str> = {
            let mut seen = std::collections::HashSet::new();
            tool_names
                .iter()
                .filter(|t| seen.insert(*t))
                .copied()
                .collect()
        };
        if unique.len() == 1 {
            format!("Called {server}:{} ×{}", unique[0], total)
        } else {
            format!("Used {server}: {} and {} more", unique[0], unique.len() - 1)
        }
    } else {
        format!("Used {server} ({total} calls)")
    }
}

fn generate_shell_exec_title(
    features: &GroupFeatures,
    total: usize,
    calls: &[ToolCallEntry],
) -> String {
    // Detect common patterns from input previews (command arguments).
    let all_previews: String = calls
        .iter()
        .map(|c| c.input_preview.as_str())
        .collect::<Vec<_>>()
        .join(" ");
    let has_build = all_previews.contains("build");
    let has_test = all_previews.contains("test");
    let has_install = all_previews.contains("install")
        || all_previews.contains("npm")
        || all_previews.contains("pip")
        || all_previews.contains("cargo");

    if has_build && has_test {
        return "Built and tested".into();
    }
    if has_build {
        return "Built project".into();
    }
    if has_test {
        return "Ran tests".into();
    }
    if has_install {
        return "Installed dependencies".into();
    }

    // Count unique tool basenames.
    let mut name_counts: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for t in &features.targets {
        *name_counts.entry(t.as_str()).or_insert(0) += 1;
    }
    let mut sorted: Vec<(&str, usize)> = name_counts.into_iter().collect();
    sorted.sort_by(|a, b| b.1.cmp(&a.1));

    if total == 1 && !sorted.is_empty() {
        return format!("Ran {}", sorted[0].0);
    }
    if sorted.len() == 1 {
        return format!("Ran {} ×{total}", sorted[0].0);
    }

    let names: Vec<String> = sorted
        .iter()
        .take(3)
        .map(|(n, c)| {
            if *c > 1 {
                format!("{n}×{c}")
            } else {
                n.to_string()
            }
        })
        .collect();
    format!("Ran {total} commands ({})", names.join(", "))
}

fn count_unique<T: Eq + std::hash::Hash + Clone>(items: &[T]) -> Vec<(T, usize)> {
    let mut counts = std::collections::HashMap::new();
    for item in items {
        *counts.entry(item.clone()).or_insert(0usize) += 1;
    }
    let mut result: Vec<(T, usize)> = counts.into_iter().collect();
    result.sort_by(|a, b| b.1.cmp(&a.1));
    result
}

fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    match chars.next() {
        None => String::new(),
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
    }
}

/// Render the collapsed (default) group header line.
pub fn render_collapsed_header(
    title: &str,
    dominant: &ToolCategory,
    signals: &SafetySignals,
    total_duration: Option<Duration>,
    calls: &[ToolCallEntry],
    width: u16,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    // Single line: icon + semantic title + call count + duration
    let mut spans: Vec<Span<'static>> = Vec::new();
    spans.push("  ".into());
    spans.push(Span::from(dominant.display_icon()));
    spans.push(" ".into());

    if signals.write_count > 0 {
        spans.push(Span::styled(
            title.to_string(),
            Style::default().red().bold(),
        ));
    } else {
        spans.push(Span::from(title.to_string()));
    }

    if let Some(dur) = total_duration {
        spans.push("  ".into());
        spans.push(Span::styled(format_duration(dur), Style::default().dim()));
    }

    if signals.protected_path_hits > 0 {
        spans.push(" 🔒".red().bold().into());
    }

    let line = Line::from(spans);
    let wrapped = adaptive_wrap_line(&line, RtOptions::new(width as usize));
    push_owned_lines(&wrapped, &mut out);

    out
}

/// Render the expanded group: header + each call's detail.
pub fn render_expanded(
    title: &str,
    dominant: &ToolCategory,
    signals: &SafetySignals,
    total_duration: Option<Duration>,
    calls: &[ToolCallEntry],
    width: u16,
) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    // Header line
    let mut header_spans: Vec<Span<'static>> = Vec::new();
    header_spans.push("  ▼ ".into());
    header_spans.push(Span::from(dominant.display_icon()));
    header_spans.push(" ".into());
    if signals.write_count > 0 {
        header_spans.push(Span::styled(
            title.to_string(),
            Style::default().red().bold(),
        ));
    } else {
        header_spans.push(Span::from(title.to_string()));
    }
    if let Some(dur) = total_duration {
        header_spans.push("  ".into());
        header_spans.push(Span::styled(format_duration(dur), Style::default().dim()));
    }
    if signals.protected_path_hits > 0 {
        header_spans.push(" 🔒".red().bold().into());
    }
    let header_line = Line::from(header_spans);
    let wrapped_header = adaptive_wrap_line(&header_line, RtOptions::new(width as usize));
    push_owned_lines(&wrapped_header, &mut out);

    // Each call detail
    for (idx, call) in calls.iter().enumerate() {
        let call_lines = render_single_call_detail(call, idx + 1, width);
        out.extend(call_lines);
    }

    out
}

fn render_single_call_detail(call: &ToolCallEntry, idx: usize, width: u16) -> Vec<Line<'static>> {
    let mut out: Vec<Line<'static>> = Vec::new();

    // Call header: N. cmd_name  — duration, N lines
    let mut header_spans: Vec<Span<'static>> = Vec::new();
    header_spans.push("    ".into());
    header_spans.push(format!("{idx}. ").into());
    header_spans.push(call.display_name.clone().into());
    if !call.input_preview.is_empty() {
        header_spans.push(" ".into());
        header_spans.push(call.input_preview.clone().dim().into());
    }
    if let Some(dur) = call.duration {
        header_spans.push(format!("  — {} ", format_duration(dur)).dim().into());
    }
    if call.output_lines > 0 {
        header_spans.push(format!("({} lines)", call.output_lines).dim().into());
    }
    match call.status {
        crate::tool_grouper::entry::CallStatus::Failed => {
            header_spans.push(" ✗".red().bold().into());
        }
        crate::tool_grouper::entry::CallStatus::Running => {
            header_spans.push(" ⏳".into());
        }
        _ => {}
    }
    let header = Line::from(header_spans);
    let wrapped = adaptive_wrap_line(&header, RtOptions::new(width as usize));
    push_owned_lines(&wrapped, &mut out);

    // Output preview (first N lines)
    if !call.output_preview.is_empty() {
        let preview_lines: Vec<Line<'static>> = call
            .output_preview
            .lines()
            .take(OUTPUT_PREVIEW_LINES)
            .map(|l| {
                let mut spans: Vec<Span<'static>> = Vec::new();
                spans.push("      ".into());
                spans.push(Span::from(l.to_string()).dim());
                Line::from(spans)
            })
            .collect();
        let wrapped_preview = crate::wrapping::adaptive_wrap_lines(
            preview_lines,
            RtOptions::new(width as usize)
                .initial_indent(Line::from("      "))
                .subsequent_indent(Line::from("        ")),
        );
        push_owned_lines(&wrapped_preview, &mut out);

        if call.output_truncated {
            let mut omit_spans: Vec<Span<'static>> = Vec::new();
            omit_spans.push("      ".into());
            omit_spans.push(
                format!("… {} more lines", call.output_lines - OUTPUT_PREVIEW_LINES)
                    .dim()
                    .into(),
            );
            push_owned_lines(&[Line::from(omit_spans)], &mut out);
        }
    }

    out
}

/// Compute total duration from a list of calls (max of start→end spans).
pub fn total_duration(calls: &[ToolCallEntry]) -> Option<Duration> {
    let durations: Vec<Duration> = calls.iter().filter_map(|c| c.duration).collect();
    if durations.is_empty() {
        return None;
    }
    Some(
        durations
            .into_iter()
            .fold(Duration::from_millis(0), |acc, d| acc + d),
    )
}

/// Aggregate safety signals from all calls in a group.
pub fn aggregate_safety(calls: &[ToolCallEntry]) -> SafetySignals {
    let mut agg = SafetySignals::default();
    for call in calls {
        let s = call.safety_contribution();
        agg.read_count += s.read_count;
        agg.write_count += s.write_count;
        agg.fail_count += s.fail_count;
        agg.protected_path_hits += s.protected_path_hits;
        agg.network_count += s.network_count;
        agg.shell_exec_count += s.shell_exec_count;
    }
    agg
}
