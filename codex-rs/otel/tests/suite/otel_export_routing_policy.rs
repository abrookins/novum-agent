use codex_otel::AuthEnvTelemetryMetadata;
use codex_otel::OtelProvider;
use codex_otel::SessionTelemetry;
use codex_otel::TelemetryAuthMode;
use opentelemetry::KeyValue;
use opentelemetry::logs::AnyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::logs::InMemoryLogExporter;
use opentelemetry_sdk::logs::SdkLogRecord;
use opentelemetry_sdk::logs::SdkLoggerProvider;
use opentelemetry_sdk::trace::InMemorySpanExporter;
use opentelemetry_sdk::trace::SdkTracerProvider;
use pretty_assertions::assert_eq;
use std::borrow::Cow;
use std::collections::BTreeMap;
use std::path::PathBuf;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::layer::SubscriberExt;

use codex_protocol::ThreadId;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::user_input::UserInput;

fn log_attributes(record: &SdkLogRecord) -> BTreeMap<String, String> {
    record
        .attributes_iter()
        .map(|(key, value)| (key.as_str().to_string(), any_value_to_string(value)))
        .collect()
}

fn span_event_attributes(event: &opentelemetry::trace::Event) -> BTreeMap<String, String> {
    event
        .attributes
        .iter()
        .map(|KeyValue { key, value, .. }| (key.as_str().to_string(), value.to_string()))
        .collect()
}

fn any_value_to_string(value: &AnyValue) -> String {
    match value {
        AnyValue::Int(value) => value.to_string(),
        AnyValue::Double(value) => value.to_string(),
        AnyValue::String(value) => value.as_str().to_string(),
        AnyValue::Boolean(value) => value.to_string(),
        AnyValue::Bytes(value) => String::from_utf8_lossy(value).into_owned(),
        AnyValue::ListAny(value) => format!("{value:?}"),
        AnyValue::Map(value) => format!("{value:?}"),
        _ => format!("{value:?}"),
    }
}

fn assert_attributes_omit_values(attributes: &BTreeMap<String, String>, values: &[&str]) {
    for value in values {
        assert!(
            attributes
                .values()
                .all(|attribute| !attribute.contains(value)),
            "telemetry attributes must omit {value:?}: {attributes:?}"
        );
    }
}

fn find_log_by_event_name<'a>(
    logs: &'a [opentelemetry_sdk::logs::in_memory_exporter::LogDataWithResource],
    event_name: &str,
) -> &'a opentelemetry_sdk::logs::in_memory_exporter::LogDataWithResource {
    logs.iter()
        .find(|log| {
            log_attributes(&log.record)
                .get("event.name")
                .is_some_and(|value| value == event_name)
        })
        .expect("log event should exist")
}

fn find_span_event_by_name_attr<'a>(
    events: &'a [opentelemetry::trace::Event],
    event_name: &str,
) -> &'a opentelemetry::trace::Event {
    events
        .iter()
        .find(|event| {
            span_event_attributes(event)
                .get("event.name")
                .is_some_and(|value| value == event_name)
        })
        .expect("span event should exist")
}

fn find_tool_log_by_name<'a>(
    logs: &'a [opentelemetry_sdk::logs::in_memory_exporter::LogDataWithResource],
    tool_name: &str,
) -> &'a opentelemetry_sdk::logs::in_memory_exporter::LogDataWithResource {
    logs.iter()
        .find(|log| {
            let attributes = log_attributes(&log.record);
            attributes.get("event.name").map(String::as_str) == Some("codex.tool_result")
                && attributes.get("tool_name").map(String::as_str) == Some(tool_name)
        })
        .expect("tool log event should exist")
}

fn find_tool_span_event_by_name<'a>(
    events: &'a [opentelemetry::trace::Event],
    tool_name: &str,
) -> &'a opentelemetry::trace::Event {
    events
        .iter()
        .find(|event| {
            let attributes = span_event_attributes(event);
            attributes.get("event.name").map(String::as_str) == Some("codex.tool_result")
                && attributes.get("tool_name").map(String::as_str) == Some(tool_name)
        })
        .expect("tool span event should exist")
}

