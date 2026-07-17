//! Client-side web tools for the DeepSeek agent: `web_search` and `read_url`.
//!
//! These replace the deleted OpenAI hosted web-search tool. Search and fetch
//! both run client-side here (no provider-hosted search), so they work against
//! the Anthropic Messages wire that DeepSeek-V4 speaks.
//!
//! NOTE: DeepSeek's server-side `<|action|>` / `<|query|>` / `<|read_url|>`
//! product tokens are NOT emitted over the Anthropic Messages API. The agentic
//! web loop here is therefore driven entirely by these explicit `web_search`
//! and `read_url` function-tool calls, not by parsing those tags.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::sync::OnceLock;
use std::time::Duration;

use anyhow::Context as _;
use anyhow::Result;
use futures::future::BoxFuture;
use regex_lite::Regex;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;

use crate::function_tool::FunctionCallError;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::handlers::parse_arguments;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;

/// Pretend to be a normal browser; DDG HTML serves a degraded page to obvious bots.
const USER_AGENT: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
     (KHTML, like Gecko) Chrome/124.0 Safari/537.36";

const DEFAULT_MAX_RESULTS: usize = 5;
const MAX_RESULTS_CAP: usize = 20;
/// Cap extracted page text so a single fetch can't blow the context window.
const READ_URL_TEXT_CAP: usize = 8_000;
const HTTP_TIMEOUT: Duration = Duration::from_secs(20);

/// A single search result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
}

/// Pluggable search backend. Swap the default DuckDuckGo HTML scraper for an API
/// backend later without touching the tool handlers.
pub trait SearchBackend: Send + Sync {
    fn search<'a>(&'a self, query: &'a str, max: usize) -> BoxFuture<'a, Result<Vec<SearchHit>>>;
}

/// Keyless DuckDuckGo HTML backend.
///
/// ponytail: scraping `html.duckduckgo.com` is the known-fragile ceiling here —
/// no API key, no stability guarantee. It is deliberately swappable via
/// [`SearchBackend`] when a real search API is wired in.
#[derive(Clone)]
pub struct DuckDuckGoBackend {
    client: reqwest::Client,
}

impl Default for DuckDuckGoBackend {
    fn default() -> Self {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(HTTP_TIMEOUT)
            .build()
            .unwrap_or_default();
        Self { client }
    }
}

impl SearchBackend for DuckDuckGoBackend {
    fn search<'a>(&'a self, query: &'a str, max: usize) -> BoxFuture<'a, Result<Vec<SearchHit>>> {
        Box::pin(async move {
            let encoded: String = url::form_urlencoded::byte_serialize(query.as_bytes()).collect();
            let endpoint = format!("https://html.duckduckgo.com/html/?q={encoded}");
            let body = self
                .client
                .get(&endpoint)
                .send()
                .await
                .context("failed to reach DuckDuckGo HTML endpoint")?
                .error_for_status()
                .context("DuckDuckGo HTML endpoint returned an error status")?
                .text()
                .await
                .context("failed to read DuckDuckGo HTML response body")?;
            Ok(parse_ddg_results(&body, max))
        })
    }
}

