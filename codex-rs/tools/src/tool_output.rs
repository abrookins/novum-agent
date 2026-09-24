use codex_protocol::models::DEFAULT_IMAGE_DETAIL;
use codex_protocol::models::FunctionCallOutputBody;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ImageReference;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::protocol::TruncationPolicy;
use codex_utils_output_truncation::truncate_text;
use codex_utils_string::take_bytes_at_char_boundary;
use serde_json::Value as JsonValue;

use crate::ToolPayload;

const TELEMETRY_PREVIEW_MAX_BYTES: usize = 2 * 1024;
const TELEMETRY_PREVIEW_MAX_LINES: usize = 64;
const TELEMETRY_PREVIEW_TRUNCATION_NOTICE: &str = "[... telemetry preview truncated ...]";

/// Opaque reference to a session-scoped, recoverable textual tool output.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoverableToolOutput {
    pub handle: String,
    pub byte_len: usize,
    pub token_count: usize,
}

/// Model-facing output contract returned by executable tool runtimes.
pub trait ToolOutput: Send {
    /// Returns a deliberately lossy diagnostic representation suitable for telemetry.
    fn log_output(&self) -> String;

    fn success_for_logging(&self) -> bool;

    /// Finalizes output using the same completed handler duration reported in tool-call logs.
    /// Called before recording model-visible history; implementations must not measure time here.
    fn set_handler_duration_ms(&mut self, _handler_duration_ms: u64) {}

    /// Whether this output contains external context that should disable memory generation when
    /// `memories.disable_on_external_context` is enabled.
    fn contains_external_context(&self) -> bool {
        false
    }

