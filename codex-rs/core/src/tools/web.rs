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
use std::sync::Arc;
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
const USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 \
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
    fn search<'a>(
        &'a self,
        query: &'a str,
        max: usize,
    ) -> BoxFuture<'a, Result<Vec<SearchHit>>>;
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
    fn search<'a>(
        &'a self,
        query: &'a str,
        max: usize,
    ) -> BoxFuture<'a, Result<Vec<SearchHit>>> {
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
fn parse_ddg_results(html: &str, max: usize) -> Vec<SearchHit> {
    let anchor_re = Regex::new(r#"(?is)<a[^>]*class="result__a"[^>]*href="([^"]*)"[^>]*>(.*?)</a>"#)
        .expect("static regex compiles");
    let snippet_re = Regex::new(r#"(?is)<a[^>]*class="result__snippet"[^>]*>(.*?)</a>"#)
        .expect("static regex compiles");

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
    let query = href.splitn(2, '?').nth(1).unwrap_or("");
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
pub fn html_to_text(html: &str) -> String {
    let script_style_re = Regex::new(r"(?is)<(script|style)[^>]*>.*?</(script|style)>")
        .expect("static regex compiles");
    let tag_re = Regex::new(r"(?s)<[^>]+>").expect("static regex compiles");

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

async fn fetch_url_as_text(url: &str) -> Result<String> {
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
            let WebSearchArgs {
                query,
                max_results,
            } = parse_arguments(arguments)?;
            let max = max_results.unwrap_or(DEFAULT_MAX_RESULTS).clamp(1, MAX_RESULTS_CAP);

            let hits = self.backend.search(&query, max).await.map_err(|err| {
                FunctionCallError::RespondToModel(format!("web_search failed: {err:#}"))
            })?;

            let text = if hits.is_empty() {
                format!("No results found for query: {query}")
            } else {
                hits.iter()
                    .enumerate()
                    .map(|(idx, hit)| {
                        format!(
                            "{}. {}\n{}\n{}",
                            idx + 1,
                            hit.title,
                            hit.url,
                            hit.snippet
                        )
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
            description: "Fetch a URL and return its readable text content (HTML stripped, truncated)."
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
}
