use super::*;
use crate::config::Config;
use crate::config::ConfigOverrides;
use crate::config::Constrained;
use crate::config::ManagedFeatures;
use crate::config::NetworkProxySpec;
use crate::config::test_config;
use crate::guardian::approval_request::guardian_request_target_item_id;
use crate::session::session::Session;
use crate::session::turn_context::TurnContext;
use crate::test_support;
use codex_analytics::GuardianApprovalRequestSource;
use codex_config::ConfigLayerStack;
use codex_config::FeatureRequirementsToml;
use codex_config::NetworkConstraints;
use codex_config::NetworkDomainPermissionToml;
use codex_config::NetworkDomainPermissionsToml;
use codex_config::RequirementSource;
use codex_config::Sourced;
use codex_config::config_toml::ConfigToml;
use codex_config::types::McpServerConfig;
use codex_exec_server::LOCAL_FS;
use codex_features::Feature;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::DEEPSEEK_PROVIDER_ID;
use codex_models_manager::manager::StaticModelsManager;
use codex_network_proxy::NetworkProxyConfig;
use codex_protocol::ThreadId;
use codex_protocol::approvals::NetworkApprovalProtocol;
use codex_protocol::config_types::ApprovalsReviewer;
use codex_protocol::models::ContentItem;
use codex_protocol::models::PermissionProfile;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ModelsResponse;
use codex_protocol::permissions::FileSystemAccessMode;
use codex_protocol::permissions::FileSystemPath;
use codex_protocol::permissions::FileSystemSandboxEntry;
use codex_protocol::permissions::FileSystemSandboxPolicy;
use codex_protocol::permissions::NetworkSandboxPolicy;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::Event;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::GranularApprovalConfig;
use codex_protocol::protocol::GuardianAssessmentStatus;
use codex_protocol::protocol::GuardianRiskLevel;
use codex_protocol::protocol::GuardianUserAuthorization;
use codex_protocol::protocol::ReviewDecision;
use core_test_support::PathBufExt;
use core_test_support::TempDirExt;
use core_test_support::responses::mount_anthropic_response_once;
use core_test_support::responses::mount_anthropic_response_sequence;
use core_test_support::responses::mount_anthropic_sse_once;
use core_test_support::responses::mount_anthropic_sse_sequence;
use core_test_support::responses::sse_anthropic_message;
use core_test_support::responses::sse_anthropic_no_message;
use core_test_support::responses::sse_response;
use core_test_support::responses::start_mock_server;
use core_test_support::skip_if_no_network;
use core_test_support::test_path_buf;
use insta::Settings;
use insta::assert_snapshot;
use pretty_assertions::assert_eq;
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

fn fixed_guardian_parent_session_id() -> ThreadId {
    ThreadId::from_string("11111111-1111-4111-8111-111111111111")
        .expect("fixed parent session id should be a valid UUID")
}

#[test]
fn guardian_rejection_circuit_breaker_interrupts_after_three_consecutive_denials() {
    let mut circuit_breaker = GuardianRejectionCircuitBreaker::default();
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::Continue
    );
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::Continue
    );
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::InterruptTurn {
            consecutive_denials: 3,
            recent_denials: 3,
        }
    );
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::Continue
    );
}

#[test]
fn guardian_rejection_circuit_breaker_resets_consecutive_denials_on_non_denial() {
    let mut circuit_breaker = GuardianRejectionCircuitBreaker::default();
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::Continue
    );
    circuit_breaker.record_non_denial("turn-1");
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::Continue
    );
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::Continue
    );
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::InterruptTurn {
            consecutive_denials: 3,
            recent_denials: 4,
        }
    );
}

#[test]
fn auto_review_rejection_circuit_breaker_interrupts_after_ten_recent_denials() {
    let mut circuit_breaker = GuardianRejectionCircuitBreaker::default();
    for _ in 0..9 {
        assert_eq!(
            circuit_breaker.record_denial("turn-1"),
            GuardianRejectionCircuitBreakerAction::Continue
        );
        circuit_breaker.record_non_denial("turn-1");
    }
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::InterruptTurn {
            consecutive_denials: 1,
            recent_denials: 10,
        }
    );
}

#[test]
fn auto_review_rejection_circuit_breaker_forgets_denials_outside_recent_review_window() {
    let mut circuit_breaker = GuardianRejectionCircuitBreaker::default();
    for _ in 0..9 {
        assert_eq!(
            circuit_breaker.record_denial("turn-1"),
            GuardianRejectionCircuitBreakerAction::Continue
        );
        circuit_breaker.record_non_denial("turn-1");
    }
    for _ in 0..(AUTO_REVIEW_DENIAL_WINDOW_SIZE - 18) {
        circuit_breaker.record_non_denial("turn-1");
    }
    assert_eq!(
        circuit_breaker.record_denial("turn-1"),
        GuardianRejectionCircuitBreakerAction::Continue
    );
}

async fn guardian_test_session_and_turn(
    server: &wiremock::MockServer,
) -> (Arc<Session>, Arc<TurnContext>) {
    guardian_test_session_and_turn_with_base_url(server.uri().as_str()).await
}

async fn guardian_test_session_turn_and_rx(
    server: &wiremock::MockServer,
) -> (
    Arc<Session>,
    Arc<TurnContext>,
    async_channel::Receiver<Event>,
) {
    let (mut session, mut turn, rx) =
        crate::session::tests::make_session_and_context_with_rx().await;
    Arc::get_mut(&mut session)
        .expect("session should be uniquely owned")
        .thread_id = fixed_guardian_parent_session_id();
    let mut config = (*turn.config).clone();
    config.model_provider.base_url = Some(format!("{}/v1", server.uri()));
    let config = Arc::new(config);
    let models_manager = test_support::models_manager_with_provider(
        config.codex_home.to_path_buf(),
        Arc::clone(&session.services.auth_manager),
        config.model_provider.clone(),
    );
    Arc::get_mut(&mut session)
        .expect("session should be uniquely owned")
        .services
        .models_manager = models_manager;
    let turn_mut = Arc::get_mut(&mut turn).expect("turn should be uniquely owned");
    turn_mut.config = Arc::clone(&config);
    turn_mut.provider =
        create_model_provider(config.model_provider.clone(), turn_mut.auth_manager.clone());

    (session, turn, rx)
}

fn guardian_shell_request(id: &str) -> GuardianApprovalRequest {
    GuardianApprovalRequest::Shell {
        id: id.to_string(),
        command: vec!["git".to_string(), "push".to_string()],
        cwd: test_path_buf("/repo/codex-rs/core").abs(),
        sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
        additional_permissions: None,
        justification: Some("Need to push the reviewed docs fix.".to_string()),
    }
}

async fn guardian_test_session_and_turn_with_base_url(
    base_url: &str,
) -> (Arc<Session>, Arc<TurnContext>) {
    let (mut session, mut turn) = crate::session::tests::make_session_and_context().await;
    session.thread_id = fixed_guardian_parent_session_id();
    let mut config = (*turn.config).clone();
    config.model_provider.base_url = Some(format!("{base_url}/v1"));
    let config = Arc::new(config);
    let models_manager = test_support::models_manager_with_provider(
        config.codex_home.to_path_buf(),
        Arc::clone(&session.services.auth_manager),
        config.model_provider.clone(),
    );
    session.services.models_manager = models_manager;
    turn.config = Arc::clone(&config);
    turn.provider = create_model_provider(config.model_provider.clone(), turn.auth_manager.clone());

    (Arc::new(session), Arc::new(turn))
}