fn auth_env_metadata() -> AuthEnvTelemetryMetadata {
    AuthEnvTelemetryMetadata {
        openai_api_key_env_present: true,
        codex_api_key_env_present: false,
        codex_api_key_env_enabled: true,
        provider_env_key_name: Some("CANARY_PRIVATE_PROVIDER_KEY_NAME".to_string()),
        provider_env_key_present: Some(true),
        refresh_token_url_override_present: true,
    }
}

#[test]
fn otel_export_routing_policy_routes_user_prompt_log_and_trace_events() {
    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let tracer = tracer_provider.tracer("sink-split-test");

    let subscriber = tracing_subscriber::registry()
        .with(
            opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                &logger_provider,
            )
            .with_filter(filter_fn(OtelProvider::log_export_filter)),
        )
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(filter_fn(OtelProvider::trace_export_filter)),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        let manager = SessionTelemetry::new(
            ThreadId::new(),
            "gpt-5.1",
            "gpt-5.1",
            Some("account-id".to_string()),
            Some("engineer@example.com".to_string()),
            Some(TelemetryAuthMode::ApiKey),
            "codex_exec".to_string(),
            /*log_user_prompts*/ true,
            "tty".to_string(),
            SessionSource::Cli,
        );
        let root_span = tracing::info_span!("root");
        let _root_guard = root_span.enter();
        manager.user_prompt(&[
            UserInput::Text {
                text: "super secret prompt".to_string(),
                text_elements: Vec::new(),
            },
            UserInput::Image {
                image_url: "https://example.com/image.png".to_string(),
                detail: None,
            },
            UserInput::LocalImage {
                path: PathBuf::from("/tmp/secret.png"),
                detail: None,
            },
        ]);
    });

    logger_provider.force_flush().expect("flush logs");
    tracer_provider.force_flush().expect("flush traces");

    let logs = log_exporter.get_emitted_logs().expect("log export");
    assert!(
        logs.iter()
            .all(|log| { log.record.target().map(Cow::as_ref) == Some("codex_otel.log_only") })
    );

    let prompt_log = find_log_by_event_name(&logs, "codex.user_prompt");
    let prompt_log_attrs = log_attributes(&prompt_log.record);
    assert_eq!(
        prompt_log_attrs.get("prompt_length").map(String::as_str),
        Some("19")
    );
    assert_eq!(
        prompt_log_attrs.get("text_input_count").map(String::as_str),
        Some("1")
    );
    assert!(!prompt_log_attrs.contains_key("prompt"));
    assert!(!prompt_log_attrs.contains_key("user.email"));
    assert!(!prompt_log_attrs.contains_key("user.account_id"));
    assert_attributes_omit_values(
        &prompt_log_attrs,
        &["super secret prompt", "engineer@example.com", "account-id"],
    );

    let spans = span_exporter.get_finished_spans().expect("span export");
    assert_eq!(spans.len(), 1);
    let span_events = &spans[0].events.events;
    assert_eq!(span_events.len(), 1);

    let prompt_trace_event = find_span_event_by_name_attr(span_events, "codex.user_prompt");
    let prompt_trace_attrs = span_event_attributes(prompt_trace_event);
    assert_eq!(
        prompt_trace_attrs.get("prompt_length").map(String::as_str),
        Some("19")
    );
    assert_eq!(
        prompt_trace_attrs
            .get("text_input_count")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        prompt_trace_attrs
            .get("image_input_count")
            .map(String::as_str),
        Some("1")
    );
    assert_eq!(
        prompt_trace_attrs
            .get("local_image_input_count")
            .map(String::as_str),
        Some("1")
    );
    assert!(!prompt_trace_attrs.contains_key("prompt"));
    assert!(!prompt_trace_attrs.contains_key("user.email"));
    assert!(!prompt_trace_attrs.contains_key("user.account_id"));
    assert_attributes_omit_values(
        &prompt_trace_attrs,
        &["super secret prompt", "engineer@example.com", "account-id"],
    );
}

