use super::*;
use pretty_assertions::assert_eq;

fn metric_call_tool_result(
    is_error: bool,
    structured_content: Option<serde_json::Value>,
) -> CallToolResult {
    CallToolResult {
        content: Vec::new(),
        structured_content,
        is_error: Some(is_error),
        meta: None,
    }
}

#[test]
fn mcp_call_metric_tags_replace_private_names_with_categories() {
    assert_eq!(
        mcp_call_metric_tags(
            "error",
            "docs server",
            "search docs",
            Some("connector/docs"),
            Some("Docs connector"),
        ),
        vec![
            ("status", "error".to_string()),
            ("server", "custom".to_string()),
            ("tool", "connector".to_string()),
            ("connector_present", "true".to_string()),
        ],
    );
}

#[test]
fn mcp_call_metric_outcome_distinguishes_request_and_tool_errors() {
    assert_eq!(
        mcp_call_metric_outcome(&Ok(metric_call_tool_result(
            /*is_error*/ false, /*structured_content*/ None,
        )),),
        McpCallMetricOutcome {
            status: "ok",
            error_type: None,
            error_category: None,
        }
    );
    assert_eq!(
        mcp_call_metric_outcome(&Ok(metric_call_tool_result(
            /*is_error*/ true,
            Some(serde_json::json!({"error_code": "RATE_LIMITED"})),
        )),),
        McpCallMetricOutcome {
            status: "error",
            error_type: Some(MCP_CALL_ERROR_TYPE_TOOL_RESULT),
            error_category: Some("rate_limit"),
        }
    );
    assert_eq!(
        mcp_call_metric_outcome(&Err("connection closed".to_string())),
        McpCallMetricOutcome {
            status: "error",
            error_type: Some(MCP_CALL_ERROR_TYPE_MCP_REQUEST),
            error_category: Some(MCP_CALL_ERROR_CATEGORY_UNKNOWN),
        }
    );
}

#[test]
fn mcp_call_metric_outcome_buckets_server_tool_error_codes() {
    let result = Ok(metric_call_tool_result(
        /*is_error*/ true,
        Some(serde_json::json!({"error_code": "arbitrary-user-value"})),
    ));

    assert_eq!(
        mcp_call_metric_outcome(&result),
        McpCallMetricOutcome {
            status: "error",
            error_type: Some(MCP_CALL_ERROR_TYPE_TOOL_RESULT),
            error_category: Some("other"),
        }
    );
}

#[test]
fn mcp_call_metric_outcome_reads_auth_error_code_from_meta() {
    let result = CallToolResult {
        content: Vec::new(),
        structured_content: None,
        is_error: Some(true),
        meta: Some(serde_json::json!({
            MCP_TOOL_CODEX_APPS_META_KEY: {
                "connector_auth_failure": {
                    "is_auth_failure": true,
                    "error_code": "UNAUTHORIZED",
                },
            },
        })),
    };

    assert_eq!(
        mcp_call_metric_outcome(&Ok(result)),
        McpCallMetricOutcome {
            status: "error",
            error_type: Some(MCP_CALL_ERROR_TYPE_TOOL_RESULT),
            error_category: Some("authentication"),
        }
    );
}

#[test]
fn mcp_call_metric_outcome_does_not_preserve_arbitrary_error_code() {
    let raw_error_code = format!("BAD CODE {}", "x".repeat(300));
    let result = Ok(metric_call_tool_result(
        /*is_error*/ true,
        Some(serde_json::json!({"error_code": raw_error_code})),
    ));

    assert_eq!(
        mcp_call_metric_outcome(&result),
        McpCallMetricOutcome {
            status: "error",
            error_type: Some(MCP_CALL_ERROR_TYPE_TOOL_RESULT),
            error_category: Some("other"),
        }
    );
}