/// Parse anchors + snippets out of a DuckDuckGo HTML results page.
///
/// Title/URL come from `result__a` anchors; snippets from `result__snippet`
/// anchors. They are zipped by index — fragile, but adequate for the HTML DDG
/// currently serves.
#[allow(clippy::expect_used, clippy::unwrap_used)]
fn parse_ddg_results(html: &str, max: usize) -> Vec<SearchHit> {
    static ANCHOR_RE: OnceLock<Regex> = OnceLock::new();
    static SNIPPET_RE: OnceLock<Regex> = OnceLock::new();
    let anchor_re = ANCHOR_RE.get_or_init(|| {
        Regex::new(r#"(?is)<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
            .expect("static regex compiles")
    });
    let snippet_re = SNIPPET_RE.get_or_init(|| {
        Regex::new(r#"(?is)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#)
            .expect("static regex compiles")
    });

    let snippets: Vec<String> = snippet_re
        .captures_iter(html)
        .map(|cap| html_to_text(&cap[1]))
        .collect();

    anchor_re
        .captures_iter(html)
        .enumerate()
        .take(max)
        .map(|(idx, cap)| SearchHit {
            title: html_to_text(&cap[2]),
            url: decode_ddg_href(&cap[1]),
            snippet: snippets.get(idx).cloned().unwrap_or_default(),
        })
        .collect()
}

/// DuckDuckGo HTML links are redirects like `//duckduckgo.com/l/?uddg=<target>`.
/// Pull out and decode the real target; fall back to the raw href otherwise.
fn decode_ddg_href(href: &str) -> String {
    let href = href.replace("&amp;", "&");
    let query = href.split_once('?').map_or("", |(_, q)| q);
    if let Some(target) = url::form_urlencoded::parse(query.as_bytes())
        .find(|(key, _)| key == "uddg")
        .map(|(_, value)| value.into_owned())
    {
        return target;
    }
    if let Some(stripped) = href.strip_prefix("//") {
        format!("https://{stripped}")
    } else {
        href
    }
}

/// Strip HTML to readable text: drop `<script>`/`<style>`, remove remaining
/// tags, decode a few common entities, and collapse whitespace.
#[allow(clippy::expect_used, clippy::unwrap_used)]
pub fn html_to_text(html: &str) -> String {
    static SCRIPT_STYLE_RE: OnceLock<Regex> = OnceLock::new();
    static TAG_RE: OnceLock<Regex> = OnceLock::new();
    let script_style_re = SCRIPT_STYLE_RE.get_or_init(|| {
        Regex::new(r"(?is)<(script|style)[^>]*>.*?</(script|style)>")
            .expect("static regex compiles")
    });
    let tag_re = TAG_RE.get_or_init(|| Regex::new(r"(?s)<[^>]+>").expect("static regex compiles"));

    let without_blocks = script_style_re.replace_all(html, " ");
    let without_tags = tag_re.replace_all(&without_blocks, " ");
    let decoded = decode_entities(&without_tags);
    decoded.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn decode_entities(text: &str) -> String {
    text.replace("&nbsp;", " ")
        .replace("&amp;", "&")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
}

/// Returns true if the IP is in a private/loopback/link-local/unspecified range.
fn is_blocked_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
        }
        IpAddr::V6(v6) => {
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                || v6.is_unicast_link_local() // fe80::/10
                || (v6.segments()[0] & 0xfe00) == 0xfc00 // fc00::/7 unique local
        }
    }
}

/// Resolve the URL's hostname and reject it if any resolved IP is in a blocked
/// range. This prevents SSRF to cloud metadata endpoints, localhost services, etc.
///
/// ponytail: this is a best-effort pre-check. It does not prevent DNS rebinding
/// (the IP checked here may differ from the one reqwest connects to). For full
/// protection, pair with a reqwest DNS resolver that enforces the same policy.
async fn validate_url_safety(url: &str) -> Result<()> {
    let parsed = url::Url::parse(url).with_context(|| format!("failed to parse URL: {url}"))?;

    let host = parsed
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("URL has no host: {url}"))?;

    // Fast path: IP literal.
    if let Ok(ip) = host.parse::<IpAddr>() {
        if is_blocked_ip(ip) {
            anyhow::bail!("URL resolves to a blocked internal address: {ip}");
        }
        return Ok(());
    }

    // Resolve hostname and check all results.
    let addrs: Vec<_> = tokio::net::lookup_host(format!("{host}:443"))
        .await
        .with_context(|| format!("failed to resolve host: {host}"))?
        .collect();

    if addrs.is_empty() {
        anyhow::bail!("DNS resolution returned no results for {host}");
    }

    for addr in &addrs {
        if is_blocked_ip(addr.ip()) {
            anyhow::bail!(
                "URL resolves to a blocked internal address: {} ({})",
                addr.ip(),
                host
            );
        }
    }

    Ok(())
}