#[test]
fn otel_export_routing_policy_routes_tool_result_log_and_trace_events() {
    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let tracer = tracer_provider.tracer("sink-split-test");

    let subscriber = tracing_subscriber::registry()
        .with(
            opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                &logger_provider,
            )
            .with_filter(filter_fn(OtelProvider::log_export_filter)),
        )
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(filter_fn(OtelProvider::trace_export_filter)),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        let manager = SessionTelemetry::new(
            ThreadId::new(),
            "gpt-5.1",
            "gpt-5.1",
            Some("account-id".to_string()),
            Some("engineer@example.com".to_string()),
            Some(TelemetryAuthMode::ApiKey),
            "codex_exec".to_string(),
            /*log_user_prompts*/ true,
            "tty".to_string(),
            SessionSource::Cli,
        );
        let root_span = tracing::info_span!("root");
        let _root_guard = root_span.enter();
        manager.tool_result_with_tags(
            "canary-private-mcp-tool",
            "call-1",
            "secret arguments",
            std::time::Duration::from_millis(42),
            /*success*/ true,
            "secret output\nsecond line",
            &[],
            &[
                ("mcp_server", "internal-mcp"),
                ("mcp_server_origin", "stdio"),
            ],
        );
        manager.log_tool_failed("canary-private-failed-tool", "secret failure output");
    });

    logger_provider.force_flush().expect("flush logs");
    tracer_provider.force_flush().expect("flush traces");

    let logs = log_exporter.get_emitted_logs().expect("log export");
    assert!(
        logs.iter()
            .all(|log| { log.record.target().map(Cow::as_ref) == Some("codex_otel.log_only") })
    );

    let tool_log = find_tool_log_by_name(&logs, "mcp");
    let tool_log_attrs = log_attributes(&tool_log.record);
    assert_eq!(
        tool_log_attrs.get("arguments_length").map(String::as_str),
        Some("16")
    );
    assert!(!tool_log_attrs.contains_key("arguments"));
    assert!(!tool_log_attrs.contains_key("call_id"));
    assert_eq!(
        tool_log_attrs.get("output_length").map(String::as_str),
        Some("25")
    );
    assert_eq!(
        tool_log_attrs.get("output_line_count").map(String::as_str),
        Some("2")
    );
    assert!(!tool_log_attrs.contains_key("output"));
    assert!(!tool_log_attrs.contains_key("mcp_server"));
    assert!(!tool_log_attrs.contains_key("mcp_server_origin"));

    let failed_tool_log = find_tool_log_by_name(&logs, "custom");
    let failed_tool_log_attrs = log_attributes(&failed_tool_log.record);
    assert_eq!(
        failed_tool_log_attrs
            .get("output_length")
            .map(String::as_str),
        Some("21")
    );
    assert_eq!(
        failed_tool_log_attrs
            .get("output_line_count")
            .map(String::as_str),
        Some("1")
    );
    assert!(!failed_tool_log_attrs.contains_key("output"));

    let spans = span_exporter.get_finished_spans().expect("span export");
    assert_eq!(spans.len(), 1);
    let span_events = &spans[0].events.events;
    assert_eq!(span_events.len(), 2);

    let tool_trace_event = find_tool_span_event_by_name(span_events, "mcp");
    let tool_trace_attrs = span_event_attributes(tool_trace_event);
    assert_eq!(
        tool_trace_attrs.get("arguments_length").map(String::as_str),
        Some("16")
    );
    assert_eq!(
        tool_trace_attrs.get("output_length").map(String::as_str),
        Some("25")
    );
    assert_eq!(
        tool_trace_attrs
            .get("output_line_count")
            .map(String::as_str),
        Some("2")
    );
    assert!(!tool_trace_attrs.contains_key("arguments"));
    assert!(!tool_trace_attrs.contains_key("call_id"));
    assert!(!tool_trace_attrs.contains_key("output"));
    assert!(!tool_trace_attrs.contains_key("mcp_server"));
    assert!(!tool_trace_attrs.contains_key("mcp_server_origin"));

    let failed_tool_trace_event = find_tool_span_event_by_name(span_events, "custom");
    let failed_tool_trace_attrs = span_event_attributes(failed_tool_trace_event);
    assert_eq!(
        failed_tool_trace_attrs
            .get("output_length")
            .map(String::as_str),
        Some("21")
    );
    assert_eq!(
        failed_tool_trace_attrs
            .get("output_line_count")
            .map(String::as_str),
        Some("1")
    );
    assert!(!failed_tool_trace_attrs.contains_key("output"));
    assert!(!failed_tool_trace_attrs.contains_key("error.message"));
    assert_attributes_omit_values(
        &tool_log_attrs,
        &["canary-private-mcp-tool", "internal-mcp", "stdio"],
    );
    assert_attributes_omit_values(
        &failed_tool_log_attrs,
        &["canary-private-failed-tool", "secret failure output"],
    );
}