async fn seed_guardian_parent_history(session: &Arc<Session>, turn: &Arc<TurnContext>) {
    session
        .record_conversation_items(
            turn.as_ref(),
            &[
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "Please check the repo visibility and push the docs fix if needed."
                            .to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                ResponseItem::FunctionCall {
                    id: None,
                    name: "gh_repo_view".to_string(),
                    namespace: None,
                    arguments: "{\"repo\":\"openai/codex\"}".to_string(),
                    call_id: "call-1".to_string(),
                    internal_chat_message_metadata_passthrough: None,
                },
                ResponseItem::FunctionCallOutput {
                    id: None,
                    call_id: "call-1".to_string(),
                    output: codex_protocol::models::FunctionCallOutputPayload::from_text(
                        "repo visibility: public".to_string(),
                    ),
                    internal_chat_message_metadata_passthrough: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "The repo is public; I now need approval to push the docs fix."
                            .to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
        )
        .await;
}

fn normalize_guardian_snapshot_paths(text: String) -> String {
    let mut text = text;
    for canonical_path in ["/repo/codex-rs/core", "/repo"] {
        let platform_path = test_path_buf(canonical_path).display().to_string();
        if platform_path == canonical_path {
            continue;
        }

        let escaped_platform_path = serde_json::to_string(&platform_path)
            .expect("test path should serialize")
            .trim_matches('"')
            .to_string();
        text = text
            .replace(&escaped_platform_path, canonical_path)
            .replace(&platform_path, canonical_path);
    }
    text
}

fn guardian_prompt_text(items: &[codex_protocol::user_input::UserInput]) -> String {
    items
        .iter()
        .map(|item| match item {
            codex_protocol::user_input::UserInput::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect::<String>()
}

#[test]
fn build_guardian_transcript_keeps_original_numbering() {
    let entries = [
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::User,
            text: "first".to_string(),
        },
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::Assistant,
            text: "second".to_string(),
        },
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::Assistant,
            text: "third".to_string(),
        },
    ];

    let (transcript, omission) = render_guardian_transcript_entries(&entries[..2]);

    assert_eq!(
        transcript,
        vec![
            "[1] user: first".to_string(),
            "[2] assistant: second".to_string()
        ]
    );
    assert!(omission.is_none());
}

#[tokio::test(flavor = "current_thread")]
async fn build_guardian_prompt_full_mode_preserves_initial_review_format() -> anyhow::Result<()> {
    let (session, turn) = guardian_test_session_and_turn_with_base_url("http://localhost").await;
    seed_guardian_parent_history(&session, &turn).await;

    let prompt = build_guardian_prompt_items(
        session.as_ref(),
        Some("Sandbox denied outbound git push to github.com.".to_string()),
        GuardianApprovalRequest::Shell {
            id: "shell-1".to_string(),
            command: vec!["git".to_string(), "push".to_string()],
            cwd: test_path_buf("/repo/codex-rs/core").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: Some("Need to push the reviewed docs fix.".to_string()),
        },
        GuardianPromptMode::Full,
    )
    .await?;

    let text = guardian_prompt_text(&prompt.items);
    assert!(text.contains("whose request action you are assessing"));
    assert!(text.contains(">>> TRANSCRIPT START\n"));
    assert!(text.contains(">>> TRANSCRIPT END\n"));
    assert!(text.contains("The Codex agent has requested the following action:\n"));
    assert!(!text.contains("TRANSCRIPT DELTA"));
    assert_eq!(prompt.transcript_cursor.transcript_entry_count, 4);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn build_guardian_prompt_includes_parent_turn_denied_reads() -> anyhow::Result<()> {
    let (mut session, mut turn) = crate::session::tests::make_session_and_context().await;
    session.thread_id = fixed_guardian_parent_session_id();
    let denied_root = test_path_buf("/repo/private").abs();
    let denied_glob = test_path_buf("/repo/private/**").display().to_string();
    turn.permission_profile = PermissionProfile::from_runtime_permissions(
        &FileSystemSandboxPolicy::restricted(vec![
            FileSystemSandboxEntry {
                path: FileSystemPath::Special {
                    value: codex_protocol::permissions::FileSystemSpecialPath::Root,
                },
                access: FileSystemAccessMode::Read,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::Path {
                    path: denied_root.clone(),
                },
                access: FileSystemAccessMode::Deny,
            },
            FileSystemSandboxEntry {
                path: FileSystemPath::GlobPattern {
                    pattern: denied_glob.clone(),
                },
                access: FileSystemAccessMode::Deny,
            },
        ]),
        NetworkSandboxPolicy::Restricted,
    );
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    seed_guardian_parent_history(&session, &turn).await;

    let prompt = build_guardian_prompt_items_with_parent_turn(
        session.as_ref(),
        Some(turn.as_ref()),
        Some("Sandbox denied reading /repo/private/secret.txt.".to_string()),
        GuardianApprovalRequest::Shell {
            id: "shell-1".to_string(),
            command: vec!["cat".to_string(), "/repo/private/secret.txt".to_string()],
            cwd: test_path_buf("/repo").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::RequireEscalated,
            additional_permissions: None,
            justification: Some("Need to inspect the secret file.".to_string()),
        },
        GuardianPromptMode::Full,
    )
    .await?;

    let text = guardian_prompt_text(&prompt.items);
    assert!(text.contains("PARENT TURN PERMISSION CONTEXT START"));
    assert!(text.contains("do not approve escalation whose purpose is to read them"));
    assert!(text.contains(denied_root.to_string_lossy().as_ref()));
    assert!(text.contains(&format!("glob `{denied_glob}`")));

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn build_guardian_prompt_delta_mode_preserves_original_numbering() -> anyhow::Result<()> {
    let (session, turn) = guardian_test_session_and_turn_with_base_url("http://localhost").await;
    seed_guardian_parent_history(&session, &turn).await;
    session
        .record_conversation_items(
            turn.as_ref(),
            &[
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "Please also push the second docs fix.".to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "I need approval for the second push.".to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
        )
        .await;

    let prompt = build_guardian_prompt_items(
        session.as_ref(),
        /*retry_reason*/ None,
        GuardianApprovalRequest::Shell {
            id: "shell-2".to_string(),
            command: vec!["git".to_string(), "push".to_string()],
            cwd: test_path_buf("/repo/codex-rs/core").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: Some("Need to push the second docs fix.".to_string()),
        },
        GuardianPromptMode::Delta {
            cursor: GuardianTranscriptCursor {
                parent_history_version: 0,
                transcript_entry_count: 4,
            },
        },
    )
    .await?;

    let text = guardian_prompt_text(&prompt.items);
    assert!(text.contains("added since your last approval assessment"));
    assert!(text.contains(">>> TRANSCRIPT DELTA START\n"));
    assert!(text.contains("[5] user: Please also push the second docs fix."));
    assert!(text.contains("[6] assistant: I need approval for the second push."));
    assert!(text.contains(">>> TRANSCRIPT DELTA END\n"));
    assert!(text.contains("The Codex agent has requested the following next action:\n"));
    assert!(!text.contains("[1] user: Please check the repo visibility"));
    assert_eq!(prompt.transcript_cursor.transcript_entry_count, 6);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn build_guardian_prompt_delta_mode_handles_empty_delta() -> anyhow::Result<()> {
    let (session, turn) = guardian_test_session_and_turn_with_base_url("http://localhost").await;
    seed_guardian_parent_history(&session, &turn).await;

    let prompt = build_guardian_prompt_items(
        session.as_ref(),
        /*retry_reason*/ None,
        GuardianApprovalRequest::Shell {
            id: "shell-2".to_string(),
            command: vec!["git".to_string(), "push".to_string()],
            cwd: test_path_buf("/repo/codex-rs/core").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: Some("Need to push the second docs fix.".to_string()),
        },
        GuardianPromptMode::Delta {
            cursor: GuardianTranscriptCursor {
                parent_history_version: 0,
                transcript_entry_count: 4,
            },
        },
    )
    .await?;

    let text = guardian_prompt_text(&prompt.items);
    assert!(text.contains(">>> TRANSCRIPT DELTA START\n"));
    assert!(text.contains("<no retained transcript delta entries>"));
    assert!(text.contains(">>> TRANSCRIPT DELTA END\n"));
    assert_eq!(prompt.transcript_cursor.transcript_entry_count, 4);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn build_guardian_prompt_stale_delta_cursor_falls_back_to_full_prompt() -> anyhow::Result<()>
{
    let (session, turn) = guardian_test_session_and_turn_with_base_url("http://localhost").await;
    seed_guardian_parent_history(&session, &turn).await;

    let prompt = build_guardian_prompt_items(
        session.as_ref(),
        /*retry_reason*/ None,
        GuardianApprovalRequest::Shell {
            id: "shell-3".to_string(),
            command: vec!["git".to_string(), "push".to_string()],
            cwd: test_path_buf("/repo/codex-rs/core").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: Some("Need to push the docs fix.".to_string()),
        },
        GuardianPromptMode::Delta {
            cursor: GuardianTranscriptCursor {
                parent_history_version: 0,
                transcript_entry_count: 99,
            },
        },
    )
    .await?;

    let text = guardian_prompt_text(&prompt.items);
    assert!(text.contains("whose request action you are assessing"));
    assert!(text.contains(">>> TRANSCRIPT START\n"));
    assert!(!text.contains("TRANSCRIPT DELTA"));
    assert_eq!(prompt.transcript_cursor.transcript_entry_count, 4);

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn build_guardian_prompt_stale_delta_version_falls_back_to_full_prompt() -> anyhow::Result<()>
{
    let (session, turn) = guardian_test_session_and_turn_with_base_url("http://localhost").await;
    seed_guardian_parent_history(&session, &turn).await;
    session
        .replace_history(
            vec![
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "Compacted retained user request.".to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "Compacted summary of earlier guardian context.".to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
            /*reference_context_item*/ None,
        )
        .await;
    session
        .record_conversation_items(
            turn.as_ref(),
            &[
                ResponseItem::Message {
                    id: None,
                    role: "user".to_string(),
                    content: vec![ContentItem::InputText {
                        text: "Please push after the compaction.".to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
                ResponseItem::Message {
                    id: None,
                    role: "assistant".to_string(),
                    content: vec![ContentItem::OutputText {
                        text: "I need approval for the post-compaction push.".to_string(),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: None,
                },
            ],
        )
        .await;

    let prompt = build_guardian_prompt_items(
        session.as_ref(),
        /*retry_reason*/ None,
        GuardianApprovalRequest::Shell {
            id: "shell-4".to_string(),
            command: vec!["git".to_string(), "push".to_string()],
            cwd: test_path_buf("/repo/codex-rs/core").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: Some("Need to push after the compaction.".to_string()),
        },
        GuardianPromptMode::Delta {
            cursor: GuardianTranscriptCursor {
                parent_history_version: 0,
                transcript_entry_count: 4,
            },
        },
    )
    .await?;

    let text = guardian_prompt_text(&prompt.items);
    assert!(text.contains("whose request action you are assessing"));
    assert!(text.contains(">>> TRANSCRIPT START\n"));
    assert!(!text.contains("TRANSCRIPT DELTA"));
    assert!(text.contains("[3] user: Please push after the compaction."));
    assert!(text.contains("[4] assistant: I need approval for the post-compaction push."));
    assert_eq!(prompt.transcript_cursor.parent_history_version, 1);
    assert_eq!(prompt.transcript_cursor.transcript_entry_count, 4);

    Ok(())
}

#[test]
fn collect_guardian_transcript_entries_skips_contextual_user_messages() {
    let items = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "<environment_context>\n<cwd>/tmp</cwd>\n</environment_context>".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "hello".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    let entries = collect_guardian_transcript_entries(&items);

    assert_eq!(entries.len(), 1);
    assert_eq!(
        entries[0],
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::Assistant,
            text: "hello".to_string(),
        }
    );
}

#[test]
fn collect_guardian_transcript_entries_keeps_manual_approval_developer_message() {
    let approval_text =
        format!("{AUTO_REVIEW_DENIED_ACTION_APPROVAL_DEVELOPER_PREFIX}\n\nApproved action:\n{{}}");
    let items = vec![
        ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: "ordinary developer context".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "developer".to_string(),
            content: vec![ContentItem::InputText {
                text: approval_text.clone(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    let entries = collect_guardian_transcript_entries(&items);

    assert_eq!(
        entries,
        vec![GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::Developer,
            text: approval_text,
        }]
    );
}

#[test]
fn collect_guardian_transcript_entries_includes_recent_tool_calls_and_output() {
    let items = vec![
        ResponseItem::Message {
            id: None,
            role: "user".to_string(),
            content: vec![ContentItem::InputText {
                text: "check the repo".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCall {
            id: None,
            name: "read_file".to_string(),
            namespace: None,
            arguments: "{\"path\":\"README.md\"}".to_string(),
            call_id: "call-1".to_string(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: None,
            call_id: "call-1".to_string(),
            output: codex_protocol::models::FunctionCallOutputPayload::from_text(
                "repo is public".to_string(),
            ),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::Message {
            id: None,
            role: "assistant".to_string(),
            content: vec![ContentItem::OutputText {
                text: "I need to push a fix".to_string(),
            }],
            phase: None,
            internal_chat_message_metadata_passthrough: None,
        },
    ];

    let entries = collect_guardian_transcript_entries(&items);

    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries[1],
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::Tool("tool read_file call".to_string()),
            text: "{\"path\":\"README.md\"}".to_string(),
        }
    );
    assert_eq!(
        entries[2],
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::Tool("tool read_file result".to_string()),
            text: "repo is public".to_string(),
        }
    );
}

#[test]
fn guardian_truncate_text_keeps_prefix_suffix_and_xml_marker() {
    let content = "prefix ".repeat(200) + &" suffix".repeat(200);

    let (truncated, was_truncated) = guardian_truncate_text(&content, /*token_cap*/ 20);

    assert!(truncated.starts_with("prefix"));
    assert!(truncated.contains("<truncated omitted_approx_tokens=\""));
    assert!(truncated.ends_with("suffix"));
    assert!(was_truncated);
}

#[test]
fn format_guardian_action_pretty_truncates_large_string_fields() -> serde_json::Result<()> {
    let patch = "line\n".repeat(100_000);
    let action = GuardianApprovalRequest::ApplyPatch {
        id: "patch-1".to_string(),
        cwd: test_path_buf("/tmp").abs(),
        files: Vec::new(),
        patch: patch.clone(),
    };

    let rendered = format_guardian_action_pretty(&action)?;

    assert!(rendered.text.contains("\"tool\": \"apply_patch\""));
    assert!(rendered.text.contains("<truncated omitted_approx_tokens="));
    assert!(rendered.text.len() < patch.len());
    assert!(rendered.truncated);
    Ok(())
}

#[test]
fn format_guardian_action_pretty_reports_no_truncation_for_small_payload() -> serde_json::Result<()>
{
    let action = GuardianApprovalRequest::ApplyPatch {
        id: "patch-1".to_string(),
        cwd: test_path_buf("/tmp").abs(),
        files: Vec::new(),
        patch: "line\n".to_string(),
    };

    let rendered = format_guardian_action_pretty(&action)?;

    assert!(rendered.text.contains("\"tool\": \"apply_patch\""));
    assert!(!rendered.truncated);
    Ok(())
}

#[test]
fn guardian_approval_request_to_json_renders_mcp_tool_call_shape() -> serde_json::Result<()> {
    let action = GuardianApprovalRequest::McpToolCall {
        id: "call-1".to_string(),
        server: "mcp_server".to_string(),
        tool_name: "browser_navigate".to_string(),
        arguments: Some(serde_json::json!({
            "url": "https://example.com",
        })),
        connector_id: None,
        connector_name: Some("Playwright".to_string()),
        connector_description: None,
        connected_account_email: Some("owner@example.com".to_string()),
        tool_title: Some("Navigate".to_string()),
        tool_description: None,
        annotations: Some(GuardianMcpAnnotations {
            destructive_hint: Some(true),
            open_world_hint: None,
            read_only_hint: Some(false),
        }),
    };

    assert_eq!(
        guardian_approval_request_to_json(&action)?,
        serde_json::json!({
            "tool": "mcp_tool_call",
            "server": "mcp_server",
            "tool_name": "browser_navigate",
            "arguments": {
                "url": "https://example.com",
            },
            "connector_name": "Playwright",
            "connected_account_email": "owner@example.com",
            "tool_title": "Navigate",
            "annotations": {
                "destructive_hint": true,
                "read_only_hint": false,
            },
        })
    );
    Ok(())
}

#[test]
fn guardian_approval_request_to_json_renders_network_access_trigger() -> serde_json::Result<()> {
    let cwd = test_path_buf("/repo").abs();
    let action = GuardianApprovalRequest::NetworkAccess {
        id: "network-1".to_string(),
        turn_id: "turn-1".to_string(),
        target: "https://example.com:443".to_string(),
        host: "example.com".to_string(),
        protocol: NetworkApprovalProtocol::Https,
        port: 443,
        trigger: Some(GuardianNetworkAccessTrigger {
            call_id: "call-1".to_string(),
            tool_name: "shell".to_string(),
            command: vec!["curl".to_string(), "https://example.com".to_string()],
            cwd: cwd.clone(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: Some("Fetch the release metadata.".to_string()),
            tty: None,
        }),
    };

    assert_eq!(
        guardian_approval_request_to_json(&action)?,
        serde_json::json!({
            "tool": "network_access",
            "target": "https://example.com:443",
            "host": "example.com",
            "protocol": "https",
            "port": 443,
            "trigger": {
                "callId": "call-1",
                "toolName": "shell",
                "command": ["curl", "https://example.com"],
                "cwd": cwd.to_string_lossy().to_string(),
                "sandboxPermissions": "use_default",
                "justification": "Fetch the release metadata.",
            },
        })
    );

    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn build_guardian_prompt_items_explains_network_access_review_scope() -> anyhow::Result<()> {
    let (session, turn) = guardian_test_session_and_turn_with_base_url("http://localhost").await;
    seed_guardian_parent_history(&session, &turn).await;
    let cwd = test_path_buf("/repo").abs();

    let prompt = build_guardian_prompt_items(
        session.as_ref(),
        Some("Network access to \"example.com\" is blocked by policy.".to_string()),
        GuardianApprovalRequest::NetworkAccess {
            id: "network-1".to_string(),
            turn_id: "turn-1".to_string(),
            target: "https://example.com:443".to_string(),
            host: "example.com".to_string(),
            protocol: NetworkApprovalProtocol::Https,
            port: 443,
            trigger: Some(GuardianNetworkAccessTrigger {
                call_id: "call-1".to_string(),
                tool_name: "shell".to_string(),
                command: vec!["curl".to_string(), "https://example.com".to_string()],
                cwd,
                sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
                additional_permissions: None,
                justification: Some("Fetch the release metadata.".to_string()),
                tty: None,
            }),
        },
        GuardianPromptMode::Full,
    )
    .await?;

    let text = guardian_prompt_text(&prompt.items);
    assert!(text.contains("Below is a proposed network access request under review."));
    assert!(!text.contains("Network approval context:"));
    assert!(
        !text.contains(
            "This approval request is about network access to the target in the network access JSON below"
        )
    );
    assert!(
        text.contains(
            "When assessing this request, focus primarily on whether the triggering command is authorised by the user and whether it is within the rules."
        )
    );
    assert!(
        text.contains(
            "The user does not need to have explicitly authorised this exact network connection, as long as the network access is a reasonable consequence of the triggering command."
        )
    );
    assert!(text.contains("\"trigger\""));
    assert!(text.contains("Network access JSON:"));
    assert!(!text.contains("The Codex agent has requested the following action:"));
    assert!(!text.contains("Planned action JSON:"));
    assert!(!text.contains("Retry reason:"));
    assert!(!text.contains("Network access to \"example.com\" is blocked by policy."));

    let mut settings = Settings::clone_current();
    settings.set_snapshot_path("snapshots");
    settings.set_prepend_module_to_snapshot(false);
    settings.bind(|| {
        assert_snapshot!(
            "codex_core__guardian__tests__network_access_guardian_prompt_layout",
            normalize_guardian_snapshot_paths(text)
        );
    });

    Ok(())
}

#[test]
fn guardian_assessment_action_redacts_apply_patch_patch_text() {
    let cwd = test_path_buf("/tmp").abs();
    let file = test_path_buf("/tmp/guardian.txt").abs();
    let action = GuardianApprovalRequest::ApplyPatch {
        id: "patch-1".to_string(),
        cwd: cwd.clone(),
        files: vec![file.clone()],
        patch: "*** Begin Patch\n*** Update File: guardian.txt\n@@\n+secret\n*** End Patch"
            .to_string(),
    };

    assert_eq!(
        serde_json::to_value(guardian_assessment_action(&action)).expect("serialize action"),
        serde_json::json!({
            "type": "apply_patch",
            "cwd": cwd,
            "files": [file],
        }),
    );
}

#[test]
fn guardian_request_turn_id_prefers_network_access_owner_turn() {
    let network_access = GuardianApprovalRequest::NetworkAccess {
        id: "network-1".to_string(),
        turn_id: "owner-turn".to_string(),
        target: "https://example.com:443".to_string(),
        host: "example.com".to_string(),
        protocol: NetworkApprovalProtocol::Https,
        port: 443,
        trigger: None,
    };
    let apply_patch = GuardianApprovalRequest::ApplyPatch {
        id: "patch-1".to_string(),
        cwd: test_path_buf("/tmp").abs(),
        files: vec![test_path_buf("/tmp/guardian.txt").abs()],
        patch: "*** Begin Patch\n*** Update File: guardian.txt\n@@\n+hello\n*** End Patch"
            .to_string(),
    };

    assert_eq!(
        guardian_request_turn_id(&network_access, "fallback-turn"),
        "owner-turn"
    );
    assert_eq!(
        guardian_request_turn_id(&apply_patch, "fallback-turn"),
        "fallback-turn"
    );
}

#[test]
fn guardian_request_target_item_id_omits_network_access_trigger_call_id() {
    let network_access = GuardianApprovalRequest::NetworkAccess {
        id: "network-1".to_string(),
        turn_id: "owner-turn".to_string(),
        target: "https://example.com:443".to_string(),
        host: "example.com".to_string(),
        protocol: NetworkApprovalProtocol::Https,
        port: 443,
        trigger: Some(GuardianNetworkAccessTrigger {
            call_id: "call-1".to_string(),
            tool_name: "shell".to_string(),
            command: vec!["curl".to_string(), "https://example.com".to_string()],
            cwd: test_path_buf("/repo").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: None,
            tty: None,
        }),
    };

    assert_eq!(guardian_request_target_item_id(&network_access), None);
}

#[tokio::test]
async fn cancelled_guardian_review_emits_terminal_abort_without_warning() {
    let (session, turn, rx) = crate::session::tests::make_session_and_context_with_rx().await;
    let cancel_token = CancellationToken::new();
    cancel_token.cancel();

    let decision = review_approval_request_with_cancel(
        &session,
        &turn,
        "review-cancelled-guardian".to_string(),
        GuardianApprovalRequest::ApplyPatch {
            id: "patch-1".to_string(),
            cwd: test_path_buf("/tmp").abs(),
            files: vec![test_path_buf("/tmp/guardian.txt").abs()],
            patch: "*** Begin Patch\n*** Update File: guardian.txt\n@@\n+hello\n*** End Patch"
                .to_string(),
        },
        /*retry_reason*/ None,
        GuardianApprovalRequestSource::MainTurn,
        cancel_token,
    )
    .await;

    assert_eq!(decision, ReviewDecision::Abort);

    let mut guardian_statuses = Vec::new();
    let mut warnings = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event.msg {
            EventMsg::GuardianAssessment(event) => guardian_statuses.push(event.status),
            EventMsg::GuardianWarning(event) => warnings.push(event.message),
            _ => {}
        }
    }

    assert_eq!(
        guardian_statuses,
        vec![
            GuardianAssessmentStatus::InProgress,
            GuardianAssessmentStatus::Aborted,
        ]
    );
    assert!(warnings.is_empty());
}

#[test]
fn guardian_timeout_message_distinguishes_timeout_from_policy_denial() {
    let message = guardian_timeout_message();
    assert!(message.contains("did not finish before its deadline"));
    assert!(message.contains("retry once"));
    assert!(!message.contains("unacceptable risk"));
}

#[tokio::test]
async fn routes_approval_to_guardian_requires_guardian_reviewer() {
    let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
    let mut config = (*turn.config).clone();
    config.approvals_reviewer = ApprovalsReviewer::User;
    turn.config = Arc::new(config.clone());

    assert!(!routes_approval_to_guardian(&turn));

    config.approvals_reviewer = ApprovalsReviewer::AutoReview;
    turn.config = Arc::new(config);

    assert!(routes_approval_to_guardian(&turn));
}

#[tokio::test]
async fn routes_approval_to_guardian_can_use_app_reviewer_override() {
    let (_session, turn) = crate::session::tests::make_session_and_context().await;

    assert!(!routes_approval_to_guardian_with_reviewer(
        &turn,
        ApprovalsReviewer::User
    ));
    assert!(routes_approval_to_guardian_with_reviewer(
        &turn,
        ApprovalsReviewer::AutoReview
    ));
}

#[tokio::test]
async fn routes_approval_to_guardian_allows_granular_review_policy() {
    let (_session, mut turn) = crate::session::tests::make_session_and_context().await;
    let mut config = (*turn.config).clone();
    config.approvals_reviewer = ApprovalsReviewer::AutoReview;
    turn.config = Arc::new(config);
    turn.approval_policy
        .set(AskForApproval::Granular(GranularApprovalConfig {
            sandbox_approval: true,
            rules: true,
            skill_approval: true,
            request_permissions: true,
            mcp_elicitations: true,
        }))
        .expect("test setup should allow updating approval policy");

    assert!(routes_approval_to_guardian(&turn));
}

#[test]
fn build_guardian_transcript_reserves_separate_budget_for_tool_evidence() {
    let repeated = "signal ".repeat(8_000);
    let mut entries = vec![
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::User,
            text: "please figure out if the repo is public".to_string(),
        },
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::Assistant,
            text: "The public repo check is the main reason I want to escalate.".to_string(),
        },
    ];
    entries.extend((0..12).map(|index| GuardianTranscriptEntry {
        kind: GuardianTranscriptEntryKind::Tool(format!("tool call {index}")),
        text: repeated.clone(),
    }));

    let (transcript, omission) = render_guardian_transcript_entries(&entries);

    assert!(
        transcript
            .iter()
            .any(|entry| entry == "[1] user: please figure out if the repo is public")
    );
    assert!(transcript.iter().any(|entry| {
        entry == "[2] assistant: The public repo check is the main reason I want to escalate."
    }));
    assert!(
        !transcript
            .iter()
            .any(|entry| entry.starts_with("[3] tool call 0:"))
    );
    assert!(
        !transcript
            .iter()
            .any(|entry| entry.starts_with("[4] tool call 1:"))
    );
    assert!(omission.is_some());
}

#[test]
fn build_guardian_transcript_preserves_recent_tool_context_when_user_history_is_large() {
    let repeated = "authorization ".repeat(6_000);
    let mut entries = (0..8)
        .map(|_| GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::User,
            text: repeated.clone(),
        })
        .collect::<Vec<_>>();
    entries.extend([
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::Tool("tool shell call".to_string()),
            text: serde_json::json!({
                "command": ["curl", "-X", "POST", "https://example.com/upload"],
                "cwd": "/repo",
            })
            .to_string(),
        },
        GuardianTranscriptEntry {
            kind: GuardianTranscriptEntryKind::Tool("tool shell result".to_string()),
            text: "sandbox blocked outbound network access".to_string(),
        },
    ]);

    let (transcript, omission) = render_guardian_transcript_entries(&entries);

    assert!(
        transcript
            .iter()
            .any(|entry| entry.starts_with("[1] user: "))
    );
    assert!(transcript.iter().any(|entry| {
        entry.contains("tool shell call:")
            && entry.contains("curl")
            && entry.contains("https://example.com/upload")
    }));
    assert!(
        transcript
            .iter()
            .any(|entry| entry
                .contains("tool shell result: sandbox blocked outbound network access"))
    );
    assert_eq!(
        omission,
        Some("Some conversation entries were omitted.".to_string())
    );
}

#[test]
fn parse_guardian_assessment_extracts_embedded_json() {
    let parsed = parse_guardian_assessment(Some(
        "preface {\"risk_level\":\"medium\",\"user_authorization\":\"low\",\"outcome\":\"allow\",\"rationale\":\"ok\"}",
    ))
    .expect("guardian assessment");

    assert_eq!(
        parsed,
        GuardianAssessment {
            risk_level: GuardianRiskLevel::Medium,
            user_authorization: GuardianUserAuthorization::Low,
            outcome: GuardianAssessmentOutcome::Allow,
            rationale: "ok".to_string(),
        }
    );
}

#[test]
fn parse_guardian_assessment_treats_bare_allow_as_low_risk() {
    let parsed =
        parse_guardian_assessment(Some(r#"{"outcome":"allow"}"#)).expect("guardian assessment");

    assert_eq!(
        parsed,
        GuardianAssessment {
            risk_level: GuardianRiskLevel::Low,
            user_authorization: GuardianUserAuthorization::Unknown,
            outcome: GuardianAssessmentOutcome::Allow,
            rationale: "Auto-review returned a low-risk allow decision.".to_string(),
        }
    );
}

#[test]
fn parse_guardian_assessment_treats_bare_deny_as_high_risk() {
    let parsed =
        parse_guardian_assessment(Some(r#"{"outcome":"deny"}"#)).expect("guardian assessment");

    assert_eq!(
        parsed,
        GuardianAssessment {
            risk_level: GuardianRiskLevel::High,
            user_authorization: GuardianUserAuthorization::Unknown,
            outcome: GuardianAssessmentOutcome::Deny,
            rationale: "Auto-review returned a deny decision without a rationale.".to_string(),
        }
    );
}

#[test]
fn guardian_output_schema_requires_only_outcome_and_allows_optional_details() {
    let schema = guardian_output_schema();

    assert_eq!(
        schema,
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "risk_level": {
                    "type": "string",
                    "enum": ["low", "medium", "high", "critical"]
                },
                "user_authorization": {
                    "type": "string",
                    "enum": ["unknown", "low", "medium", "high"]
                },
                "outcome": {
                    "type": "string",
                    "enum": ["allow", "deny"]
                },
                "rationale": {
                    "type": "string"
                }
            },
            "required": ["outcome"]
        })
    );
}

enum GuardianTestCatalog {
    Bundled,
    ParentOnly,
}

async fn guardian_request_model_for_auto_review(
    auto_review_model_override: Option<String>,
    catalog: GuardianTestCatalog,
) -> anyhow::Result<(
    String,
    String,
    String,
    codex_analytics::GuardianReviewAnalyticsResult,
)> {
    let server = start_mock_server().await;
    let guardian_assessment = serde_json::json!({
        "outcome": "allow",
    })
    .to_string();
    let request_log = mount_anthropic_sse_once(
        &server,
        sse_anthropic_message(&guardian_assessment),
    )
    .await;

    let (mut session, mut turn) = guardian_test_session_and_turn(&server).await;
    match catalog {
        GuardianTestCatalog::Bundled => {}
        GuardianTestCatalog::ParentOnly => {
            let parent_model = turn.model_info.clone();
            let auth_manager = Arc::clone(&session.services.auth_manager);
            let models_manager = StaticModelsManager::new(
                Some(auth_manager),
                ModelsResponse {
                    models: vec![parent_model],
                },
            );
            Arc::get_mut(&mut session)
                .expect("session should be unique")
                .services
                .models_manager = Arc::new(models_manager);
        }
    }
    Arc::get_mut(&mut turn)
        .expect("turn should be unique")
        .model_info
        .auto_review_model_override = auto_review_model_override;
    let parent_model = turn.model_info.slug.clone();
    let preferred_model = turn.provider.approval_review_preferred_model().to_string();
    seed_guardian_parent_history(&session, &turn).await;

    let (outcome, analytics_result) = run_guardian_review_session_for_test(
        Arc::clone(&session),
        turn,
        GuardianApprovalRequest::Shell {
            id: "shell-1".to_string(),
            command: vec!["git".to_string(), "push".to_string()],
            cwd: test_path_buf("/repo/codex-rs/core").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: None,
        },
        Some("Sandbox denied outbound git push to github.com.".to_string()),
        guardian_output_schema(),
        /*external_cancel*/ None,
        /*max_attempts*/ 1,
    )
    .await;
    let GuardianReviewOutcome::Completed(_) = outcome else {
        panic!("expected guardian assessment");
    };

    let request = request_log.single_request();
    let request_model = request
        .body_json()
        .get("model")
        .and_then(|value| value.as_str())
        .expect("guardian request should include a model")
        .to_string();

    Ok((
        request_model,
        parent_model,
        preferred_model,
        analytics_result,
    ))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_review_uses_model_catalog_override_when_preferred_review_model_exists()
-> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let override_model = "guardian-review-model-override".to_string();
    let (request_model, parent_model, preferred_model, analytics_result) =
        guardian_request_model_for_auto_review(
            Some(override_model.clone()),
            GuardianTestCatalog::Bundled,
        )
        .await?;

    assert_eq!(request_model, override_model);
    assert_ne!(request_model, parent_model);
    assert_ne!(request_model, preferred_model);
    assert_eq!(
        analytics_result.guardian_catalog_contains_auto_review,
        Some(true)
    );
    assert_eq!(
        analytics_result.guardian_default_review_model_id.as_deref(),
        Some(preferred_model.as_str())
    );
    assert_eq!(
        analytics_result.guardian_review_model_overridden,
        Some(true)
    );
    assert_eq!(
        analytics_result.guardian_review_model_override.as_deref(),
        Some(override_model.as_str())
    );
    assert_eq!(
        analytics_result.guardian_model_provider_id.as_deref(),
        Some(DEEPSEEK_PROVIDER_ID)
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_review_uses_preferred_review_model_without_model_catalog_override()
-> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let (request_model, parent_model, preferred_model, analytics_result) =
        guardian_request_model_for_auto_review(
            /*auto_review_model_override*/ None,
            GuardianTestCatalog::Bundled,
        )
        .await?;

    assert_eq!(request_model, preferred_model);
    assert_ne!(request_model, parent_model);
    assert_eq!(
        analytics_result.guardian_catalog_contains_auto_review,
        Some(true)
    );
    assert_eq!(
        analytics_result.guardian_default_review_model_id.as_deref(),
        Some(preferred_model.as_str())
    );
    assert_eq!(
        analytics_result.guardian_review_model_overridden,
        Some(false)
    );
    assert_eq!(
        analytics_result.guardian_review_model_override.as_deref(),
        None
    );
    assert_eq!(
        analytics_result.guardian_model_provider_id.as_deref(),
        Some(DEEPSEEK_PROVIDER_ID)
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_review_records_missing_auto_review_model_in_analytics_metadata()
-> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let (request_model, parent_model, preferred_model, analytics_result) =
        guardian_request_model_for_auto_review(
            /*auto_review_model_override*/ None,
            GuardianTestCatalog::ParentOnly,
        )
        .await?;

    assert_eq!(request_model, parent_model);
    assert_ne!(request_model, preferred_model);
    assert_eq!(
        analytics_result.guardian_catalog_contains_auto_review,
        Some(false)
    );
    assert_eq!(
        analytics_result.guardian_default_review_model_id.as_deref(),
        Some(preferred_model.as_str())
    );
    assert_eq!(
        analytics_result.guardian_review_model_overridden,
        Some(false)
    );
    assert_eq!(
        analytics_result.guardian_review_model_override.as_deref(),
        None
    );
    assert_eq!(
        analytics_result.guardian_model_provider_id.as_deref(),
        Some(DEEPSEEK_PROVIDER_ID)
    );

    Ok(())
}

#[tokio::test]
async fn build_guardian_prompt_items_includes_parent_session_id() -> anyhow::Result<()> {
    let (session, _) = crate::session::tests::make_session_and_context().await;
    let prompt = build_guardian_prompt_items(
        &session,
        /*retry_reason*/ None,
        GuardianApprovalRequest::Shell {
            id: "shell-1".to_string(),
            command: vec!["git".to_string(), "status".to_string()],
            cwd: test_path_buf("/repo").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: None,
        },
        GuardianPromptMode::Full,
    )
    .await?;
    let prompt_text = prompt
        .items
        .into_iter()
        .map(|item| match item {
            codex_protocol::user_input::UserInput::Text { text, .. } => text,
            codex_protocol::user_input::UserInput::Image { .. } => String::new(),
            _ => String::new(),
        })
        .collect::<String>();

    assert!(
        prompt_text.contains(&format!(
            ">>> TRANSCRIPT END\nReviewed Codex session id: {}\n",
            session.thread_id
        )),
        "guardian prompt should expose the parent session id immediately after the transcript end"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_review_surfaces_responses_api_errors_in_rejection_reason() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let error_message =
        "Item 'rs_test' of type 'reasoning' was provided without its required following item.";
    let request_log = mount_anthropic_response_once(
        &server,
        wiremock::ResponseTemplate::new(400).set_body_json(serde_json::json!({
            "error": {
                "message": error_message,
                "type": "invalid_request_error",
                "param": "input"
            }
        })),
    )
    .await;

    let (mut session, mut turn, rx) =
        crate::session::tests::make_session_and_context_with_rx().await;
    let mut config = (*turn.config).clone();
    config.model_provider.base_url = Some(format!("{}/v1", server.uri()));
    let config = Arc::new(config);
    let models_manager = test_support::models_manager_with_provider(
        config.codex_home.to_path_buf(),
        Arc::clone(&session.services.auth_manager),
        config.model_provider.clone(),
    );
    Arc::get_mut(&mut session)
        .expect("session should be uniquely owned")
        .services
        .models_manager = models_manager;
    let turn_mut = Arc::get_mut(&mut turn).expect("turn should be uniquely owned");
    turn_mut.config = Arc::clone(&config);
    turn_mut.provider =
        create_model_provider(config.model_provider.clone(), turn_mut.auth_manager.clone());

    seed_guardian_parent_history(&session, &turn).await;

    let decision = review_approval_request(
        &session,
        &turn,
        "review-shell-guardian-error".to_string(),
        GuardianApprovalRequest::Shell {
            id: "shell-guardian-error".to_string(),
            command: vec!["git".to_string(), "push".to_string()],
            cwd: test_path_buf("/repo/codex-rs/core").abs(),
            sandbox_permissions: crate::sandboxing::SandboxPermissions::UseDefault,
            additional_permissions: None,
            justification: Some("Need to push the reviewed docs fix.".to_string()),
        },
        /*retry_reason*/ None,
    )
    .await;

    assert_eq!(decision, ReviewDecision::Denied);
    assert_eq!(request_log.requests().len(), 1);

    let mut warnings = Vec::new();
    let mut denial_rationales = Vec::new();
    while let Ok(event) = rx.try_recv() {
        match event.msg {
            EventMsg::GuardianWarning(event) => warnings.push(event.message),
            EventMsg::GuardianAssessment(event)
                if event.status == GuardianAssessmentStatus::Denied =>
            {
                denial_rationales.push(event.rationale)
            }
            _ => {}
        }
    }

    assert!(
        warnings
            .iter()
            .any(|message| message.contains(error_message)),
        "warning should include the underlying responses api error"
    );
    assert!(
        denial_rationales
            .iter()
            .flatten()
            .any(|message| message.contains(error_message)),
        "denial rationale should include the underlying responses api error"
    );
    assert!(
        denial_rationales.iter().flatten().all(|message| {
            !message.contains("guardian review completed without an assessment payload")
        }),
        "denial rationale should not fall back to the generic missing payload error"
    );
    {
        let rationales = session.services.guardian_rejections.lock().await;
        assert!(rationales.contains_key("review-shell-guardian-error"));
        assert!(!rationales.contains_key("shell-guardian-error"));
    }
    let rejection_message =
        guardian_rejection_message(session.as_ref(), "review-shell-guardian-error").await;
    assert!(
        rejection_message.contains("Reason: Automatic approval review failed:")
            && rejection_message.contains(error_message),
        "rejection message should include guardian rationale: {rejection_message}"
    );

    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_review_retries_transient_session_failure_then_approves() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let approval = serde_json::json!({
        "risk_level": "low",
        "user_authorization": "high",
        "outcome": "allow",
        "rationale": "retry succeeded",
    })
    .to_string();
    let overloaded = || {
        wiremock::ResponseTemplate::new(503).set_body_json(serde_json::json!({
            "error": {
                "code": "server_is_overloaded",
                "message": "temporary reviewer overload"
            }
        }))
    };
    // The first guardian attempt exhausts the per-request transport retries on a
    // transient overload (a 503 maps to `ServerOverloaded`), so the guardian-level
    // retry takes a second attempt that succeeds.
    let request_log = mount_anthropic_response_sequence(
        &server,
        vec![
            overloaded(),
            overloaded(),
            sse_response(sse_anthropic_message(&approval)),
        ],
    )
    .await;
    let (session, turn) = guardian_test_session_and_turn(&server).await;
    seed_guardian_parent_history(&session, &turn).await;

    let (outcome, metadata) = run_guardian_review_session_for_test(
        Arc::clone(&session),
        Arc::clone(&turn),
        guardian_shell_request("shell-session-retry"),
        /*retry_reason*/ None,
        guardian_output_schema(),
        /*external_cancel*/ None,
        /*max_attempts*/ 3,
    )
    .await;

    let GuardianReviewOutcome::Completed(assessment) = outcome else {
        panic!("expected guardian assessment");
    };
    assert_eq!(assessment.outcome, GuardianAssessmentOutcome::Allow);
    assert_eq!(assessment.rationale, "retry succeeded");
    assert_eq!(metadata.attempt_count, 2);
    assert!(matches!(
        metadata.guardian_session_kind,
        Some(codex_analytics::GuardianReviewSessionKind::TrunkReused)
    ));
    assert_eq!(request_log.requests().len(), 3);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_review_does_not_retry_missing_assessment_payload() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let request_log = mount_anthropic_sse_sequence(
        &server,
        vec![sse_anthropic_no_message()],
    )
    .await;
    let (session, turn) = guardian_test_session_and_turn(&server).await;
    seed_guardian_parent_history(&session, &turn).await;

    let decision = review_approval_request(
        &session,
        &turn,
        "review-missing-assessment".to_string(),
        guardian_shell_request("shell-missing-assessment"),
        /*retry_reason*/ None,
    )
    .await;

    assert_eq!(decision, ReviewDecision::Denied);
    assert_eq!(request_log.requests().len(), 1);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_review_retries_two_parse_failures_then_approves() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let approval = serde_json::json!({
        "risk_level": "low",
        "user_authorization": "high",
        "outcome": "allow",
        "rationale": "retry succeeded",
    })
    .to_string();
    let request_log = mount_anthropic_sse_sequence(
        &server,
        vec![
            sse_anthropic_message("not valid guardian json"),
            sse_anthropic_message("still not valid guardian json"),
            sse_anthropic_message(&approval),
        ],
    )
    .await;
    let (session, turn) = guardian_test_session_and_turn(&server).await;
    seed_guardian_parent_history(&session, &turn).await;

    let (outcome, metadata) = run_guardian_review_session_for_test(
        Arc::clone(&session),
        Arc::clone(&turn),
        guardian_shell_request("shell-parse-retry"),
        /*retry_reason*/ None,
        guardian_output_schema(),
        /*external_cancel*/ None,
        /*max_attempts*/ 3,
    )
    .await;

    let GuardianReviewOutcome::Completed(assessment) = outcome else {
        panic!("expected guardian assessment");
    };
    assert_eq!(assessment.outcome, GuardianAssessmentOutcome::Allow);
    assert_eq!(assessment.rationale, "retry succeeded");
    assert_eq!(metadata.attempt_count, 3);
    assert!(matches!(
        metadata.guardian_session_kind,
        Some(codex_analytics::GuardianReviewSessionKind::TrunkReused)
    ));
    assert_eq!(request_log.requests().len(), 3);
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_review_exhausts_three_failures_with_one_terminal_event() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let request_log = mount_anthropic_sse_sequence(
        &server,
        vec![
            sse_anthropic_message("invalid one"),
            sse_anthropic_message("invalid two"),
            sse_anthropic_message("invalid three"),
        ],
    )
    .await;
    let (session, turn, rx) = guardian_test_session_turn_and_rx(&server).await;
    seed_guardian_parent_history(&session, &turn).await;

    let decision = review_approval_request(
        &session,
        &turn,
        "review-exhausted-retry".to_string(),
        guardian_shell_request("shell-exhausted-retry"),
        /*retry_reason*/ None,
    )
    .await;

    assert_eq!(decision, ReviewDecision::Denied);
    assert_eq!(request_log.requests().len(), 3);
    let mut statuses = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let EventMsg::GuardianAssessment(event) = event.msg {
            statuses.push(event.status);
        }
    }
    assert_eq!(
        statuses,
        vec![
            GuardianAssessmentStatus::InProgress,
            GuardianAssessmentStatus::Denied,
        ]
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guardian_review_does_not_retry_valid_denial() -> anyhow::Result<()> {
    skip_if_no_network!(Ok(()));

    let server = start_mock_server().await;
    let denial = serde_json::json!({
        "risk_level": "high",
        "user_authorization": "unknown",
        "outcome": "deny",
        "rationale": "unsafe",
    })
    .to_string();
    let request_log = mount_anthropic_sse_sequence(
        &server,
        vec![sse_anthropic_message(&denial)],
    )
    .await;
    let (session, turn) = guardian_test_session_and_turn(&server).await;
    seed_guardian_parent_history(&session, &turn).await;

    let decision = review_approval_request(
        &session,
        &turn,
        "review-valid-denial".to_string(),
        guardian_shell_request("shell-valid-denial"),
        /*retry_reason*/ None,
    )
    .await;

    assert_eq!(decision, ReviewDecision::Denied);
    assert_eq!(request_log.requests().len(), 1);
    Ok(())
}

#[tokio::test]
async fn guardian_review_session_config_preserves_parent_network_proxy() {
    let mut parent_config = test_config().await;
    let network = NetworkProxySpec::from_config_and_constraints(
        NetworkProxyConfig::default(),
        Some(NetworkConstraints {
            enabled: Some(true),
            domains: Some(NetworkDomainPermissionsToml {
                entries: std::collections::BTreeMap::from([(
                    "github.com".to_string(),
                    NetworkDomainPermissionToml::Allow,
                )]),
            }),
            ..Default::default()
        }),
        parent_config.permissions.permission_profile(),
    )
    .expect("network proxy spec");
    parent_config.permissions.network = Some(network.clone());

    let guardian_config = build_guardian_review_session_config_for_test(
        &parent_config,
        /*live_network_config*/ None,
        "parent-active-model",
        Some(codex_protocol::openai_models::ReasoningEffort::Low),
    )
    .expect("guardian config");

    assert_eq!(guardian_config.permissions.network, Some(network));
    assert_eq!(
        guardian_config.model,
        Some("parent-active-model".to_string())
    );
    assert_eq!(
        guardian_config.model_reasoning_effort,
        Some(codex_protocol::openai_models::ReasoningEffort::Low)
    );
    assert_eq!(
        guardian_config.permissions.approval_policy,
        Constrained::allow_only(AskForApproval::Never)
    );
    assert_eq!(
        guardian_config.permissions.permission_profile(),
        &PermissionProfile::read_only()
    );
}

#[tokio::test]
async fn guardian_review_session_config_clears_parent_developer_instructions() {
    let mut parent_config = test_config().await;
    parent_config.developer_instructions =
        Some("parent or managed config should not replace guardian policy".to_string());

    let guardian_config = build_guardian_review_session_config_for_test(
        &parent_config,
        /*live_network_config*/ None,
        "active-model",
        /*reasoning_effort*/ None,
    )
    .expect("guardian config");

    assert_eq!(guardian_config.developer_instructions, None);
    assert_eq!(
        guardian_config.base_instructions,
        Some(guardian_policy_prompt())
    );
}

#[tokio::test]
async fn guardian_review_session_config_clears_legacy_notify() {
    let mut parent_config = test_config().await;
    parent_config.notify = Some(vec![
        "/path/to/notify".to_string(),
        "turn-ended".to_string(),
    ]);

    let guardian_config = build_guardian_review_session_config_for_test(
        &parent_config,
        /*live_network_config*/ None,
        "active-model",
        /*reasoning_effort*/ None,
    )
    .expect("guardian config");

    assert_eq!(guardian_config.notify, None);
}

#[tokio::test]
async fn guardian_review_session_config_uses_live_network_proxy_state() {
    let mut parent_config = test_config().await;
    let mut parent_network = NetworkProxyConfig::default();
    parent_network.network.enabled = true;
    parent_network
        .network
        .set_allowed_domains(vec!["parent.example".to_string()]);
    parent_config.permissions.network = Some(
        NetworkProxySpec::from_config_and_constraints(
            parent_network,
            /*requirements*/ None,
            parent_config.permissions.permission_profile(),
        )
        .expect("parent network proxy spec"),
    );

    let mut live_network = NetworkProxyConfig::default();
    live_network.network.enabled = true;
    live_network
        .network
        .set_allowed_domains(vec!["github.com".to_string()]);

    let guardian_config = build_guardian_review_session_config_for_test(
        &parent_config,
        Some(live_network.clone()),
        "active-model",
        /*reasoning_effort*/ None,
    )
    .expect("guardian config");

    assert_eq!(
        guardian_config.permissions.network,
        Some(
            NetworkProxySpec::from_config_and_constraints(
                live_network,
                /*requirements*/ None,
                &PermissionProfile::read_only(),
            )
            .expect("live network proxy spec")
        )
    );
}

#[tokio::test]
async fn guardian_review_session_config_disables_mcp_apps_plugins_and_memories() {
    let mut parent_config = test_config().await;
    let server: McpServerConfig =
        toml::from_str("command = \"docs-server\"").expect("deserialize MCP server");
    parent_config
        .mcp_servers
        .set(HashMap::from([("docs".to_string(), server)]))
        .expect("parent MCP servers are configurable");
    parent_config
        .features
        .enable(Feature::Apps)
        .expect("apps feature is configurable");
    parent_config
        .features
        .enable(Feature::Plugins)
        .expect("plugins feature is configurable");
    parent_config.include_apps_instructions = true;
    parent_config.memories.use_memories = true;
    parent_config.memories.dedicated_tools = true;

    let guardian_config = build_guardian_review_session_config_for_test(
        &parent_config,
        /*live_network_config*/ None,
        "active-model",
        /*reasoning_effort*/ None,
    )
    .expect("guardian config");

    assert!(guardian_config.mcp_servers.get().is_empty());
    assert!(!guardian_config.features.enabled(Feature::Apps));
    assert!(!guardian_config.features.enabled(Feature::Plugins));
    assert!(!guardian_config.include_apps_instructions);
    assert!(!guardian_config.memories.use_memories);
    assert!(!guardian_config.memories.dedicated_tools);
}

#[tokio::test]
async fn guardian_review_session_config_allows_pinned_disabled_feature() {
    let mut parent_config = test_config().await;
    parent_config.features = ManagedFeatures::from_configured(
        parent_config.features.get().clone(),
        Some(Sourced {
            value: FeatureRequirementsToml {
                entries: BTreeMap::from([("multi_agent".to_string(), true)]),
            },
            source: RequirementSource::Unknown,
        }),
    )
    .expect("managed features");

    let guardian_config = build_guardian_review_session_config_for_test(
        &parent_config,
        /*live_network_config*/ None,
        "active-model",
        /*reasoning_effort*/ None,
    )
    .expect("guardian config should continue when a disabled feature is pinned on");

    assert!(guardian_config.features.enabled(Feature::Collab));
    assert!(guardian_config.mcp_servers.get().is_empty());
    assert!(!guardian_config.include_apps_instructions);
}

#[tokio::test]
async fn guardian_review_session_config_uses_parent_active_model_instead_of_hardcoded_slug() {
    let mut parent_config = test_config().await;
    parent_config.model = Some("configured-model".to_string());

    let guardian_config = build_guardian_review_session_config_for_test(
        &parent_config,
        /*live_network_config*/ None,
        "active-model",
        /*reasoning_effort*/ None,
    )
    .expect("guardian config");

    assert_eq!(guardian_config.model, Some("active-model".to_string()));
}

#[tokio::test]
async fn guardian_review_session_config_uses_requirements_guardian_policy_config() {
    let codex_home = tempfile::tempdir().expect("create temp dir");
    let workspace = tempfile::tempdir().expect("create temp dir");
    let config_layer_stack = ConfigLayerStack::new(
        Vec::new(),
        Default::default(),
        codex_config::ConfigRequirementsToml {
            guardian_policy_config: Some(
                "  Use the workspace-managed guardian policy.  ".to_string(),
            ),
            ..Default::default()
        },
    )
    .expect("config layer stack");
    let parent_config = Config::load_config_with_layer_stack(
        LOCAL_FS.as_ref(),
        ConfigToml::default(),
        ConfigOverrides {
            cwd: Some(workspace.path().to_path_buf()),
            ..Default::default()
        },
        codex_home.abs(),
        config_layer_stack,
    )
    .await
    .expect("load config");

    let guardian_config = build_guardian_review_session_config_for_test(
        &parent_config,
        /*live_network_config*/ None,
        "active-model",
        /*reasoning_effort*/ None,
    )
    .expect("guardian config");

    assert_eq!(guardian_config.developer_instructions, None);
    assert_eq!(
        guardian_config.base_instructions,
        Some(guardian_policy_prompt_with_config(
            "Use the workspace-managed guardian policy."
        ))
    );
}

#[tokio::test]
async fn guardian_review_session_config_uses_default_guardian_policy_without_requirements_override()
{
    let codex_home = tempfile::tempdir().expect("create temp dir");
    let workspace = tempfile::tempdir().expect("create temp dir");
    let config_layer_stack =
        ConfigLayerStack::new(Vec::new(), Default::default(), Default::default())
            .expect("config layer stack");
    let parent_config = Config::load_config_with_layer_stack(
        LOCAL_FS.as_ref(),
        ConfigToml::default(),
        ConfigOverrides {
            cwd: Some(workspace.path().to_path_buf()),
            ..Default::default()
        },
        codex_home.abs(),
        config_layer_stack,
    )
    .await
    .expect("load config");

    let guardian_config = build_guardian_review_session_config_for_test(
        &parent_config,
        /*live_network_config*/ None,
        "active-model",
        /*reasoning_effort*/ None,
    )
    .expect("guardian config");

    assert_eq!(guardian_config.developer_instructions, None);
    assert_eq!(
        guardian_config.base_instructions,
        Some(guardian_policy_prompt())
    );
}
