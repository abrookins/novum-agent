use crate::trace_sanitizer::PrivacyPreservingSpanExporter;
use opentelemetry::InstrumentationScope;
use opentelemetry::KeyValue;
use opentelemetry::SpanId;
use opentelemetry::TraceFlags;
use opentelemetry::TraceId;
use opentelemetry::trace::Event;
use opentelemetry::trace::Link;
use opentelemetry::trace::SpanContext;
use opentelemetry::trace::SpanKind;
use opentelemetry::trace::Status;
use opentelemetry::trace::TraceState;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::InMemorySpanExporter;
use opentelemetry_sdk::trace::SdkTracerProvider;
use opentelemetry_sdk::trace::SpanData;
use opentelemetry_sdk::trace::SpanEvents;
use opentelemetry_sdk::trace::SpanExporter as _;
use opentelemetry_sdk::trace::SpanLinks;
use pretty_assertions::assert_eq;
use std::borrow::Cow;
use std::time::SystemTime;
use tracing_subscriber::layer::SubscriberExt;

const CANARIES: &[&str] = &[
    "/canary/private/cwd",
    "canary-private-server",
    "canary-private-skill",
    "canary-private-mcp-tool",
    "canary-private-connector",
    "canary-private-identifier",
    "canary private prompt",
    "canary private arguments",
    "canary private output",
    "canary private error",
];

#[tokio::test]
async fn span_exporter_keeps_only_approved_trace_data() {
    let span_context = SpanContext::new(
        TraceId::from(1),
        SpanId::from(2),
        TraceFlags::SAMPLED,
        /*is_remote*/ false,
        TraceState::default(),
    );
    let mut events = SpanEvents::default();
    events.events.push(Event::new(
        "canary-event-name",
        SystemTime::now(),
        vec![
            KeyValue::new("event.name", "codex.tool_result"),
            KeyValue::new("tool_name", "mcp"),
            KeyValue::new("success", true),
            KeyValue::new("duration_ms", 42_i64),
            KeyValue::new("arguments_length", 24_i64),
            KeyValue::new("output_length", 21_i64),
            KeyValue::new("output_line_count", 1_i64),
            KeyValue::new("cwd", CANARIES[0]),
            KeyValue::new("mcp.server.name", CANARIES[1]),
            KeyValue::new("skill.name", CANARIES[2]),
            KeyValue::new("mcp.tool.name", CANARIES[3]),
            KeyValue::new("connector.id", CANARIES[4]),
            KeyValue::new("call_id", CANARIES[5]),
            KeyValue::new("prompt", CANARIES[6]),
            KeyValue::new("arguments", CANARIES[7]),
            KeyValue::new("output", CANARIES[8]),
            KeyValue::new("error.message", CANARIES[9]),
            KeyValue::new("reasoning_effort", "canary-private-effort"),
            KeyValue::new("service_tier", "canary-private-tier"),
            KeyValue::new("decision", "canary-private-decision"),
            KeyValue::new("success", "canary-private-success"),
            KeyValue::new("attempt", "canary-private-attempt"),
            KeyValue::new("event.name", "canary-private-duplicate-event-name"),
        ],
        /*dropped_attributes_count*/ 0,
    ));
    events.events.push(Event::new(
        "canary-unapproved-event",
        SystemTime::now(),
        vec![
            KeyValue::new("event.name", "codex.canary.private"),
            KeyValue::new("message", CANARIES[6]),
        ],
        /*dropped_attributes_count*/ 0,
    ));
    let mut links = SpanLinks::default();
    links.links.push(Link::new(
        SpanContext::new(
            TraceId::from(3),
            SpanId::from(4),
            TraceFlags::SAMPLED,
            /*is_remote*/ true,
            TraceState::default(),
        ),
        vec![KeyValue::new("link.private", CANARIES[5])],
        /*dropped_attributes_count*/ 0,
    ));
    let span = SpanData {
        span_context: span_context.clone(),
        parent_span_id: SpanId::from(5),
        parent_span_is_remote: true,
        span_kind: SpanKind::Client,
        name: Cow::Borrowed("canary-private-dynamic-span-name"),
        start_time: SystemTime::now(),
        end_time: SystemTime::now(),
        attributes: CANARIES
            .iter()
            .enumerate()
            .map(|(index, value)| KeyValue::new(format!("private.{index}"), *value))
            .collect(),
        dropped_attributes_count: 0,
        events,
        links,
        status: Status::error(CANARIES[9]),
        instrumentation_scope: InstrumentationScope::builder("canary-private-scope")
            .with_attributes([KeyValue::new("scope.private", CANARIES[5])])
            .build(),
    };

    let exporter = InMemorySpanExporter::default();
    let sanitizer = PrivacyPreservingSpanExporter::new(exporter.clone());
    sanitizer.export(vec![span]).await.expect("export span");

    let spans = exporter.get_finished_spans().expect("finished spans");
    assert_eq!(spans.len(), 1);
    let exported = &spans[0];
    assert_eq!(exported.span_context, span_context);
    assert_eq!(exported.name, "codex.operation");
    assert_eq!(exported.attributes, Vec::<KeyValue>::new());
    assert_eq!(exported.instrumentation_scope.name(), "codex");
    assert_eq!(exported.events.events.len(), 1);
    assert_eq!(exported.events.events[0].name, "codex.tool_result");
    assert_eq!(
        exported.events.events[0]
            .attributes
            .iter()
            .map(|attribute| attribute.key.as_str())
            .collect::<Vec<_>>(),
        vec![
            "event.name",
            "tool_name",
            "success",
            "duration_ms",
            "arguments_length",
            "output_length",
            "output_line_count",
            "reasoning_effort",
            "service_tier",
            "decision",
        ]
    );
    let attributes = &exported.events.events[0].attributes;
    assert_eq!(
        attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == "reasoning_effort")
            .map(|attribute| attribute.value.to_string()),
        Some("other".to_string())
    );
    assert_eq!(
        attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == "service_tier")
            .map(|attribute| attribute.value.to_string()),
        Some("other".to_string())
    );
    assert_eq!(
        attributes
            .iter()
            .find(|attribute| attribute.key.as_str() == "decision")
            .map(|attribute| attribute.value.to_string()),
        Some("other".to_string())
    );
    assert_eq!(
        attributes
            .iter()
            .filter(|attribute| attribute.key.as_str() == "event.name")
            .count(),
        1
    );
    assert!(exported.links.links[0].attributes.is_empty());
    assert_eq!(exported.status, Status::error(""));
    let exported_debug = format!("{exported:?}");
    for canary in CANARIES {
        assert!(
            !exported_debug.contains(canary),
            "exported span leaked {canary:?}: {exported_debug}"
        );
    }
}