#[test]
fn otel_export_routing_policy_routes_auth_recovery_log_and_trace_events() {
    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let tracer = tracer_provider.tracer("sink-split-test");

    let subscriber = tracing_subscriber::registry()
        .with(
            opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                &logger_provider,
            )
            .with_filter(filter_fn(OtelProvider::log_export_filter)),
        )
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(filter_fn(OtelProvider::trace_export_filter)),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        let manager = SessionTelemetry::new(
            ThreadId::new(),
            "gpt-5.1",
            "gpt-5.1",
            Some("account-id".to_string()),
            Some("engineer@example.com".to_string()),
            Some(TelemetryAuthMode::Chatgpt),
            "codex_exec".to_string(),
            /*log_user_prompts*/ true,
            "tty".to_string(),
            SessionSource::Cli,
        );
        let root_span = tracing::info_span!("root");
        let _root_guard = root_span.enter();
        manager.record_auth_recovery(
            "managed",
            "reload",
            "recovery_succeeded",
            Some("canary raw authorization failure"),
            Some("token_expired"),
            Some("canary-private-recovery-reason"),
            Some(true),
        );
    });

    logger_provider.force_flush().expect("flush logs");
    tracer_provider.force_flush().expect("flush traces");

    let logs = log_exporter.get_emitted_logs().expect("log export");
    let recovery_log = find_log_by_event_name(&logs, "codex.auth_recovery");
    let recovery_log_attrs = log_attributes(&recovery_log.record);
    assert_eq!(
        recovery_log_attrs.get("auth.mode").map(String::as_str),
        Some("managed")
    );
    assert_eq!(
        recovery_log_attrs.get("auth.step").map(String::as_str),
        Some("reload")
    );
    assert_eq!(
        recovery_log_attrs.get("auth.outcome").map(String::as_str),
        Some("recovery_succeeded")
    );
    assert_eq!(
        recovery_log_attrs
            .get("auth.error_code")
            .map(String::as_str),
        Some("token_expired")
    );
    assert_eq!(
        recovery_log_attrs
            .get("auth.state_changed")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        recovery_log_attrs.get("error.type").map(String::as_str),
        Some("authentication")
    );
    assert_eq!(
        recovery_log_attrs
            .get("auth.recovery_reason")
            .map(String::as_str),
        Some("other")
    );
    assert!(!recovery_log_attrs.contains_key("auth.request_id"));
    assert!(!recovery_log_attrs.contains_key("auth.cf_ray"));
    assert!(!recovery_log_attrs.contains_key("auth.error"));
    assert_attributes_omit_values(
        &recovery_log_attrs,
        &[
            "canary-auth-request-id",
            "canary-auth-ray-id",
            "canary raw authorization failure",
            "canary-private-recovery-reason",
        ],
    );

    let spans = span_exporter.get_finished_spans().expect("span export");
    assert_eq!(spans.len(), 1);
    let span_events = &spans[0].events.events;
    assert_eq!(span_events.len(), 1);

    let recovery_trace_event = find_span_event_by_name_attr(span_events, "codex.auth_recovery");
    let recovery_trace_attrs = span_event_attributes(recovery_trace_event);
    assert_eq!(
        recovery_trace_attrs.get("auth.mode").map(String::as_str),
        Some("managed")
    );
    assert_eq!(
        recovery_trace_attrs.get("auth.step").map(String::as_str),
        Some("reload")
    );
    assert_eq!(
        recovery_trace_attrs.get("auth.outcome").map(String::as_str),
        Some("recovery_succeeded")
    );
    assert_eq!(
        recovery_trace_attrs
            .get("auth.error_code")
            .map(String::as_str),
        Some("token_expired")
    );
    assert_eq!(
        recovery_trace_attrs
            .get("auth.state_changed")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        recovery_trace_attrs.get("error.type").map(String::as_str),
        Some("authentication")
    );
    assert_eq!(
        recovery_trace_attrs
            .get("auth.recovery_reason")
            .map(String::as_str),
        Some("other")
    );
    assert!(!recovery_trace_attrs.contains_key("auth.request_id"));
    assert!(!recovery_trace_attrs.contains_key("auth.cf_ray"));
    assert!(!recovery_trace_attrs.contains_key("auth.error"));
    assert_attributes_omit_values(
        &recovery_trace_attrs,
        &[
            "canary-auth-request-id",
            "canary-auth-ray-id",
            "canary raw authorization failure",
            "canary-private-recovery-reason",
        ],
    );
}

