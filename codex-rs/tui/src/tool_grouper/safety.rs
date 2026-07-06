//! Safety signal detection for tool call groups.
//!
//! These signals are always shown on the collapsed group header so the user
//! can spot dangerous activity without expanding.

use codex_utils_absolute_path::AbsolutePathBuf;
use std::path::Path;

const PROTECTED_PATH_PATTERNS: &[&str] = &[
    ".ssh/",
    ".ssh\\",
    ".env",
    ".aws/",
    ".kube/",
    ".gnupg/",
    ".docker/",
    "/etc/",
    "/root/",
    "id_rsa",
    "id_ed25519",
    "id_ecdsa",
    ".pem",
    ".key",
    "auth.json",
    "credentials",
    ".npmrc",
    ".netrc",
];

#[derive(Debug, Clone, Default)]
pub struct SafetySignals {
    pub read_count: u32,
    pub write_count: u32,
    pub fail_count: u32,
    pub protected_path_hits: u32,
    pub network_count: u32,
    pub shell_exec_count: u32,
}

impl SafetySignals {
    pub fn has_warnings(&self) -> bool {
        self.write_count > 0 || self.fail_count > 0 || self.protected_path_hits > 0
    }

    pub fn has_danger(&self) -> bool {
        self.protected_path_hits > 0 || (self.write_count > 0 && self.fail_count > 0)
    }
}

/// Check whether a command and its arguments touch any protected paths.
pub fn detect_protected_paths(command: &str, args: &[String]) -> u32 {
    let full = format!("{command} {}", args.join(" "));
    let mut hits = 0;
    for pattern in PROTECTED_PATH_PATTERNS {
        if full.contains(pattern) {
            hits += 1;
        }
    }
    hits
}

/// Check whether arguments contain URLs (network activity).
pub fn detect_network_activity(args: &[String]) -> bool {
    let full = args.join(" ");
    full.contains("http://") || full.contains("https://") || full.contains("ftp://")
}

/// Format safety signals as compact inline badges for the collapsed header.
///
/// Example: `5R 1W 0✗` or with protected path hit: `🔒 3R 0W 1✗`
pub fn format_safety_badges(signals: &SafetySignals) -> String {
    let mut parts = Vec::new();

    if signals.protected_path_hits > 0 {
        parts.push("🔒".to_string());
    }
    if signals.network_count > 0 {
        parts.push("🌐".to_string());
    }

    parts.push(format!("{}R", signals.read_count));
    if signals.write_count > 0 {
        parts.push(format!("{}W", signals.write_count));
    }
    if signals.fail_count > 0 {
        parts.push(format!("{}✗", signals.fail_count));
    }

    parts.join(" ")
}

/// Returns true if the path looks like it targets a sensitive location.
pub fn is_sensitive_path(path: &Path) -> bool {
    let path_str = path.to_string_lossy();
    for pattern in PROTECTED_PATH_PATTERNS {
        if path_str.contains(pattern) {
            return true;
        }
    }
    false
}

/// Check if an absolute path buffer targets a sensitive location.
pub fn is_sensitive_absolute_path(path: &AbsolutePathBuf) -> bool {
    is_sensitive_path(path.as_path())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_ssh_path() {
        assert!(detect_protected_paths("cat", &["~/.ssh/id_rsa".into()]) > 0);
    }

    #[test]
    fn detects_env_file() {
        assert!(detect_protected_paths("cat", &[".env".into()]) > 0);
    }

    #[test]
    fn normal_file_no_hit() {
        assert_eq!(detect_protected_paths("cat", &["src/main.rs".into()]), 0);
    }

    #[test]
    fn network_detection() {
        assert!(detect_network_activity(&["https://example.com/api".into()]));
        assert!(!detect_network_activity(&["src/main.rs".into()]));
    }

    #[test]
    fn safety_badges_format() {
        let signals = SafetySignals {
            read_count: 5,
            write_count: 1,
            fail_count: 0,
            protected_path_hits: 0,
            network_count: 0,
            shell_exec_count: 0,
        };
        assert_eq!(format_safety_badges(&signals), "5R 1W");
    }

    #[test]
    fn safety_badges_with_protected() {
        let signals = SafetySignals {
            read_count: 3,
            write_count: 0,
            fail_count: 1,
            protected_path_hits: 1,
            network_count: 0,
            shell_exec_count: 0,
        };
        let badges = format_safety_badges(&signals);
        assert!(badges.contains("🔒"));
        assert!(badges.contains("3R"));
        assert!(badges.contains("1✗"));
    }
}
