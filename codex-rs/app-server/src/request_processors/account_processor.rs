use super::*;
use chrono::DateTime;
use codex_protocol::account::ProviderAccount;

mod rate_limit_resets;

const ACCOUNT_TOKEN_USAGE_FETCH_TIMEOUT: Duration = Duration::from_secs(/*secs*/ 10);
const ACCOUNT_WORKSPACE_MESSAGES_FETCH_TIMEOUT: Duration =
    Duration::from_millis(/*millis*/ 1000);

#[derive(Clone)]
pub(crate) struct AccountRequestProcessor {
    auth_manager: Arc<AuthManager>,
    config: Arc<Config>,
}

impl AccountRequestProcessor {
    pub(crate) fn new(auth_manager: Arc<AuthManager>, config: Arc<Config>) -> Self {
        Self {
            auth_manager,
            config,
        }
    }

    pub(crate) async fn get_account(
        &self,
        params: GetAccountParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_auth_status(
        &self,
        _params: GetAuthStatusParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        let account = self.get_account_response(GetAccountParams {}).await?;
        let auth_method =
            matches!(account.account, Some(Account::ApiKey {})).then_some(AuthMode::ApiKey);
        Ok(Some(
            GetAuthStatusResponse {
                auth_method,
                auth_token: None,
                requires_openai_auth: Some(account.requires_openai_auth),
            }
            .into(),
        ))
    }

    pub(crate) async fn get_account_rate_limits(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_rate_limits_response()
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_account_token_usage(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_account_token_usage_response()
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn get_workspace_messages(
        &self,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.get_workspace_messages_response()
            .await
            .map(|response| Some(response.into()))
    }

    pub(crate) async fn send_add_credits_nudge_email(
        &self,
        params: SendAddCreditsNudgeEmailParams,
    ) -> Result<Option<ClientResponsePayload>, JSONRPCErrorError> {
        self.send_add_credits_nudge_email_response(params)
            .await
            .map(|response| Some(response.into()))
    }

    async fn get_account_response(
        &self,
        params: GetAccountParams,
    ) -> Result<GetAccountResponse, JSONRPCErrorError> {
        let _ = params;

        let provider = create_model_provider(
            self.config.model_provider.clone(),
            Some(self.auth_manager.clone()),
        );
        let account_state = match provider.account_state() {
            Ok(account_state) => account_state,
            Err(err) => return Err(invalid_request(err.to_string())),
        };
        let account = match account_state.account {
            Some(ProviderAccount::ApiKey) => Some(Account::ApiKey {}),
            Some(ProviderAccount::Chatgpt { .. } | ProviderAccount::AmazonBedrock { .. })
            | None => None,
        };

        Ok(GetAccountResponse {
            account,
            requires_openai_auth: account_state.requires_openai_auth,
        })
    }

    async fn get_account_rate_limits_response(
        &self,
    ) -> Result<GetAccountRateLimitsResponse, JSONRPCErrorError> {
        let Some(auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "codex account authentication required to read rate limits",
            ));
        };

        if !auth.uses_codex_backend() {
            return Err(invalid_request(
                "chatgpt authentication required to read rate limits",
            ));
        }

        let client = BackendClient::from_auth(self.config.chatgpt_base_url.clone(), &auth)
            .map_err(|err| internal_error(format!("failed to construct backend client: {err}")))?;

        let response = client
            .get_rate_limits_with_reset_credits()
            .await
            .map_err(|err| internal_error(format!("failed to fetch codex rate limits: {err}")))?;
        if response.rate_limits.is_empty() {
            return Err(internal_error(
                "failed to fetch codex rate limits: no snapshots returned",
            ));
        }

        let rate_limits_by_limit_id: HashMap<_, _> = response
            .rate_limits
            .iter()
            .cloned()
            .map(|snapshot| {
                let limit_id = snapshot
                    .limit_id
                    .clone()
                    .unwrap_or_else(|| "codex".to_string());
                (limit_id, snapshot)
            })
            .collect();
        let rate_limits = response
            .rate_limits
            .iter()
            .find(|snapshot| snapshot.limit_id.as_deref() == Some("codex"))
            .cloned()
            .unwrap_or_else(|| response.rate_limits[0].clone());

        Ok(GetAccountRateLimitsResponse {
            rate_limits: rate_limits.into(),
            rate_limits_by_limit_id: Some(
                rate_limits_by_limit_id
                    .into_iter()
                    .map(|(limit_id, snapshot)| (limit_id, snapshot.into()))
                    .collect(),
            ),
            rate_limit_reset_credits: response.rate_limit_reset_credits.map(|summary| {
                RateLimitResetCreditsSummary {
                    available_count: summary.available_count,
                }
            }),
        })
    }

    async fn get_account_token_usage_response(
        &self,
    ) -> Result<GetAccountTokenUsageResponse, JSONRPCErrorError> {
        let Some(auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "codex account authentication required to read token usage",
            ));
        };