#[test]
fn otel_export_routing_policy_routes_api_request_auth_observability() {
    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let tracer = tracer_provider.tracer("sink-split-test");

    let subscriber = tracing_subscriber::registry()
        .with(
            opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                &logger_provider,
            )
            .with_filter(filter_fn(OtelProvider::log_export_filter)),
        )
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(filter_fn(OtelProvider::trace_export_filter)),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        let manager = SessionTelemetry::new(
            ThreadId::new(),
            "gpt-5.1",
            "gpt-5.1",
            Some("account-id".to_string()),
            Some("engineer@example.com".to_string()),
            Some(TelemetryAuthMode::Chatgpt),
            "codex_exec".to_string(),
            /*log_user_prompts*/ true,
            "tty".to_string(),
            SessionSource::Cli,
        )
        .with_auth_env(auth_env_metadata());
        let root_span = tracing::info_span!("root");
        let _root_guard = root_span.enter();
        manager.conversation_starts(
            "openai",
            /*reasoning_effort*/ None,
            ReasoningSummary::Auto,
            /*context_window*/ None,
            /*auto_compact_token_limit*/ None,
            AskForApproval::Never,
            SandboxPolicy::DangerFullAccess,
            Vec::new(),
        );
        manager.record_api_request(
            /*attempt*/ 1,
            Some(401),
            Some("api"),
            std::time::Duration::from_millis(42),
            /*auth_header_attached*/ true,
            Some("authorization"),
            /*retry_after_unauthorized*/ true,
            Some("managed"),
            Some("refresh_token"),
            "/responses",
            Some("canary raw api auth failure"),
            Some("token_expired"),
        );
    });

    logger_provider.force_flush().expect("flush logs");
    tracer_provider.force_flush().expect("flush traces");

    let logs = log_exporter.get_emitted_logs().expect("log export");
    let conversation_log = find_log_by_event_name(&logs, "codex.conversation_starts");
    let conversation_log_attrs = log_attributes(&conversation_log.record);
    assert_eq!(
        conversation_log_attrs
            .get("auth.env_openai_api_key_present")
            .map(String::as_str),
        Some("true")
    );
    assert!(!conversation_log_attrs.contains_key("auth.env_provider_key_name"));
    let request_log = find_log_by_event_name(&logs, "codex.api_request");
    let request_log_attrs = log_attributes(&request_log.record);
    assert_eq!(
        request_log_attrs
            .get("auth.header_attached")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_log_attrs
            .get("auth.header_name")
            .map(String::as_str),
        Some("authorization")
    );
    assert_eq!(
        request_log_attrs
            .get("auth.retry_after_unauthorized")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_log_attrs
            .get("auth.recovery_mode")
            .map(String::as_str),
        Some("managed")
    );
    assert_eq!(
        request_log_attrs
            .get("auth.recovery_phase")
            .map(String::as_str),
        Some("refresh_token")
    );
    assert_eq!(
        request_log_attrs.get("endpoint").map(String::as_str),
        Some("/responses")
    );
    assert_eq!(
        request_log_attrs
            .get("auth.env_codex_api_key_enabled")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_log_attrs
            .get("auth.env_refresh_token_url_override_present")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_log_attrs.get("error.type").map(String::as_str),
        Some("authentication")
    );
    for field in [
        "error.message",
        "auth.request_id",
        "auth.cf_ray",
        "auth.error",
        "auth.agent_id",
        "auth.task_id",
    ] {
        assert!(!request_log_attrs.contains_key(field));
    }
    assert_attributes_omit_values(
        &request_log_attrs,
        &[
            "canary-api-request-id",
            "canary-api-ray-id",
            "canary raw api auth failure",
            "agent-runtime-otel",
            "task-run-otel",
        ],
    );

    let spans = span_exporter.get_finished_spans().expect("span export");
    let conversation_trace_event =
        find_span_event_by_name_attr(&spans[0].events.events, "codex.conversation_starts");
    let conversation_trace_attrs = span_event_attributes(conversation_trace_event);
    assert_eq!(
        conversation_trace_attrs
            .get("auth.env_provider_key_present")
            .map(String::as_str),
        Some("true")
    );
    let request_trace_event =
        find_span_event_by_name_attr(&spans[0].events.events, "codex.api_request");
    let request_trace_attrs = span_event_attributes(request_trace_event);
    assert_eq!(
        request_trace_attrs
            .get("auth.header_attached")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_trace_attrs
            .get("auth.header_name")
            .map(String::as_str),
        Some("authorization")
    );
    assert_eq!(
        request_trace_attrs
            .get("auth.retry_after_unauthorized")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_trace_attrs.get("endpoint").map(String::as_str),
        Some("/responses")
    );
    assert_eq!(
        request_trace_attrs
            .get("auth.env_openai_api_key_present")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_trace_attrs.get("error.type").map(String::as_str),
        Some("authentication")
    );
    for field in [
        "error.message",
        "auth.request_id",
        "auth.cf_ray",
        "auth.error",
        "auth.agent_id",
        "auth.task_id",
    ] {
        assert!(!request_trace_attrs.contains_key(field));
    }
    assert_attributes_omit_values(
        &request_trace_attrs,
        &[
            "canary-api-request-id",
            "canary-api-ray-id",
            "canary raw api auth failure",
            "agent-runtime-otel",
            "task-run-otel",
        ],
    );
}

