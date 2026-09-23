use std::time::Duration;

use codex_mcp::CODEX_APPS_MCP_SERVER_NAME;
use codex_mcp::MCP_TOOL_CODEX_APPS_META_KEY;
use codex_otel::SessionTelemetry;
use codex_protocol::mcp::CallToolResult;
use serde_json::Value as JsonValue;
use tracing::Span;

const MCP_CALL_COUNT_METRIC: &str = "codex.mcp.call";
const MCP_CALL_DURATION_METRIC: &str = "codex.mcp.call.duration_ms";
const MCP_CALL_ERROR_COUNT_METRIC: &str = "codex.mcp.call.error";
// No CallToolResult was received. This includes request setup, transport, timeout, protocol, and
// JSON-RPC failures; it does not imply that the request never reached the MCP server.
const MCP_CALL_ERROR_TYPE_MCP_REQUEST: &str = "mcp_request";
// The MCP server returned a CallToolResult with isError=true.
const MCP_CALL_ERROR_TYPE_TOOL_RESULT: &str = "tool_result";
const MCP_CALL_ERROR_CATEGORY_UNKNOWN: &str = "unknown";
const MCP_CALL_ERROR_TYPE_SPAN_ATTR: &str = "error.type";
const MCP_CALL_ERROR_CATEGORY_SPAN_ATTR: &str = "codex.mcp.error.category";

#[derive(Debug, PartialEq, Eq)]
pub(super) struct McpCallMetricOutcome {
    status: &'static str,
    error_type: Option<&'static str>,
    error_category: Option<&'static str>,
}

impl McpCallMetricOutcome {
    pub(super) fn from_status(status: &'static str) -> Self {
        Self {
            status,
            error_type: None,
            error_category: None,
        }
    }
}

pub(super) fn emit_mcp_call_metrics(
    session_telemetry: &SessionTelemetry,
    outcome: &McpCallMetricOutcome,
    server_name: &str,
    tool_name: &str,
    connector_id: Option<&str>,
    connector_name: Option<&str>,
    duration: Option<Duration>,
) {
    let tags = mcp_call_metric_tags(
        outcome.status,
        server_name,
        tool_name,
        connector_id,
        connector_name,
    );
    let tag_refs: Vec<(&str, &str)> = tags
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    session_telemetry.counter(MCP_CALL_COUNT_METRIC, /*inc*/ 1, &tag_refs);
    if let Some(duration) = duration {
        session_telemetry.record_duration(MCP_CALL_DURATION_METRIC, duration, &tag_refs);
    }

    let (Some(error_type), Some(error_category)) = (outcome.error_type, outcome.error_category)
    else {
        return;
    };
    let mut error_tags = tags;
    error_tags.push(("error_type", error_type.to_string()));
    error_tags.push(("error_category", error_category.to_string()));
    let error_tag_refs: Vec<(&str, &str)> = error_tags
        .iter()
        .map(|(key, value)| (*key, value.as_str()))
        .collect();
    session_telemetry.counter(MCP_CALL_ERROR_COUNT_METRIC, /*inc*/ 1, &error_tag_refs);
}

fn mcp_call_metric_tags(
    status: &str,
    server_name: &str,
    _tool_name: &str,
    connector_id: Option<&str>,
    connector_name: Option<&str>,
) -> Vec<(&'static str, String)> {
    let status = match status {
        "ok" => "ok",
        "error" => "error",
        _ => "other",
    };
    let server = if server_name == CODEX_APPS_MCP_SERVER_NAME {
        "codex_apps"
    } else {
        "custom"
    };
    let connector_present = connector_id.is_some_and(|value| !value.is_empty())
        || connector_name.is_some_and(|value| !value.is_empty());
    vec![
        ("status", status.to_string()),
        ("server", server.to_string()),
        (
            "tool",
            if connector_present {
                "connector"
            } else {
                "custom"
            }
            .to_string(),
        ),
        ("connector_present", connector_present.to_string()),
    ]
}

fn mcp_error_category(error_code: Option<&str>) -> &'static str {
    let Some(error_code) = error_code else {
        return MCP_CALL_ERROR_CATEGORY_UNKNOWN;
    };
    let error_code = error_code.to_ascii_uppercase();
    if error_code.contains("AUTH") || error_code.contains("UNAUTHORIZED") {
        "authentication"
    } else if error_code.contains("RATE") || error_code.contains("THROTTL") {
        "rate_limit"
    } else if error_code.contains("NOT_FOUND") {
        "not_found"
    } else if error_code.contains("INVALID") {
        "invalid_request"
    } else {
        "other"
    }
}

pub(super) fn mcp_call_metric_outcome(
    result: &Result<CallToolResult, String>,
) -> McpCallMetricOutcome {
    match result {
        Ok(result) if result.is_error.unwrap_or(false) => {
            let error_code = result
                .structured_content
                .as_ref()
                .and_then(JsonValue::as_object)
                .and_then(|structured_content| structured_content.get("error_code"))
                .and_then(JsonValue::as_str)
                .filter(|error_code| !error_code.is_empty())
                .or_else(|| {
                    result
                        .meta
                        .as_ref()
                        .and_then(JsonValue::as_object)
                        .and_then(|meta| meta.get(MCP_TOOL_CODEX_APPS_META_KEY))
                        .and_then(JsonValue::as_object)
                        .and_then(|codex_apps| codex_apps.get("connector_auth_failure"))
                        .and_then(JsonValue::as_object)
                        .filter(|auth_failure| {
                            auth_failure
                                .get("is_auth_failure")
                                .and_then(JsonValue::as_bool)
                                == Some(true)
                        })
                        .and_then(|auth_failure| auth_failure.get("error_code"))
                        .and_then(JsonValue::as_str)
                        .filter(|error_code| !error_code.is_empty())
                });
            McpCallMetricOutcome {
                status: "error",
                error_type: Some(MCP_CALL_ERROR_TYPE_TOOL_RESULT),
                error_category: Some(mcp_error_category(error_code)),
            }
        }
        Ok(_) => McpCallMetricOutcome::from_status("ok"),
        Err(_) => McpCallMetricOutcome {
            status: "error",
            error_type: Some(MCP_CALL_ERROR_TYPE_MCP_REQUEST),
            error_category: Some(MCP_CALL_ERROR_CATEGORY_UNKNOWN),
        },
    }
}

pub(super) fn record_mcp_call_outcome_span_telemetry(
    span: &Span,
    result: &Result<CallToolResult, String>,
) {
    let outcome = mcp_call_metric_outcome(result);
    let (Some(error_type), Some(error_category)) = (outcome.error_type, outcome.error_category)
    else {
        return;
    };
    span.record(MCP_CALL_ERROR_TYPE_SPAN_ATTR, error_type);
    span.record(MCP_CALL_ERROR_CATEGORY_SPAN_ATTR, error_category);
}

#[cfg(test)]
#[path = "telemetry_tests.rs"]
mod tests;