#[tracing::instrument(skip_all, fields(argument_count = values.len()))]
fn instrumented_operation(values: &[&str]) {
    assert!(!values.is_empty());
}

#[test]
fn instrumented_functions_skip_argument_values_before_export() {
    let exporter = InMemorySpanExporter::default();
    let provider = SdkTracerProvider::builder()
        .with_simple_exporter(exporter.clone())
        .build();
    let subscriber = tracing_subscriber::registry()
        .with(tracing_opentelemetry::layer().with_tracer(provider.tracer("instrumentation-test")));

    tracing::subscriber::with_default(subscriber, || {
        instrumented_operation(&[CANARIES[6], CANARIES[7], CANARIES[8]]);
    });
    provider.force_flush().expect("flush spans");

    let spans = exporter.get_finished_spans().expect("finished spans");
    assert_eq!(spans.len(), 1);
    let argument_count = spans[0]
        .attributes
        .iter()
        .find(|attribute| attribute.key.as_str() == "argument_count")
        .expect("argument count attribute");
    assert_eq!(argument_count.value.to_string(), "3");
    let span_debug = format!("{:?}", spans[0]);
    for canary in [CANARIES[6], CANARIES[7], CANARIES[8]] {
        assert!(
            !span_debug.contains(canary),
            "instrumented span captured argument {canary:?}: {span_debug}"
        );
    }
}