#[test]
fn otel_export_routing_policy_routes_websocket_connect_auth_observability() {
    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let tracer = tracer_provider.tracer("sink-split-test");

    let subscriber = tracing_subscriber::registry()
        .with(
            opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                &logger_provider,
            )
            .with_filter(filter_fn(OtelProvider::log_export_filter)),
        )
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(filter_fn(OtelProvider::trace_export_filter)),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        let manager = SessionTelemetry::new(
            ThreadId::new(),
            "gpt-5.1",
            "gpt-5.1",
            Some("account-id".to_string()),
            Some("engineer@example.com".to_string()),
            Some(TelemetryAuthMode::Chatgpt),
            "codex_exec".to_string(),
            /*log_user_prompts*/ true,
            "tty".to_string(),
            SessionSource::Cli,
        )
        .with_auth_env(auth_env_metadata());
        let root_span = tracing::info_span!("root");
        let _root_guard = root_span.enter();
        manager.record_websocket_connect(
            std::time::Duration::from_millis(17),
            Some(401),
            Some("http"),
            /*auth_header_attached*/ true,
            Some("authorization"),
            /*retry_after_unauthorized*/ true,
            Some("managed"),
            Some("reload"),
            "/responses",
            /*connection_reused*/ false,
            Some("canary raw websocket auth failure"),
            Some("token_expired"),
        );
    });

    logger_provider.force_flush().expect("flush logs");
    tracer_provider.force_flush().expect("flush traces");

    let logs = log_exporter.get_emitted_logs().expect("log export");
    let connect_log = find_log_by_event_name(&logs, "codex.websocket_connect");
    let connect_log_attrs = log_attributes(&connect_log.record);
    assert_eq!(
        connect_log_attrs
            .get("auth.header_attached")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        connect_log_attrs
            .get("auth.header_name")
            .map(String::as_str),
        Some("authorization")
    );
    assert_eq!(
        connect_log_attrs.get("endpoint").map(String::as_str),
        Some("/responses")
    );
    assert_eq!(
        connect_log_attrs
            .get("auth.connection_reused")
            .map(String::as_str),
        Some("false")
    );
    assert!(!connect_log_attrs.contains_key("auth.env_provider_key_name"));
    assert_eq!(
        connect_log_attrs.get("error.type").map(String::as_str),
        Some("authentication")
    );
    for field in [
        "error.message",
        "auth.request_id",
        "auth.cf_ray",
        "auth.error",
        "auth.agent_id",
        "auth.task_id",
    ] {
        assert!(!connect_log_attrs.contains_key(field));
    }
    assert_attributes_omit_values(
        &connect_log_attrs,
        &[
            "canary-websocket-request-id",
            "canary-websocket-ray-id",
            "canary raw websocket auth failure",
            "agent-runtime-ws",
            "task-run-ws",
        ],
    );

    let spans = span_exporter.get_finished_spans().expect("span export");
    let connect_trace_event =
        find_span_event_by_name_attr(&spans[0].events.events, "codex.websocket_connect");
    let connect_trace_attrs = span_event_attributes(connect_trace_event);
    assert_eq!(
        connect_trace_attrs
            .get("auth.recovery_phase")
            .map(String::as_str),
        Some("reload")
    );
    assert_eq!(
        connect_trace_attrs
            .get("auth.env_refresh_token_url_override_present")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        connect_trace_attrs.get("error.type").map(String::as_str),
        Some("authentication")
    );
    for field in [
        "error.message",
        "auth.request_id",
        "auth.cf_ray",
        "auth.error",
        "auth.agent_id",
        "auth.task_id",
    ] {
        assert!(!connect_trace_attrs.contains_key(field));
    }
    assert_attributes_omit_values(
        &connect_trace_attrs,
        &[
            "canary-websocket-request-id",
            "canary-websocket-ray-id",
            "canary raw websocket auth failure",
            "agent-runtime-ws",
            "task-run-ws",
        ],
    );
}

