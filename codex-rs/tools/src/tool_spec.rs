use crate::FreeformTool;
use crate::JsonSchema;
use crate::LoadableToolSpec;
use crate::NamespaceToolSpecMode;
use crate::ResponsesApiNamespace;
use crate::ResponsesApiTool;
use crate::flatten_responses_api_namespace;
use serde::Serialize;
use serde_json::Value;

/// When serialized as JSON, this produces a valid "Tool" in the OpenAI
/// Responses API.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(tag = "type")]
pub enum ToolSpec {
    #[serde(rename = "function")]
    Function(ResponsesApiTool),
    #[serde(rename = "namespace")]
    Namespace(ResponsesApiNamespace),
    #[serde(rename = "tool_search")]
    ToolSearch {
        execution: String,
        description: String,
        parameters: JsonSchema,
    },
    #[serde(rename = "image_generation")]
    ImageGeneration { output_format: String },
    #[serde(rename = "custom")]
    Freeform(FreeformTool),
}

/// Project tool specs for a given wire capability around namespaces.
pub fn serialize_tool_specs(
    specs: impl IntoIterator<Item = ToolSpec>,
    namespace_tool_spec_mode: NamespaceToolSpecMode,
) -> Vec<ToolSpec> {
    match namespace_tool_spec_mode {
        NamespaceToolSpecMode::Preserve => specs.into_iter().collect(),
        NamespaceToolSpecMode::Flatten => specs
            .into_iter()
            .flat_map(|spec| match spec {
                ToolSpec::Namespace(namespace) => flatten_responses_api_namespace(namespace)
                    .into_iter()
                    .map(ToolSpec::Function)
                    .collect(),
                spec => vec![spec],
            })
            .collect(),
    }
}

impl ToolSpec {
    pub fn name(&self) -> &str {
        match self {
            ToolSpec::Function(tool) => tool.name.as_str(),
            ToolSpec::Namespace(namespace) => namespace.name.as_str(),
            ToolSpec::ToolSearch { .. } => "tool_search",
            ToolSpec::ImageGeneration { .. } => "image_generation",
            ToolSpec::Freeform(tool) => tool.name.as_str(),
        }
    }
}

impl From<LoadableToolSpec> for ToolSpec {
    fn from(value: LoadableToolSpec) -> Self {
        match value {
            LoadableToolSpec::Function(tool) => ToolSpec::Function(tool),
            LoadableToolSpec::Namespace(namespace) => ToolSpec::Namespace(namespace),
        }
    }
}

/// Returns JSON values that are compatible with Function Calling in the
/// Responses API:
/// https://platform.openai.com/docs/guides/function-calling?api-mode=responses
pub fn create_tools_json_for_responses_api(
    tools: &[ToolSpec],
) -> Result<Vec<Value>, serde_json::Error> {
    let mut tools_json = Vec::new();

    for tool in tools {
        let json = serde_json::to_value(tool)?;
        tools_json.push(json);
    }

    Ok(tools_json)
}

/// Returns Anthropic-compatible tool definitions: `{name, description, input_schema}`.
///
/// Prefer feeding this already-flattened specs ([`serialize_tool_specs`] with
/// [`NamespaceToolSpecMode::Flatten`]). Namespace wrappers still present are
/// flattened with [`ToolName::canonical_flat_name`] so two MCP servers cannot
/// collide on a short child name. Responses-API hosted tools
/// (image_generation/tool_search) have no Anthropic analogue and are dropped.
/// Freeform/grammar tools become a single string arg.
pub fn create_tools_json_for_anthropic(
    tools: &[ToolSpec],
) -> Result<Vec<Value>, serde_json::Error> {
    let mut tools_json = Vec::new();
    // Defense in depth: flatten namespaces even if the planner forgot to.
    let tools = serialize_tool_specs(tools.iter().cloned(), NamespaceToolSpecMode::Flatten);
    for tool in &tools {
        match tool {
            ToolSpec::Function(t) => {
                tools_json.push(anthropic_tool(&t.name, &t.description, &t.parameters)?);
            }
            ToolSpec::Namespace(_) => {
                // unreachable after Flatten — kept for exhaustiveness.
            }
            ToolSpec::Freeform(t) => {
                // ponytail: lossy — a grammar-constrained tool becomes a plain string arg.
                tools_json.push(serde_json::json!({
                    "name": t.name,
                    "description": t.description,
                    "input_schema": {
                        "type": "object",
                        "properties": { "input": { "type": "string" } },
                        "required": ["input"],
                    },
                }));
            }
            ToolSpec::ToolSearch { .. } | ToolSpec::ImageGeneration { .. } => {}
        }
    }
    Ok(tools_json)
}

fn anthropic_tool(
    name: &str,
    description: &str,
    parameters: &JsonSchema,
) -> Result<Value, serde_json::Error> {
    Ok(serde_json::json!({
        "name": name,
        "description": description,
        "input_schema": serde_json::to_value(parameters)?,
    }))
}

#[cfg(test)]
#[path = "tool_spec_tests.rs"]
mod tests;
