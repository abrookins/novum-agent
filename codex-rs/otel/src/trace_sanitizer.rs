use crate::metrics::bounded_environment_category;
use crate::metrics::bounded_originator_tag_value;
use crate::metrics::bounded_reasoning_effort_category;
use crate::metrics::bounded_service_tier_category;
use opentelemetry::InstrumentationScope;
use opentelemetry::KeyValue;
use opentelemetry::Value;
use opentelemetry::trace::Status;
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::OTelSdkResult;
use opentelemetry_sdk::trace::SpanData;
use opentelemetry_sdk::trace::SpanExporter;
use opentelemetry_semantic_conventions as semconv;
use std::time::Duration;

const APPROVED_EVENT_NAMES: &[&str] = &[
    "codex.api_request",
    "codex.auth_recovery",
    "codex.conversation_starts",
    "codex.plugin_install_elicitation_sent",
    "codex.plugin_install_suggestion",
    "codex.sandbox_outcome",
    "codex.sse_event",
    "codex.startup_phase",
    "codex.tool_decision",
    "codex.tool_result",
    "codex.turn_ttft",
    "codex.user_prompt",
    "codex.websocket_connect",
    "codex.websocket_request",
];

const APPROVED_EVENT_ATTRIBUTE_KEYS: &[&str] = &[
    "event.name",
    "duration_ms",
    "success",
    "http.response.status_code",
    "attempt",
    "input_token_count",
    "output_token_count",
    "cached_token_count",
    "cache_write_token_count",
    "reasoning_token_count",
    "tool_token_count",
    "arguments_length",
    "output_length",
    "output_line_count",
    "prompt_length",
    "text_input_count",
    "image_input_count",
    "local_image_input_count",
    "mcp_server_count",
    "initial_duration_ms",
    "escalated_duration_ms",
    "provider.category",
    "tool_name",
    "tool_origin",
    "mcp_tool",
    "event.kind",
    "error.type",
    "service_tier",
    "model_reasoning_effort",
    "reasoning_effort",
    "reasoning_summary",
    "approval_policy",
    "sandbox_policy",
    "startup.phase",
    "startup.status",
    "decision",
    "source",
    "outcome",
    "plugin_install.tool_type",
    "plugin_install.response_action",
    "plugin_install.user_confirmed",
    "plugin_install.completed",
    "auth.mode",
    "auth.step",
    "auth.outcome",
    "auth.error_code",
    "auth.state_changed",
    "auth.header_attached",
    "auth.header_name",
    "auth.retry_after_unauthorized",
    "auth.recovery_mode",
    "auth.recovery_phase",
    "auth.env_openai_api_key_present",
    "auth.env_codex_api_key_present",
    "auth.env_codex_api_key_enabled",
    "auth.env_provider_key_present",
    "auth.env_refresh_token_url_override_present",
    "auth.connection_reused",
];

const APPROVED_SPAN_NAMES: &[&str] = &[
    "dispatch_tool_call_with_code_mode_result",
    "dispatch_tool_call_with_terminal_outcome",
    "endpoint_session.stream_encoded_json_with",
    "handle_output_item_done",
    "handle_responses",
    "handle_tool_call",
    "list_models",
    "load",
    "mcp.tools.call",
    "message_from_assistant",
    "model_client.stream_responses_api",
    "op.dispatch.user_input",
    "responses.stream_request",
    "run_sampling_request",
    "run_turn",
    "session_init",
    "session_loop",
    "session_task.run",
    "session_task.turn",
    "stream_request",
    "submission_dispatch",
    "thread_spawn",
    "try_run_sampling_request",
    "world_state.build",
];

#[derive(Debug)]
pub(crate) struct PrivacyPreservingSpanExporter<E> {
    inner: E,
}

impl<E> PrivacyPreservingSpanExporter<E> {
    pub(crate) fn new(inner: E) -> Self {
        Self { inner }
    }
}

impl<E> SpanExporter for PrivacyPreservingSpanExporter<E>
where
    E: SpanExporter,
{
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        self.inner
            .export(batch.into_iter().map(sanitize_span).collect())
            .await
    }

    fn shutdown_with_timeout(&mut self, timeout: Duration) -> OTelSdkResult {
        self.inner.shutdown_with_timeout(timeout)
    }

    fn force_flush(&mut self) -> OTelSdkResult {
        self.inner.force_flush()
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.inner.set_resource(&sanitize_resource(resource));
    }
}

