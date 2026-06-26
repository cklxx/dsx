use crate::FreeformTool;
use crate::JsonSchema;
use crate::LoadableToolSpec;
use crate::ResponsesApiNamespace;
use crate::ResponsesApiNamespaceTool;
use crate::ResponsesApiTool;
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
/// Namespaced tools are flattened to their function name; Responses-API hosted
/// tools (web_search/image_generation/tool_search) have no Anthropic analogue
/// and are dropped. Freeform/grammar tools are exposed as a single string arg.
pub fn create_tools_json_for_anthropic(
    tools: &[ToolSpec],
) -> Result<Vec<Value>, serde_json::Error> {
    let mut tools_json = Vec::new();
    for tool in tools {
        match tool {
            ToolSpec::Function(t) => {
                tools_json.push(anthropic_tool(&t.name, &t.description, &t.parameters)?);
            }
            ToolSpec::Namespace(ns) => {
                for nt in &ns.tools {
                    let ResponsesApiNamespaceTool::Function(t) = nt;
                    tools_json.push(anthropic_tool(&t.name, &t.description, &t.parameters)?);
                }
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