        if !auth.uses_codex_backend() {
            return Err(invalid_request(
                "chatgpt authentication required to read token usage",
            ));
        }

        let client = BackendClient::from_auth(self.config.chatgpt_base_url.clone(), &auth)
            .map_err(|err| internal_error(format!("failed to construct backend client: {err}")))?;
        let profile = tokio::time::timeout(
            ACCOUNT_TOKEN_USAGE_FETCH_TIMEOUT,
            client.get_token_usage_profile(),
        )
        .await
        .map_err(|_| internal_error("token usage profile fetch timed out"))?
        .map_err(|err| internal_error(format!("failed to fetch token usage profile: {err}")))?;
        Ok(Self::account_token_usage_response(profile))
    }

    async fn get_workspace_messages_response(
        &self,
    ) -> Result<GetWorkspaceMessagesResponse, JSONRPCErrorError> {
        let Some(auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "codex account authentication required to read workspace messages",
            ));
        };

        if !auth.uses_codex_backend() {
            return Err(invalid_request(
                "chatgpt authentication required to read workspace messages",
            ));
        }

        let client = BackendClient::from_auth(self.config.chatgpt_base_url.clone(), &auth)
            .map_err(|err| internal_error(format!("failed to construct backend client: {err}")))?;
        let messages = tokio::time::timeout(
            ACCOUNT_WORKSPACE_MESSAGES_FETCH_TIMEOUT,
            client.list_workspace_messages(),
        )
        .await
        .map_err(|_| internal_error("workspace messages fetch timed out"))?;

        match messages {
            Ok(messages) => {
                Self::workspace_messages_response(messages, /*feature_enabled*/ true)
            }
            Err(err) if workspace_messages_feature_disabled(&err) => {
                Self::workspace_messages_response(
                    BackendWorkspaceMessagesResponse {
                        messages: Vec::new(),
                    },
                    /*feature_enabled*/ false,
                )
            }
            Err(err) => Err(internal_error(format!(
                "failed to fetch workspace messages: {err}"
            ))),
        }
    }

    fn account_token_usage_response(profile: TokenUsageProfile) -> GetAccountTokenUsageResponse {
        let stats = profile.stats;
        GetAccountTokenUsageResponse {
            summary: AccountTokenUsageSummary {
                lifetime_tokens: stats.lifetime_tokens,
                peak_daily_tokens: stats.peak_daily_tokens,
                longest_running_turn_sec: stats.longest_running_turn_sec,
                current_streak_days: stats.current_streak_days,
                longest_streak_days: stats.longest_streak_days,
            },
            daily_usage_buckets: stats.daily_usage_buckets.map(|buckets| {
                buckets
                    .into_iter()
                    .map(|bucket| AccountTokenUsageDailyBucket {
                        start_date: bucket.start_date,
                        tokens: bucket.tokens,
                    })
                    .collect()
            }),
        }
    }

    fn workspace_messages_response(
        messages: BackendWorkspaceMessagesResponse,
        feature_enabled: bool,
    ) -> Result<GetWorkspaceMessagesResponse, JSONRPCErrorError> {
        Ok(GetWorkspaceMessagesResponse {
            feature_enabled,
            messages: messages
                .messages
                .into_iter()
                .map(workspace_message_from_backend)
                .collect::<Result<Vec<_>, _>>()?,
        })
    }

    async fn send_add_credits_nudge_email_response(
        &self,
        params: SendAddCreditsNudgeEmailParams,
    ) -> Result<SendAddCreditsNudgeEmailResponse, JSONRPCErrorError> {
        self.send_add_credits_nudge_email_inner(params)
            .await
            .map(|status| SendAddCreditsNudgeEmailResponse { status })
    }

    async fn send_add_credits_nudge_email_inner(
        &self,
        params: SendAddCreditsNudgeEmailParams,
    ) -> Result<AddCreditsNudgeEmailStatus, JSONRPCErrorError> {
        let Some(auth) = self.auth_manager.auth().await else {
            return Err(invalid_request(
                "codex account authentication required to notify workspace owner",
            ));
        };

        if !auth.uses_codex_backend() {
            return Err(invalid_request(
                "chatgpt authentication required to notify workspace owner",
            ));
        }

        let client = BackendClient::from_auth(self.config.chatgpt_base_url.clone(), &auth)
            .map_err(|err| internal_error(format!("failed to construct backend client: {err}")))?;

        match client
            .send_add_credits_nudge_email(Self::backend_credit_type(params.credit_type))
            .await
        {
            Ok(()) => Ok(AddCreditsNudgeEmailStatus::Sent),
            Err(err) if err.status().is_some_and(|status| status.as_u16() == 429) => {
                Ok(AddCreditsNudgeEmailStatus::CooldownActive)
            }
            Err(err) => Err(internal_error(format!(
                "failed to notify workspace owner: {err}"
            ))),
        }
    }

    fn backend_credit_type(value: AddCreditsNudgeCreditType) -> BackendAddCreditsNudgeCreditType {
        match value {
            AddCreditsNudgeCreditType::Credits => BackendAddCreditsNudgeCreditType::Credits,
            AddCreditsNudgeCreditType::UsageLimit => BackendAddCreditsNudgeCreditType::UsageLimit,
        }
    }
}