fn sanitize_span(mut span: SpanData) -> SpanData {
    span.name = if APPROVED_SPAN_NAMES.contains(&span.name.as_ref()) {
        span.name
    } else {
        "codex.operation".into()
    };
    span.attributes.clear();
    span.events.events.retain_mut(sanitize_event);
    for link in &mut span.links.links {
        link.attributes.clear();
    }
    if matches!(span.status, Status::Error { .. }) {
        span.status = Status::error("");
    }
    span.instrumentation_scope = InstrumentationScope::builder("codex").build();
    span
}

fn sanitize_event(event: &mut opentelemetry::trace::Event) -> bool {
    let Some(event_name) = event.attributes.iter().find_map(|attribute| {
        if attribute.key.as_str() != "event.name" {
            return None;
        }
        match &attribute.value {
            Value::String(value) => Some(value.as_str().to_string()),
            _ => None,
        }
    }) else {
        return false;
    };
    if !APPROVED_EVENT_NAMES.contains(&event_name.as_str()) {
        return false;
    }

    event.name = event_name.into();
    event
        .attributes
        .retain_mut(|attribute| sanitize_event_attribute(attribute, event.name.as_ref()));
    true
}

fn sanitize_event_attribute(attribute: &mut KeyValue, event_name: &str) -> bool {
    let key = attribute.key.as_str();
    if !APPROVED_EVENT_ATTRIBUTE_KEYS.contains(&key) {
        return false;
    }
    if key == "event.name" {
        return matches!(&attribute.value, Value::String(value) if value.as_str() == event_name);
    }
    if matches!(
        key,
        "duration_ms"
            | "http.response.status_code"
            | "attempt"
            | "input_token_count"
            | "output_token_count"
            | "cached_token_count"
            | "cache_write_token_count"
            | "reasoning_token_count"
            | "tool_token_count"
            | "arguments_length"
            | "output_length"
            | "output_line_count"
            | "prompt_length"
            | "text_input_count"
            | "image_input_count"
            | "local_image_input_count"
            | "mcp_server_count"
            | "initial_duration_ms"
            | "escalated_duration_ms"
    ) {
        return sanitize_nonnegative_integer(attribute);
    }
    if matches!(
        key,
        "success"
            | "mcp_tool"
            | "plugin_install.user_confirmed"
            | "plugin_install.completed"
            | "auth.state_changed"
            | "auth.header_attached"
            | "auth.retry_after_unauthorized"
            | "auth.env_openai_api_key_present"
            | "auth.env_codex_api_key_present"
            | "auth.env_codex_api_key_enabled"
            | "auth.env_provider_key_present"
            | "auth.env_refresh_token_url_override_present"
            | "auth.connection_reused"
    ) {
        return sanitize_boolean(attribute);
    }
    sanitize_category_attribute(attribute)
}

fn sanitize_nonnegative_integer(attribute: &mut KeyValue) -> bool {
    let value = match &attribute.value {
        Value::I64(value) if *value >= 0 => *value,
        Value::String(value) => match value.as_str().parse::<u64>() {
            Ok(value) => value.min(i64::MAX as u64) as i64,
            Err(_) => return false,
        },
        _ => return false,
    };
    attribute.value = value.into();
    true
}

fn sanitize_boolean(attribute: &mut KeyValue) -> bool {
    let value = match &attribute.value {
        Value::Bool(value) => *value,
        Value::String(value) if value.as_str() == "true" => true,
        Value::String(value) if value.as_str() == "false" => false,
        _ => return false,
    };
    attribute.value = value.into();
    true
}