#[test]
fn otel_export_routing_policy_routes_websocket_request_transport_observability() {
    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let tracer = tracer_provider.tracer("sink-split-test");

    let subscriber = tracing_subscriber::registry()
        .with(
            opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                &logger_provider,
            )
            .with_filter(filter_fn(OtelProvider::log_export_filter)),
        )
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(filter_fn(OtelProvider::trace_export_filter)),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        let manager = SessionTelemetry::new(
            ThreadId::new(),
            "gpt-5.1",
            "gpt-5.1",
            Some("account-id".to_string()),
            Some("engineer@example.com".to_string()),
            Some(TelemetryAuthMode::Chatgpt),
            "codex_exec".to_string(),
            /*log_user_prompts*/ true,
            "tty".to_string(),
            SessionSource::Cli,
        )
        .with_auth_env(auth_env_metadata());
        let root_span = tracing::info_span!("root");
        let _root_guard = root_span.enter();
        manager.record_websocket_request(
            std::time::Duration::from_millis(23),
            Some("stream"),
            /*connection_reused*/ true,
        );
    });

    logger_provider.force_flush().expect("flush logs");
    tracer_provider.force_flush().expect("flush traces");

    let logs = log_exporter.get_emitted_logs().expect("log export");
    let request_log = find_log_by_event_name(&logs, "codex.websocket_request");
    let request_log_attrs = log_attributes(&request_log.record);
    assert_eq!(
        request_log_attrs
            .get("auth.connection_reused")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_log_attrs
            .get("auth.env_openai_api_key_present")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_log_attrs.get("error.type").map(String::as_str),
        Some("stream")
    );
    for field in ["error.message", "auth.agent_id", "auth.task_id"] {
        assert!(!request_log_attrs.contains_key(field));
    }
    assert_attributes_omit_values(
        &request_log_attrs,
        &["agent-runtime-ws-request", "task-run-ws-request"],
    );

    let spans = span_exporter.get_finished_spans().expect("span export");
    let request_trace_event =
        find_span_event_by_name_attr(&spans[0].events.events, "codex.websocket_request");
    let request_trace_attrs = span_event_attributes(request_trace_event);
    assert_eq!(
        request_trace_attrs
            .get("auth.connection_reused")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_trace_attrs
            .get("auth.env_provider_key_present")
            .map(String::as_str),
        Some("true")
    );
    assert_eq!(
        request_trace_attrs.get("error.type").map(String::as_str),
        Some("stream")
    );
    for field in ["error.message", "auth.agent_id", "auth.task_id"] {
        assert!(!request_trace_attrs.contains_key(field));
    }
    assert_attributes_omit_values(
        &request_trace_attrs,
        &["agent-runtime-ws-request", "task-run-ws-request"],
    );
}

