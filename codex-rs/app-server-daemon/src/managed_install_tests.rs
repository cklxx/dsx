use pretty_assertions::assert_eq;

use super::managed_codex_file_name;
use super::parse_codex_version;

#[test]
fn managed_executable_uses_dsx_name() {
    assert_eq!(managed_codex_file_name(), "dsx");
}

#[test]
fn parses_codex_cli_version_output() {
    assert_eq!(
        parse_codex_version("codex 1.2.3\n").expect("version"),
        "1.2.3"
    );
}

#[test]
fn rejects_malformed_codex_cli_version_output() {
    assert!(parse_codex_version("codex\n").is_err());
}