async fn fetch_url_as_text(url: &str) -> Result<String> {
    validate_url_safety(url).await?;

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(HTTP_TIMEOUT)
        .build()
        .context("failed to build HTTP client")?;
    let body = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("failed to fetch {url}"))?
        .error_for_status()
        .with_context(|| format!("{url} returned an error status"))?
        .text()
        .await
        .with_context(|| format!("failed to read body from {url}"))?;
    let mut text = html_to_text(&body);
    if text.len() > READ_URL_TEXT_CAP {
        text.truncate(READ_URL_TEXT_CAP);
        text.push_str("\n[... truncated ...]");
    }
    Ok(text)
}

#[derive(Deserialize)]
struct WebSearchArgs {
    query: String,
    #[serde(default)]
    max_results: Option<usize>,
}

#[derive(Deserialize)]
struct ReadUrlArgs {
    url: String,
}

/// `web_search` tool: keyless DuckDuckGo search.
pub struct WebSearchHandler {
    backend: Arc<dyn SearchBackend>,
}

impl Default for WebSearchHandler {
    fn default() -> Self {
        Self {
            backend: Arc::new(DuckDuckGoBackend::default()),
        }
    }
}

impl ToolExecutor<ToolInvocation> for WebSearchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("web_search")
    }

    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([
            (
                "query".to_string(),
                JsonSchema::string(Some("The search query.".to_string())),
            ),
            (
                "max_results".to_string(),
                JsonSchema::integer(Some(format!(
                    "Maximum number of results to return (default {DEFAULT_MAX_RESULTS}, max {MAX_RESULTS_CAP})."
                ))),
            ),
        ]);
        ToolSpec::Function(ResponsesApiTool {
            name: "web_search".to_string(),
            description: "Search the web (keyless DuckDuckGo) and return a list of result titles, URLs, and snippets."
                .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["query".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "web_search handler received unsupported payload".to_string(),
                ));
            };
            let WebSearchArgs { query, max_results } = parse_arguments(arguments)?;
            let max = max_results
                .unwrap_or(DEFAULT_MAX_RESULTS)
                .clamp(1, MAX_RESULTS_CAP);

            let hits = self.backend.search(&query, max).await.map_err(|err| {
                FunctionCallError::RespondToModel(format!("web_search failed: {err:#}"))
            })?;

            let text = if hits.is_empty() {
                format!("No results found for query: {query}")
            } else {
                hits.iter()
                    .enumerate()
                    .map(|(idx, hit)| {
                        format!("{}. {}\n{}\n{}", idx + 1, hit.title, hit.url, hit.snippet)
                    })
                    .collect::<Vec<_>>()
                    .join("\n\n")
            };

            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                text,
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for WebSearchHandler {}

/// `read_url` tool: fetch a URL and return readable text.
#[derive(Default)]
pub struct ReadUrlHandler;

impl ToolExecutor<ToolInvocation> for ReadUrlHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("read_url")
    }

    fn spec(&self) -> ToolSpec {
        let properties = BTreeMap::from([(
            "url".to_string(),
            JsonSchema::string(Some("The absolute http(s) URL to fetch.".to_string())),
        )]);
        ToolSpec::Function(ResponsesApiTool {
            name: "read_url".to_string(),
            description:
                "Fetch a URL and return its readable text content (HTML stripped, truncated)."
                    .to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::object(
                properties,
                Some(vec!["url".to_string()]),
                Some(false.into()),
            ),
            output_schema: None,
        })
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "read_url handler received unsupported payload".to_string(),
                ));
            };
            let ReadUrlArgs { url } = parse_arguments(arguments)?;
            if !(url.starts_with("http://") || url.starts_with("https://")) {
                return Err(FunctionCallError::RespondToModel(format!(
                    "read_url requires an absolute http(s) URL, got `{url}`"
                )));
            }

            let text = fetch_url_as_text(&url).await.map_err(|err| {
                FunctionCallError::RespondToModel(format!("read_url failed: {err:#}"))
            })?;

            Ok(boxed_tool_output(FunctionToolOutput::from_text(
                text,
                Some(true),
            )))
        })
    }
}

impl CoreToolRuntime for ReadUrlHandler {}