#[test]
fn otel_export_routing_policy_redacts_sse_failure_details() {
    let log_exporter = InMemoryLogExporter::default();
    let logger_provider = SdkLoggerProvider::builder()
        .with_simple_exporter(log_exporter.clone())
        .build();
    let span_exporter = InMemorySpanExporter::default();
    let tracer_provider = SdkTracerProvider::builder()
        .with_simple_exporter(span_exporter.clone())
        .build();
    let tracer = tracer_provider.tracer("sink-split-test");

    let subscriber = tracing_subscriber::registry()
        .with(
            opentelemetry_appender_tracing::layer::OpenTelemetryTracingBridge::new(
                &logger_provider,
            )
            .with_filter(filter_fn(OtelProvider::log_export_filter)),
        )
        .with(
            tracing_opentelemetry::layer()
                .with_tracer(tracer)
                .with_filter(filter_fn(OtelProvider::trace_export_filter)),
        );

    tracing::subscriber::with_default(subscriber, || {
        tracing::callsite::rebuild_interest_cache();
        let manager = SessionTelemetry::new(
            ThreadId::new(),
            "gpt-5.1",
            "gpt-5.1",
            Some("account-id".to_string()),
            Some("engineer@example.com".to_string()),
            Some(TelemetryAuthMode::Chatgpt),
            "codex_exec".to_string(),
            /*log_user_prompts*/ true,
            "tty".to_string(),
            SessionSource::Cli,
        );
        let root_span = tracing::info_span!("root");
        let _root_guard = root_span.enter();
        let kind = "response.failed".to_string();
        let error = serde_json::json!({
            "response": {
                "error": {
                    "message": "canary raw response.failed server body",
                    "code": "server_error"
                }
            }
        });
        manager.sse_event_failed(Some(&kind), std::time::Duration::from_millis(31), &error);
    });

    logger_provider.force_flush().expect("flush logs");
    tracer_provider.force_flush().expect("flush traces");

    let logs = log_exporter.get_emitted_logs().expect("log export");
    let event_log = find_log_by_event_name(&logs, "codex.sse_event");
    let log_attrs = log_attributes(&event_log.record);
    assert_eq!(
        log_attrs.get("event.kind").map(String::as_str),
        Some("response.failed")
    );
    assert_eq!(log_attrs.get("duration_ms").map(String::as_str), Some("31"));
    assert_eq!(
        log_attrs.get("error.type").map(String::as_str),
        Some("response_failed")
    );
    assert_eq!(log_attrs.get("success").map(String::as_str), Some("false"));
    assert!(!log_attrs.contains_key("error.message"));
    assert_attributes_omit_values(&log_attrs, &["canary raw response.failed server body"]);

    let spans = span_exporter.get_finished_spans().expect("span export");
    let event = find_span_event_by_name_attr(&spans[0].events.events, "codex.sse_event");
    let trace_attrs = span_event_attributes(event);
    assert_eq!(
        trace_attrs.get("event.kind").map(String::as_str),
        Some("response.failed")
    );
    assert_eq!(
        trace_attrs.get("error.type").map(String::as_str),
        Some("response_failed")
    );
    assert!(!trace_attrs.contains_key("error.message"));
    assert_attributes_omit_values(&trace_attrs, &["canary raw response.failed server body"]);
}