fn sanitize_category_attribute(attribute: &mut KeyValue) -> bool {
    let Value::String(value) = &attribute.value else {
        return false;
    };
    let value = value.as_str();
    let sanitized = match attribute.key.as_str() {
        "tool_name" => bounded_value(
            value,
            &[
                "apply_patch",
                "connector",
                "custom",
                "image",
                "mcp",
                "multi_agent",
                "other",
                "planning",
                "shell",
                "tool_search",
                "web_search",
                "agent",
            ],
        ),
        "tool_origin" => bounded_value(value, &["builtin", "mcp"]),
        "provider.category" => bounded_value(
            value,
            &["amazon_bedrock", "lmstudio", "ollama", "openai", "other"],
        ),
        "auth.error_code" => {
            bounded_value(value, &["refresh_token_expired", "token_expired", "other"])
        }
        "error.type" => bounded_value(
            value,
            &[
                "api",
                "authentication",
                "body",
                "build",
                "builder",
                "connect",
                "context_window_exceeded",
                "cyber_policy",
                "decode",
                "event_decode",
                "http",
                "invalid_request",
                "network",
                "other",
                "quota_exceeded",
                "rate_limit",
                "request",
                "response_completed",
                "response_failed",
                "retry_limit",
                "retryable",
                "server_overloaded",
                "stream",
                "timeout",
                "usage_not_included",
            ],
        ),
        "event.kind" => bounded_value(
            value,
            &[
                "parse_error",
                "response.completed",
                "response.created",
                "response.custom_tool_call_input.delta",
                "response.failed",
                "response.incomplete",
                "response.output_item.added",
                "response.output_item.done",
                "response.output_text.delta",
                "response.reasoning_summary_part.added",
                "response.reasoning_summary_text.delta",
                "response.reasoning_summary_text.done",
                "response.reasoning_text.delta",
                "responsesapi.websocket_timing",
                "unknown",
                "other",
            ],
        ),
        "service_tier" => bounded_service_tier_category(value),
        "model_reasoning_effort" | "reasoning_effort" => bounded_reasoning_effort_category(value),
        "reasoning_summary" => bounded_value(value, &["auto", "concise", "detailed", "none"]),
        "approval_policy" => {
            bounded_value(value, &["untrusted", "on-request", "granular", "never"])
        }
        "sandbox_policy" => bounded_value(
            value,
            &[
                "danger-full-access",
                "read-only",
                "external-sandbox",
                "workspace-write",
            ],
        ),
        "startup.phase" => bounded_value(
            value,
            &[
                "startup_prewarm_resolve",
                "startup_prewarm_total",
                "startup_prewarm_create_turn_context",
                "startup_prewarm_build_tools",
                "startup_prewarm_build_prompt",
                "startup_prewarm_websocket_warmup",
            ],
        ),
        "startup.status" => bounded_value(
            value,
            &[
                "cancelled",
                "consumed",
                "failed",
                "not_scheduled",
                "ready",
                "timed_out",
            ],
        ),
        "decision" => bounded_value(
            value,
            &[
                "abort",
                "approved",
                "approved_for_session",
                "approved_with_amendment",
                "approved_with_network_policy_allow",
                "denied",
                "denied_with_network_policy_deny",
                "timed_out",
            ],
        ),
        "source" => bounded_value(value, &["AutomatedReviewer", "Config", "User"]),
        "outcome" => bounded_value(value, &["denied", "escalated", "signal", "timed_out"]),
        "plugin_install.tool_type" => bounded_value(value, &["connector", "plugin"]),
        "plugin_install.response_action" => bounded_value(
            value,
            &["accept", "decline", "cancel", "unavailable", "unknown"],
        ),
        "auth.mode" => bounded_value(value, &["managed", "external", "none", "other"]),
        "auth.step" => bounded_value(
            value,
            &[
                "reload",
                "refresh_token",
                "external_refresh",
                "done",
                "none",
                "other",
            ],
        ),
        "auth.outcome" => bounded_value(
            value,
            &[
                "recovery_succeeded",
                "recovery_failed_permanent",
                "recovery_failed_transient",
                "recovery_not_run",
                "other",
            ],
        ),
        "auth.header_name" => bounded_value(value, &["authorization", "other"]),
        "auth.recovery_mode" => bounded_value(value, &["managed", "external", "other"]),
        "auth.recovery_phase" => bounded_value(
            value,
            &[
                "reload",
                "refresh_token",
                "external_refresh",
                "done",
                "other",
            ],
        ),
        _ => return false,
    };
    attribute.value = sanitized.into();
    true
}

fn bounded_value<'a>(value: &str, approved: &'a [&'a str]) -> &'a str {
    approved
        .iter()
        .copied()
        .find(|approved| *approved == value)
        .unwrap_or("other")
}

fn sanitize_resource(_resource: &Resource) -> Resource {
    Resource::builder_empty()
        .with_service_name(bounded_originator_tag_value("other"))
        .with_attributes([
            KeyValue::new(
                semconv::attribute::SERVICE_VERSION,
                env!("CARGO_PKG_VERSION"),
            ),
            KeyValue::new("env", bounded_environment_category("other")),
        ])
        .build()
}