// Keep an explicit JSON helper so output schemas can be added later if needed.
#[allow(dead_code)]
fn search_hit_json(hit: &SearchHit) -> JsonValue {
    json!({ "title": hit.title, "url": hit.url, "snippet": hit.snippet })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    const SAMPLE_DDG_HTML: &str = r##"
<html><body>
<div class="result results_links results_links_deep web-result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust%2Dlang.org%2F&amp;rut=abc">The <b>Rust</b> Programming Language</a>
  </h2>
  <a class="result__snippet" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fwww.rust%2Dlang.org%2F">A language empowering everyone to build <b>reliable</b> software.</a>
</div>
<div class="result results_links results_links_deep web-result">
  <h2 class="result__title">
    <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fdoc.rust%2Dlang.org%2Fbook%2F&amp;rut=def">The Rust Book</a>
  </h2>
  <a class="result__snippet" href="#">Read the official book.</a>
</div>
</body></html>
"##;

    #[test]
    fn parses_ddg_results_with_decoded_urls() {
        let hits = parse_ddg_results(SAMPLE_DDG_HTML, 5);
        assert_eq!(hits.len(), 2);
        assert_eq!(
            hits[0],
            SearchHit {
                title: "The Rust Programming Language".to_string(),
                url: "https://www.rust-lang.org/".to_string(),
                snippet: "A language empowering everyone to build reliable software.".to_string(),
            }
        );
        assert_eq!(hits[1].url, "https://doc.rust-lang.org/book/");
        assert_eq!(hits[1].title, "The Rust Book");
        assert_eq!(hits[1].snippet, "Read the official book.");
    }

    #[test]
    fn ddg_results_respect_max() {
        let hits = parse_ddg_results(SAMPLE_DDG_HTML, 1);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn html_to_text_strips_tags_scripts_and_entities() {
        let html = "<html><head><style>p{color:red}</style></head><body>\
            <script>alert('x')</script><p>Hello &amp; welcome to   Rust&#39;s world</p></body></html>";
        assert_eq!(html_to_text(html), "Hello & welcome to Rust's world");
    }

    #[test]
    fn blocked_ip_detects_private_ranges() {
        use std::net::Ipv4Addr;
        use std::net::Ipv6Addr;

        // IPv4 blocked
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))));
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(172, 16, 0, 1))));
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(169, 254, 169, 254))));
        assert!(is_blocked_ip(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0))));

        // IPv6 blocked
        assert!(is_blocked_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        assert!(is_blocked_ip(IpAddr::V6(Ipv6Addr::UNSPECIFIED)));
        assert!(is_blocked_ip(IpAddr::V6("fc00::1".parse().unwrap())));
        assert!(is_blocked_ip(IpAddr::V6("fe80::1".parse().unwrap())));

        // Public IPs — not blocked
        assert!(!is_blocked_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
        assert!(!is_blocked_ip(IpAddr::V4(Ipv4Addr::new(1, 1, 1, 1))));
        assert!(!is_blocked_ip(IpAddr::V6(
            "2001:4860:4860::8888".parse().unwrap()
        )));
    }

    #[tokio::test]
    async fn validate_url_safety_rejects_internal_ips() {
        // IP literal in URL
        assert!(validate_url_safety("http://127.0.0.1/admin").await.is_err());
        assert!(
            validate_url_safety("http://169.254.169.254/latest/meta-data/")
                .await
                .is_err()
        );
        assert!(
            validate_url_safety("http://10.0.0.1/internal")
                .await
                .is_err()
        );
        assert!(validate_url_safety("http://[::1]/admin").await.is_err());
    }

    #[tokio::test]
    async fn validate_url_safety_allows_public_urls() {
        // These should pass DNS resolution and not hit blocked ranges.
        // Using well-known public hostnames.
        let result = validate_url_safety("https://www.rust-lang.org/").await;
        assert!(
            result.is_ok(),
            "rust-lang.org should be allowed: {result:?}"
        );

        let result = validate_url_safety("https://example.com/").await;
        assert!(result.is_ok(), "example.com should be allowed: {result:?}");
    }
}