fn workspace_message_from_backend(
    message: BackendWorkspaceMessage,
) -> Result<WorkspaceMessage, JSONRPCErrorError> {
    Ok(WorkspaceMessage {
        message_id: message.message_id,
        message_type: workspace_message_type_from_backend(message.message_type),
        message_body: message.message_body,
        created_at: workspace_message_timestamp_from_backend(message.created_at)?,
        archived_at: workspace_message_timestamp_from_backend(message.archived_at)?,
    })
}

fn workspace_message_timestamp_from_backend(
    timestamp: Option<String>,
) -> Result<Option<i64>, JSONRPCErrorError> {
    timestamp
        .map(|timestamp| {
            DateTime::parse_from_rfc3339(&timestamp)
                .map(|timestamp| timestamp.timestamp())
                .map_err(|err| {
                    internal_error(format!(
                        "failed to parse workspace message timestamp `{timestamp}`: {err}"
                    ))
                })
        })
        .transpose()
}

fn workspace_message_type_from_backend(
    message_type: BackendWorkspaceMessageType,
) -> WorkspaceMessageType {
    match message_type {
        BackendWorkspaceMessageType::Headline => WorkspaceMessageType::Headline,
        BackendWorkspaceMessageType::Announcement => WorkspaceMessageType::Announcement,
        BackendWorkspaceMessageType::Unknown => WorkspaceMessageType::Unknown,
    }
}

fn workspace_messages_feature_disabled(err: &BackendRequestError) -> bool {
    err.status().is_some_and(|status| status.as_u16() == 404)
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_backend_client::TokenUsageProfileDailyBucket;
    use codex_backend_client::TokenUsageProfileStats;
    use pretty_assertions::assert_eq;

    #[test]
    fn account_token_usage_response_maps_profile_stats_and_daily_buckets() {
        let response = AccountRequestProcessor::account_token_usage_response(TokenUsageProfile {
            stats: TokenUsageProfileStats {
                lifetime_tokens: Some(123),
                peak_daily_tokens: Some(45),
                longest_running_turn_sec: Some(67),
                current_streak_days: Some(8),
                longest_streak_days: Some(9),
                daily_usage_buckets: Some(vec![TokenUsageProfileDailyBucket {
                    start_date: "2026-05-29".to_string(),
                    tokens: 10,
                }]),
            },
        });

        assert_eq!(
            response,
            GetAccountTokenUsageResponse {
                summary: AccountTokenUsageSummary {
                    lifetime_tokens: Some(123),
                    peak_daily_tokens: Some(45),
                    longest_running_turn_sec: Some(67),
                    current_streak_days: Some(8),
                    longest_streak_days: Some(9),
                },
                daily_usage_buckets: Some(vec![AccountTokenUsageDailyBucket {
                    start_date: "2026-05-29".to_string(),
                    tokens: 10,
                }]),
            }
        );
    }

    #[test]
    fn workspace_messages_response_maps_backend_messages() {
        let response = AccountRequestProcessor::workspace_messages_response(
            BackendWorkspaceMessagesResponse {
                messages: vec![BackendWorkspaceMessage {
                    message_id: "headline-id".to_string(),
                    message_type: BackendWorkspaceMessageType::Headline,
                    message_body: "Headline body".to_string(),
                    created_at: Some("2026-06-14T00:00:00Z".to_string()),
                    archived_at: Some("2026-06-15T00:00:00Z".to_string()),
                }],
            },
            /*feature_enabled*/ true,
        )
        .expect("workspace message timestamps should parse");

        assert_eq!(
            response,
            GetWorkspaceMessagesResponse {
                feature_enabled: true,
                messages: vec![WorkspaceMessage {
                    message_id: "headline-id".to_string(),
                    message_type: WorkspaceMessageType::Headline,
                    message_body: "Headline body".to_string(),
                    created_at: Some(1_781_395_200),
                    archived_at: Some(1_781_481_600),
                }],
            }
        );
    }

    #[test]
    fn workspace_messages_feature_disabled_only_for_not_found() {
        let cases = [
            (reqwest::StatusCode::NOT_FOUND, true),
            (reqwest::StatusCode::UNAUTHORIZED, false),
            (reqwest::StatusCode::FORBIDDEN, false),
        ];

        for (status, expected) in cases {
            let err = BackendRequestError::UnexpectedStatus {
                method: "GET".to_string(),
                url: "https://example.test/api/codex/workspace-messages".to_string(),
                status,
                content_type: "application/json".to_string(),
                body: "{}".to_string(),
            };
            assert_eq!(workspace_messages_feature_disabled(&err), expected);
        }
    }
}
