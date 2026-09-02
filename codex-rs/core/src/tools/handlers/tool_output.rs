use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::output_store::MAX_CONTEXT_LINES;
use crate::tools::output_store::MAX_PATTERN_BYTES;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use codex_protocol::models::ResponseInputItem;
use codex_tools::JsonSchema;
use codex_tools::JsonToolOutput;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_utils_output_truncation::TruncationPolicy;
use serde::Deserialize;
use serde_json::Value as JsonValue;
use serde_json::json;
use std::collections::BTreeMap;

use super::parse_arguments;

const NAMESPACE: &str = "tool_output";
const READ_TOOL_NAME: &str = "read";
const SEARCH_TOOL_NAME: &str = "search";

#[derive(Debug, Deserialize)]
struct ReadArgs {
    handle: String,
    offset: usize,
    max_tokens: usize,
}

#[derive(Debug, Deserialize)]
struct SearchArgs {
    handle: String,
    pattern: String,
    context_lines: usize,
}

struct RetrievalOutput(JsonValue);

impl ToolOutput for RetrievalOutput {
    fn log_output(&self) -> String {
        self.0.to_string()
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        JsonToolOutput::new(self.0.clone()).to_response_item(call_id, payload)
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        self.0.clone()
    }

    fn untruncated_text(&self, _payload: &ToolPayload) -> Option<String> {
        // Retrieval output must never become another recoverable handle.
        None
    }
}

pub struct ToolOutputReadHandler;

impl ToolExecutor<ToolInvocation> for ToolOutputReadHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(NAMESPACE, READ_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: NAMESPACE.to_string(),
            description: "Read recoverable tool output from this session.".to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: READ_TOOL_NAME.to_string(),
                description: "Read a bounded token window from a recoverable tool-output handle."
                    .to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::from([
                        (
                            "handle".to_string(),
                            JsonSchema::string(Some(
                                "Opaque handle returned with oversized tool output.".to_string(),
                            )),
                        ),
                        (
                            "offset".to_string(),
                            JsonSchema::integer(Some(
                                "Zero-based token offset to read from.".to_string(),
                            )),
                        ),
                        (
                            "max_tokens".to_string(),
                            JsonSchema::integer(Some(
                                "Maximum tokens to return. Must be positive and cannot exceed the active tool-output limit."
                                    .to_string(),
                            )),
                        ),
                    ]),
                    Some(vec![
                        "handle".to_string(),
                        "offset".to_string(),
                        "max_tokens".to_string(),
                    ]),
                    Some(false.into()),
                ),
                output_schema: None,
            })],
        })
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "tool_output.read received unsupported payload".to_string(),
                ));
            };
            let args: ReadArgs = parse_arguments(&arguments)?;
            let max_tokens = validate_max_tokens(
                args.max_tokens,
                invocation.turn.model_info().truncation_policy.into(),
            )?;
            let output = invocation
                .session
                .tool_output_store
                .read(&args.handle, args.offset, max_tokens)
                .await
                .map_err(store_error)?;
            Ok(boxed_tool_output(RetrievalOutput(json!({
                "content": output.content,
                "next_offset": output.next_offset,
                "total_tokens": output.total_tokens,
            }))))
        })
    }
}

impl CoreToolRuntime for ToolOutputReadHandler {}

pub struct ToolOutputSearchHandler;

impl ToolExecutor<ToolInvocation> for ToolOutputSearchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(NAMESPACE, SEARCH_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Namespace(ResponsesApiNamespace {
            name: NAMESPACE.to_string(),
            description: "Read recoverable tool output from this session.".to_string(),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: SEARCH_TOOL_NAME.to_string(),
                description: "Search a recoverable tool-output handle for a literal pattern with bounded line context."
                    .to_string(),
                strict: false,
                defer_loading: None,
                parameters: JsonSchema::object(
                    BTreeMap::from([
                        (
                            "handle".to_string(),
                            JsonSchema::string(Some(
                                "Opaque handle returned with oversized tool output.".to_string(),
                            )),
                        ),
                        (
                            "pattern".to_string(),
                            JsonSchema::string(Some(
                                "Literal, non-empty text to find.".to_string(),
                            )),
                        ),
                        (
                            "context_lines".to_string(),
                            JsonSchema::integer(Some(format!(
                                "Number of lines before and after each match, from 0 through {MAX_CONTEXT_LINES}."
                            ))),
                        ),
                    ]),
                    Some(vec![
                        "handle".to_string(),
                        "pattern".to_string(),
                        "context_lines".to_string(),
                    ]),
                    Some(false.into()),
                ),
                output_schema: None,
            })],
        })
    }

    fn handle<'a>(&'a self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "tool_output.search received unsupported payload".to_string(),
                ));
            };
            let args: SearchArgs = parse_arguments(&arguments)?;
            validate_search_args(&args)?;
            let policy: TruncationPolicy = invocation.turn.model_info().truncation_policy.into();
            let output = invocation
                .session
                .tool_output_store
                .search(
                    &args.handle,
                    &args.pattern,
                    args.context_lines,
                    policy.token_budget(),
                )
                .await
                .map_err(store_error)?;
            let matches = output
                .matches
                .into_iter()
                .map(|result| {
                    json!({
                        "line_number": result.line_number,
                        "context_start_line": result.context_start_line,
                        "context_end_line": result.context_end_line,
                        "content": result.content,
                    })
                })
                .collect::<Vec<_>>();
            Ok(boxed_tool_output(RetrievalOutput(json!({
                "matches": matches,
                "total_matches": output.total_matches,
                "truncated": output.truncated,
            }))))
        })
    }
}

impl CoreToolRuntime for ToolOutputSearchHandler {}

fn validate_max_tokens(
    max_tokens: usize,
    policy: TruncationPolicy,
) -> Result<usize, FunctionCallError> {
    if max_tokens == 0 {
        return Err(FunctionCallError::RespondToModel(
            "max_tokens must be greater than zero".to_string(),
        ));
    }
    Ok(max_tokens.min(policy.token_budget()))
}

fn validate_search_args(args: &SearchArgs) -> Result<(), FunctionCallError> {
    if args.pattern.is_empty() {
        return Err(FunctionCallError::RespondToModel(
            "pattern must not be empty".to_string(),
        ));
    }
    if args.pattern.len() > MAX_PATTERN_BYTES {
        return Err(FunctionCallError::RespondToModel(format!(
            "pattern exceeds the maximum size of {MAX_PATTERN_BYTES} bytes"
        )));
    }
    if args.context_lines > MAX_CONTEXT_LINES {
        return Err(FunctionCallError::RespondToModel(format!(
            "context_lines must be between 0 and {MAX_CONTEXT_LINES}"
        )));
    }
    Ok(())
}

fn store_error(error: crate::tools::output_store::ToolOutputStoreError) -> FunctionCallError {
    FunctionCallError::RespondToModel(error.to_string())
}

#[cfg(test)]
#[path = "tool_output_tests.rs"]
mod tests;