    /// Overrides history's fallback token limit after tool-specific truncation.
    /// Include any serialization allowance; history uses this limit unchanged.
    fn fallback_token_limit_override(&self) -> Option<usize> {
        None
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem;

    /// Returns the model-visible response after a recoverable-output preview
    /// replaces its textual content.
    ///
    /// Outputs with response metadata can override this to retain it while
    /// replacing only the recoverable text.
    fn to_response_item_with_recoverable_preview(
        &self,
        call_id: &str,
        payload: &ToolPayload,
        preview: String,
    ) -> ResponseInputItem {
        replace_response_text(self.to_response_item(call_id, payload), preview)
    }

    /// Returns the tool call id exposed to `PostToolUse` hooks for this output.
    fn post_tool_use_id(&self, call_id: &str) -> String {
        call_id.to_string()
    }

    /// Returns the tool input exposed to `PostToolUse` hooks for this output.
    fn post_tool_use_input(&self, _payload: &ToolPayload) -> Option<JsonValue> {
        None
    }

    /// Returns the stable value exposed to `PostToolUse` hooks for this tool output.
    ///
    /// Tool handlers decide whether a tool participates in `PostToolUse`, but
    /// this method lets the output type own any conversion from model-facing
    /// response content to hook-facing data. Returning `None` means the output
    /// should not produce a post-use hook payload, not merely that the tool had
    /// empty output.
    fn post_tool_use_response(&self, _call_id: &str, _payload: &ToolPayload) -> Option<JsonValue> {
        None
    }

    fn code_mode_result(&self, payload: &ToolPayload) -> JsonValue {
        response_input_to_code_mode_result(self.to_response_item("", payload))
    }

    /// Returns the bounded result supplied to the Code Mode runtime.
    fn code_mode_result_with_policy(
        &self,
        payload: &ToolPayload,
        policy: TruncationPolicy,
    ) -> JsonValue {
        truncate_code_mode_result(self.code_mode_result(payload), policy)
    }

    /// Returns the Code Mode value after its textual output is replaced with a
    /// recoverable-output preview.
    ///
    /// Outputs with a structured Code Mode contract can override this to keep
    /// their metadata and replace only their text field.
    fn code_mode_result_with_recoverable_preview(
        &self,
        _payload: &ToolPayload,
        preview: String,
    ) -> JsonValue {
        JsonValue::String(preview)
    }

    /// Returns the bounded Code Mode value after textual output is replaced
    /// with a recoverable-output preview.
    ///
    /// Structured output types can override this to preserve their shape when
    /// the preview itself must be reduced further.
    fn code_mode_result_with_recoverable_preview_and_policy(
        &self,
        payload: &ToolPayload,
        preview: String,
        policy: TruncationPolicy,
    ) -> JsonValue {
        truncate_code_mode_result(
            self.code_mode_result_with_recoverable_preview(payload, preview),
            policy,
        )
    }

    /// Returns lossless textual output suitable for a recoverable handle.
    ///
    /// Non-text content is deliberately excluded so image, audio, and mixed
    /// content outputs retain their existing response representation.
    fn untruncated_text(&self, payload: &ToolPayload) -> Option<String> {
        response_input_to_plain_text(self.to_response_item("", payload))
    }

    /// Returns output that was captured before a transport-specific preview
    /// discarded its middle bytes.
    fn recoverable_output(&self) -> Option<RecoverableToolOutput> {
        None
    }

    /// Returns the budget for the model-visible preview that accompanies a
    /// recoverable-output handle.
    fn recoverable_preview_policy(&self, policy: TruncationPolicy) -> TruncationPolicy {
        policy
    }

    /// Reports configured source capture only after acceptance; `None` means no capture attempt.
    fn tool_result_sources(&self) -> Option<codex_protocol::models::ToolResultSources> {
        None
    }

    /// Borrows original host-only metadata for recording, not for model output or logging.
    fn tool_result_metadata(&self) -> Option<&JsonValue> {
        None
    }
}

impl<T> ToolOutput for Box<T>
where
    T: ToolOutput + ?Sized,
{
    fn log_output(&self) -> String {
        (**self).log_output()
    }

    fn success_for_logging(&self) -> bool {
        (**self).success_for_logging()
    }

    fn set_handler_duration_ms(&mut self, handler_duration_ms: u64) {
        (**self).set_handler_duration_ms(handler_duration_ms);
    }

    fn contains_external_context(&self) -> bool {
        (**self).contains_external_context()
    }

    fn fallback_token_limit_override(&self) -> Option<usize> {
        (**self).fallback_token_limit_override()
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        (**self).to_response_item(call_id, payload)
    }

    fn to_response_item_with_recoverable_preview(
        &self,
        call_id: &str,
        payload: &ToolPayload,
        preview: String,
    ) -> ResponseInputItem {
        (**self).to_response_item_with_recoverable_preview(call_id, payload, preview)
    }

    fn post_tool_use_id(&self, call_id: &str) -> String {
        (**self).post_tool_use_id(call_id)
    }

    fn post_tool_use_input(&self, payload: &ToolPayload) -> Option<JsonValue> {
        (**self).post_tool_use_input(payload)
    }

    fn post_tool_use_response(&self, call_id: &str, payload: &ToolPayload) -> Option<JsonValue> {
        (**self).post_tool_use_response(call_id, payload)
    }

    fn code_mode_result(&self, payload: &ToolPayload) -> JsonValue {
        (**self).code_mode_result(payload)
    }

    fn code_mode_result_with_policy(
        &self,
        payload: &ToolPayload,
        policy: TruncationPolicy,
    ) -> JsonValue {
        (**self).code_mode_result_with_policy(payload, policy)
    }

    fn code_mode_result_with_recoverable_preview(
        &self,
        payload: &ToolPayload,
        preview: String,
    ) -> JsonValue {
        (**self).code_mode_result_with_recoverable_preview(payload, preview)
    }

    fn code_mode_result_with_recoverable_preview_and_policy(
        &self,
        payload: &ToolPayload,
        preview: String,
        policy: TruncationPolicy,
    ) -> JsonValue {
        (**self).code_mode_result_with_recoverable_preview_and_policy(payload, preview, policy)
    }

    fn untruncated_text(&self, payload: &ToolPayload) -> Option<String> {
        (**self).untruncated_text(payload)
    }

    fn recoverable_output(&self) -> Option<RecoverableToolOutput> {
        (**self).recoverable_output()
    }

    fn recoverable_preview_policy(&self, policy: TruncationPolicy) -> TruncationPolicy {
        (**self).recoverable_preview_policy(policy)
    }

    fn tool_result_sources(&self) -> Option<codex_protocol::models::ToolResultSources> {
        (**self).tool_result_sources()
    }

    fn tool_result_metadata(&self) -> Option<&JsonValue> {
        (**self).tool_result_metadata()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JsonToolOutput {
    value: JsonValue,
    success: Option<bool>,
    contains_external_context: bool,
}

impl JsonToolOutput {
    pub fn new(value: JsonValue) -> Self {
        Self {
            value,
            success: Some(true),
            contains_external_context: false,
        }
    }

    pub fn with_success(value: JsonValue, success: Option<bool>) -> Self {
        Self {
            value,
            success,
            contains_external_context: false,
        }
    }

    pub fn with_external_context(mut self) -> Self {
        self.contains_external_context = true;
        self
    }
}

impl ToolOutput for JsonToolOutput {
    fn log_output(&self) -> String {
        telemetry_preview(&self.value.to_string())
    }

    fn success_for_logging(&self) -> bool {
        self.success.unwrap_or(true)
    }

    fn contains_external_context(&self) -> bool {
        self.contains_external_context
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        let output = FunctionCallOutputPayload {
            body: FunctionCallOutputBody::Text(self.value.to_string()),
            success: self.success,
        };

        if matches!(payload, ToolPayload::Custom { .. }) {
            return ResponseInputItem::CustomToolCallOutput {
                call_id: call_id.to_string(),
                name: None,
                output,
            };
        }

        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output,
        }
    }

    fn post_tool_use_response(&self, _call_id: &str, _payload: &ToolPayload) -> Option<JsonValue> {
        Some(self.value.clone())
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        self.value.clone()
    }
}

impl ToolOutput for codex_protocol::mcp::CallToolResult {
    fn log_output(&self) -> String {
        let output = self.as_function_call_output_payload();
        let preview = output.body.to_text().unwrap_or_else(|| output.to_string());
        telemetry_preview(&preview)
    }

    fn success_for_logging(&self) -> bool {
        self.success()
    }

    fn to_response_item(&self, call_id: &str, _payload: &ToolPayload) -> ResponseInputItem {
        ResponseInputItem::McpToolCallOutput {
            call_id: call_id.to_string(),
            output: self.clone(),
        }
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        let mut result = serde_json::to_value(self).unwrap_or_else(|err| {
            JsonValue::String(format!("failed to serialize mcp result: {err}"))
        });
        // MCP result metadata is private to clients and must not reach Code Mode.
        if let JsonValue::Object(fields) = &mut result {
            fields.remove("_meta");
        }
        result
    }
}

fn truncate_code_mode_result(result: JsonValue, policy: TruncationPolicy) -> JsonValue {
    match result {
        JsonValue::String(text) => JsonValue::String(truncate_text(&text, policy)),
        value => {
            let serialized = serde_json::to_string(&value)
                .unwrap_or_else(|err| format!("failed to serialize Code Mode tool result: {err}"));
            let truncated = truncate_text(&serialized, policy);
            if truncated == serialized {
                value
            } else {
                JsonValue::String(truncated)
            }
        }
    }
}

fn response_input_to_plain_text(response: ResponseInputItem) -> Option<String> {
    match response {
        ResponseInputItem::FunctionCallOutput { output, .. }
        | ResponseInputItem::CustomToolCallOutput { output, .. } => body_to_plain_text(output.body),
        // Preserve raw MCP content arrays. `McpToolOutput` overrides this
        // method when it adapts textual MCP output to a function-call result.
        ResponseInputItem::McpToolCallOutput { .. } => None,
        ResponseInputItem::Message { content, .. } => {
            let mut texts = Vec::with_capacity(content.len());
            for item in content {
                match item {
                    codex_protocol::models::ContentItem::InputText { text }
                    | codex_protocol::models::ContentItem::OutputText { text } => texts.push(text),
                    codex_protocol::models::ContentItem::InputImage { .. }
                    | codex_protocol::models::ContentItem::InputAudio { .. } => return None,
                }
            }
            Some(texts.join("\n"))
        }
        ResponseInputItem::ToolSearchOutput { .. } => None,
    }
}

fn replace_response_text(response: ResponseInputItem, text: String) -> ResponseInputItem {
    match response {
        ResponseInputItem::FunctionCallOutput {
            call_id,
            mut output,
        } => {
            output.body = FunctionCallOutputBody::Text(text);
            ResponseInputItem::FunctionCallOutput { call_id, output }
        }
        ResponseInputItem::CustomToolCallOutput {
            call_id,
            name,
            mut output,
        } => {
            output.body = FunctionCallOutputBody::Text(text);
            ResponseInputItem::CustomToolCallOutput {
                call_id,
                name,
                output,
            }
        }
        // MCP content arrays can contain extensions that have no equivalent
        // FunctionCallOutput representation. Leave them intact instead of
        // collapsing their representation to a single text item.
        ResponseInputItem::McpToolCallOutput { .. } => response,
        ResponseInputItem::Message { role, phase, .. } => ResponseInputItem::Message {
            role,
            phase,
            content: vec![codex_protocol::models::ContentItem::InputText { text }],
        },
        ResponseInputItem::ToolSearchOutput { .. } => response,
    }
}

fn body_to_plain_text(body: FunctionCallOutputBody) -> Option<String> {
    match body {
        FunctionCallOutputBody::Text(text) => Some(text),
        FunctionCallOutputBody::ContentItems(items) => {
            let mut texts = Vec::with_capacity(items.len());
            for item in items {
                match item {
                    FunctionCallOutputContentItem::InputText { text } => texts.push(text),
                    FunctionCallOutputContentItem::InputImage { .. }
                    | FunctionCallOutputContentItem::InputAudio { .. }
                    | FunctionCallOutputContentItem::EncryptedContent { .. } => return None,
                }
            }
            Some(texts.join("\n"))
        }
    }
}

fn response_input_to_code_mode_result(response: ResponseInputItem) -> JsonValue {
    match response {
        ResponseInputItem::Message { content, .. } => content_items_to_code_mode_result(
            &content
                .into_iter()
                .map(|item| match item {
                    codex_protocol::models::ContentItem::InputText { text }
                    | codex_protocol::models::ContentItem::OutputText { text } => {
                        FunctionCallOutputContentItem::InputText { text }
                    }
                    codex_protocol::models::ContentItem::InputImage { image, detail } => {
                        FunctionCallOutputContentItem::InputImage {
                            image,
                            detail: detail.or(Some(DEFAULT_IMAGE_DETAIL)),
                        }
                    }
                    codex_protocol::models::ContentItem::InputAudio { audio_url } => {
                        FunctionCallOutputContentItem::InputAudio { audio_url }
                    }
                })
                .collect::<Vec<_>>(),
        ),
        ResponseInputItem::FunctionCallOutput { output, .. }
        | ResponseInputItem::CustomToolCallOutput { output, .. } => match output.body {
            FunctionCallOutputBody::Text(text) => JsonValue::String(text),
            FunctionCallOutputBody::ContentItems(items) => {
                content_items_to_code_mode_result(&items)
            }
        },
        ResponseInputItem::ToolSearchOutput { tools, .. } => JsonValue::Array(tools),
        ResponseInputItem::McpToolCallOutput { output, .. } => serde_json::to_value(output)
            .unwrap_or_else(|err| {
                JsonValue::String(format!("failed to serialize mcp result: {err}"))
            }),
    }
}

fn content_items_to_code_mode_result(items: &[FunctionCallOutputContentItem]) -> JsonValue {
    JsonValue::String(
        items
            .iter()
            .filter_map(|item| match item {
                FunctionCallOutputContentItem::InputText { text } if !text.trim().is_empty() => {
                    Some(text.clone())
                }
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::Inline { image_url },
                    ..
                } if !image_url.trim().is_empty() => Some(image_url.clone()),
                FunctionCallOutputContentItem::InputImage {
                    image: ImageReference::File { file_id },
                    ..
                } if !file_id.trim().is_empty() => Some(file_id.clone()),
                FunctionCallOutputContentItem::InputAudio { audio_url }
                    if !audio_url.trim().is_empty() =>
                {
                    Some(audio_url.clone())
                }
                FunctionCallOutputContentItem::InputText { .. }
                | FunctionCallOutputContentItem::InputImage { .. }
                | FunctionCallOutputContentItem::InputAudio { .. }
                | FunctionCallOutputContentItem::EncryptedContent { .. } => None,
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn telemetry_preview(content: &str) -> String {
    let truncated_slice = take_bytes_at_char_boundary(content, TELEMETRY_PREVIEW_MAX_BYTES);
    let truncated_by_bytes = truncated_slice.len() < content.len();

    let mut preview = String::new();
    let mut lines_iter = truncated_slice.lines();
    for idx in 0..TELEMETRY_PREVIEW_MAX_LINES {
        match lines_iter.next() {
            Some(line) => {
                if idx > 0 {
                    preview.push('\n');
                }
                preview.push_str(line);
            }
            None => break,
        }
    }
    let truncated_by_lines = lines_iter.next().is_some();

    if !truncated_by_bytes && !truncated_by_lines {
        return content.to_string();
    }

    if preview.len() < truncated_slice.len()
        && truncated_slice
            .as_bytes()
            .get(preview.len())
            .is_some_and(|byte| *byte == b'\n')
    {
        preview.push('\n');
    }

    if !preview.is_empty() && !preview.ends_with('\n') {
        preview.push('\n');
    }
    preview.push_str(TELEMETRY_PREVIEW_TRUNCATION_NOTICE);

    preview
}

#[cfg(test)]
#[path = "tool_output_tests.rs"]
mod tests;
