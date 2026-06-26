use crate::anthropic::MessagesApiRequest;
use crate::auth::SharedAuthProvider;
use crate::common::ResponseStream;
use crate::endpoint::session::EndpointSession;
use crate::error::ApiError;
use crate::provider::Provider;
use crate::requests::Compression;
use crate::sse::spawn_anthropic_stream;
use crate::telemetry::SseTelemetry;
use codex_client::EncodedJsonBody;
use codex_client::HttpTransport;
use codex_client::RequestCompression;
use codex_client::RequestTelemetry;
use http::HeaderMap;
use http::HeaderValue;
use http::Method;
use std::sync::Arc;
use tracing::instrument;

/// Client for DeepSeek's Anthropic-compatible Messages API
/// (`{base_url}/v1/messages`).
pub struct AnthropicClient<T: HttpTransport> {
    session: EndpointSession<T>,
    sse_telemetry: Option<Arc<dyn SseTelemetry>>,
}

#[derive(Default)]
pub struct AnthropicOptions {
    pub extra_headers: HeaderMap,
    pub compression: Compression,
}

impl<T: HttpTransport> AnthropicClient<T> {
    pub fn new(transport: T, provider: Provider, auth: SharedAuthProvider) -> Self {
        Self {
            session: EndpointSession::new(transport, provider, auth),
            sse_telemetry: None,
        }
    }

    pub fn with_telemetry(
        self,
        request: Option<Arc<dyn RequestTelemetry>>,
        sse: Option<Arc<dyn SseTelemetry>>,
    ) -> Self {
        Self {
            session: self.session.with_request_telemetry(request),
            sse_telemetry: sse,
        }
    }

    fn path() -> &'static str {
        "v1/messages"
    }

    #[instrument(
        name = "anthropic.stream_request",
        level = "info",
        skip_all,
        fields(
            transport = "anthropic_http",
            http.method = "POST",
            api.path = "v1/messages"
        )
    )]
    pub async fn stream_request(
        &self,
        request: MessagesApiRequest,
        options: AnthropicOptions,
    ) -> Result<ResponseStream, ApiError> {
        let body = EncodedJsonBody::encode(&request)
            .map_err(|e| ApiError::Stream(format!("failed to encode messages request: {e}")))?;
        let request_compression = match options.compression {
            Compression::None => RequestCompression::None,
            Compression::Zstd => RequestCompression::Zstd,
        };

        let stream_response = self
            .session
            .stream_encoded_json_with(
                Method::POST,
                Self::path(),
                options.extra_headers,
                Some(body),
                |req| {
                    req.headers.insert(
                        http::header::ACCEPT,
                        HeaderValue::from_static("text/event-stream"),
                    );
                    req.compression = request_compression;
                },
            )
            .await?;

        Ok(spawn_anthropic_stream(
            stream_response,
            self.session.provider().stream_idle_timeout,
            self.sse_telemetry.clone(),
        ))
    }
}
