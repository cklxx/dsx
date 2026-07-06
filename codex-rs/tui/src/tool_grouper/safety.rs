//! Safety signal aggregation for grouped tool calls.

use super::entry::ToolCallEntry;

/// Aggregated safety signals for a group of tool calls.
///
/// Displayed in the collapsed header so that dangerous operations are never hidden
/// by the grouping UI.
#[derive(Debug, Clone, Default)]
pub(crate) struct SafetySignals {
    /// Number of read operations.
    pub(crate) read_count: u32,
    /// Number of write operations. ≥1 should be highlighted.
    pub(crate) write_count: u32,
    /// Number of failed calls. ≥1 should be highlighted.
    pub(crate) fail_count: u32,
    /// Number of calls touching protected paths. ≥1 should be highlighted.
    pub(crate) protected_path_hits: u32,
    /// Number of calls involving network access.
    pub(crate) network_count: u32,
    /// Total number of shell exec calls.
    pub(crate) shell_exec_count: u32,
}

/// Path patterns considered sensitive.
const PROTECTED_PATTERNS: &[&str] = &[
    ".ssh/",
    ".env",
    ".aws/",
    ".kube/",
    "/etc/",
    "/root/",
    "id_rsa",
    "id_ed25519",
    ".pem",
    ".key",
    "auth.json",
    "credentials",
];

impl SafetySignals {
    /// Build safety signals from a list of entries.
    pub(crate) fn from_entries(entries: &[ToolCallEntry]) -> Self {
        let mut signals = Self::default();
        for entry in entries {
            use super::category::ToolCategory::*;
            match &entry.category {
                FileRead | FileSearch | FileList => signals.read_count += 1,
                FileWrite => signals.write_count += 1,
                ShellExec { .. } => signals.shell_exec_count += 1,
                _ => {}
            }
            if entry.failed {
                signals.fail_count += 1;
            }
            if entry.is_network {
                signals.network_count += 1;
            }
            for path in &entry.referenced_paths {
                if is_protected_path(path) {
                    signals.protected_path_hits += 1;
                    break;
                }
            }
            // Also check command text for protected paths
            if let Some(cmd) = &entry.command_text {
                if is_protected_path(cmd) {
                    signals.protected_path_hits += 1;
                }
            }
        }
        signals
    }

    /// Returns true if any signal warrants user attention.
    pub(crate) fn has_alerts(&self) -> bool {
        self.write_count > 0 || self.fail_count > 0 || self.protected_path_hits > 0
    }
}

/// Check if a path or command string contains protected patterns.
pub(crate) fn is_protected_path(s: &str) -> bool {
    PROTECTED_PATTERNS.iter().any(|p| s.contains(p))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_protected_detection() {
        assert!(is_protected_path("~/.ssh/id_rsa"));
        assert!(is_protected_path("/etc/passwd"));
        assert!(is_protected_path(".env"));
        assert!(!is_protected_path("src/main.rs"));
    }

    #[test]
    fn test_empty_signals() {
        let signals = SafetySignals::from_entries(&[]);
        assert_eq!(signals.read_count, 0);
        assert_eq!(signals.write_count, 0);
        assert!(!signals.has_alerts());
    }
}
