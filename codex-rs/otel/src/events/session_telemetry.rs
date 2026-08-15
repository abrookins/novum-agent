use crate::TelemetryAuthMode;
use crate::ToolDecisionSource;
use crate::bounded_model_category;
use crate::bounded_originator_tag_value;
use crate::bounded_reasoning_effort_category;
use crate::bounded_service_tier_category;
use crate::events::shared::log_and_trace_event;
use crate::events::shared::log_event;
use crate::events::shared::trace_event;
use crate::metrics::API_CALL_COUNT_METRIC;
use crate::metrics::API_CALL_DURATION_METRIC;
use crate::metrics::MetricsClient;
use crate::metrics::MetricsConfig;
use crate::metrics::MetricsError;
use crate::metrics::PLUGIN_INSTALL_ELICITATION_SENT_METRIC;
use crate::metrics::PLUGIN_INSTALL_SUGGESTION_METRIC;
use crate::metrics::RESPONSES_API_ENGINE_IAPI_TBT_DURATION_METRIC;
use crate::metrics::RESPONSES_API_ENGINE_IAPI_TTFT_DURATION_METRIC;
use crate::metrics::RESPONSES_API_ENGINE_SERVICE_TBT_DURATION_METRIC;
use crate::metrics::RESPONSES_API_ENGINE_SERVICE_TTFT_DURATION_METRIC;
use crate::metrics::RESPONSES_API_INFERENCE_TIME_DURATION_METRIC;
use crate::metrics::RESPONSES_API_OVERHEAD_DURATION_METRIC;
use crate::metrics::Result as MetricsResult;
use crate::metrics::SSE_EVENT_COUNT_METRIC;
use crate::metrics::SSE_EVENT_DURATION_METRIC;
use crate::metrics::STARTUP_PHASE_DURATION_METRIC;
use crate::metrics::SessionMetricTagValues;
use crate::metrics::TOOL_CALL_COUNT_METRIC;
use crate::metrics::TOOL_CALL_DURATION_METRIC;
use crate::metrics::TURN_TTFT_DURATION_METRIC;
use crate::metrics::WEBSOCKET_EVENT_COUNT_METRIC;
use crate::metrics::WEBSOCKET_EVENT_DURATION_METRIC;
use crate::metrics::WEBSOCKET_REQUEST_COUNT_METRIC;
use crate::metrics::WEBSOCKET_REQUEST_DURATION_METRIC;
use crate::metrics::runtime_metrics::RuntimeMetricsSummary;
use crate::metrics::timer::Timer;
use crate::provider::OtelProvider;
use codex_api::ApiError;
use codex_api::ResponseEvent;
use codex_protocol::ThreadId;
use codex_protocol::config_types::ReasoningSummary;
use codex_protocol::models::ResponseItem;
use codex_protocol::openai_models::ReasoningEffort;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::protocol::SessionSource;
use codex_protocol::protocol::TokenUsage;
use codex_protocol::user_input::UserInput;
use eventsource_stream::Event as StreamEvent;
use eventsource_stream::EventStreamError as StreamError;
use opentelemetry_sdk::metrics::data::ResourceMetrics;
use reqwest::Error;
use reqwest::Response;
use std::borrow::Cow;
use std::future::Future;
use std::time::Duration;
use std::time::Instant;
use tokio::time::error::Elapsed;
use tracing::Span;

const RESPONSES_WEBSOCKET_TIMING_KIND: &str = "responsesapi.websocket_timing";
const RESPONSES_WEBSOCKET_TIMING_METRICS_FIELD: &str = "timing_metrics";
const RESPONSES_API_OVERHEAD_FIELD: &str = "responses_duration_excl_engine_and_client_tool_time_ms";
const RESPONSES_API_INFERENCE_FIELD: &str = "engine_service_total_ms";
const RESPONSES_API_ENGINE_IAPI_TTFT_FIELD: &str = "engine_iapi_ttft_total_ms";
const RESPONSES_API_ENGINE_SERVICE_TTFT_FIELD: &str = "engine_service_ttft_total_ms";
const RESPONSES_API_ENGINE_IAPI_TBT_FIELD: &str = "engine_iapi_tbt_across_engine_calls_ms";
const RESPONSES_API_ENGINE_SERVICE_TBT_FIELD: &str = "engine_service_tbt_across_engine_calls_ms";

fn reqwest_error_type(error: &reqwest::Error) -> &'static str {
    if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "network"
    } else if error.is_builder() {
        "build"
    } else if error.is_request() {
        "request"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else {
        "other"
    }
}

fn bucket_auth_error_code(code: Option<&str>) -> Option<&str> {
    match code {
        Some("token_expired") => Some("token_expired"),
        Some("refresh_token_expired") => Some("refresh_token_expired"),
        Some(_) => Some("other"),
        None => None,
    }
}

fn bounded_error_type(error_type: Option<&str>) -> Option<&'static str> {
    error_type.map(|error_type| match error_type {
        "api" => "api",
        "authentication" => "authentication",
        "body" => "body",
        "build" => "build",
        "builder" => "builder",
        "context_window_exceeded" => "context_window_exceeded",
        "cyber_policy" => "cyber_policy",
        "decode" => "decode",
        "event_decode" => "event_decode",
        "http" => "http",
        "invalid_request" => "invalid_request",
        "network" => "network",
        "quota_exceeded" => "quota_exceeded",
        "rate_limit" => "rate_limit",
        "request" => "request",
        "response_completed" => "response_completed",
        "response_failed" => "response_failed",
        "retry_limit" => "retry_limit",
        "retryable" => "retryable",
        "server_overloaded" => "server_overloaded",
        "stream" => "stream",
        "timeout" => "timeout",
        "usage_not_included" => "usage_not_included",
        _ => "other",
    })
}

fn bounded_endpoint(endpoint: &str) -> &'static str {
    match endpoint {
        "/models" => "/models",
        "/responses" => "/responses",
        "unknown" => "unknown",
        _ => "other",
    }
}

fn bounded_auth_header_name(header_name: Option<&str>) -> Option<&'static str> {
    header_name.map(|header_name| match header_name {
        "authorization" => "authorization",
        _ => "other",
    })
}

fn bounded_auth_recovery_mode(mode: Option<&str>) -> Option<&'static str> {
    mode.map(|mode| match mode {
        "external" => "external",
        "managed" => "managed",
        "none" => "none",
        _ => "other",
    })
}

fn bounded_auth_recovery_phase(phase: Option<&str>) -> Option<&'static str> {
    phase.map(|phase| match phase {
        "done" => "done",
        "external_refresh" => "external_refresh",
        "none" => "none",
        "refresh_token" => "refresh_token",
        "reload" => "reload",
        _ => "other",
    })
}

fn bounded_auth_recovery_outcome(outcome: &str) -> &'static str {
    match outcome {
        "recovery_failed_permanent" => "recovery_failed_permanent",
        "recovery_failed_transient" => "recovery_failed_transient",
        "recovery_not_run" => "recovery_not_run",
        "recovery_succeeded" => "recovery_succeeded",
        _ => "other",
    }
}

fn bounded_auth_recovery_reason(reason: Option<&str>) -> Option<&'static str> {
    reason.map(|reason| match reason {
        "auth_manager_missing" => "auth_manager_missing",
        "no_external_auth" => "no_external_auth",
        "not_chatgpt_auth" => "not_chatgpt_auth",
        "not_refreshable_auth" => "not_refreshable_auth",
        "ready" => "ready",
        "recovery_exhausted" => "recovery_exhausted",
        _ => "other",
    })
}

fn bounded_sandbox_outcome(outcome: &str) -> &'static str {
    match outcome {
        "denied" => "denied",
        "escalated" => "escalated",
        "signal" => "signal",
        "timed_out" => "timed_out",
        _ => "other",
    }
}

fn sse_error_type(kind: Option<&str>) -> &'static str {
    match kind {
        Some("response.failed") => "response_failed",
        Some("response.completed") => "response_completed",
        Some(_) => "event_decode",
        None => "stream",
    }
}

fn bounded_response_event_kind(kind: Option<&str>) -> &str {
    match kind {
        Some(
            kind @ ("response.created"
            | "response.failed"
            | "response.incomplete"
            | "response.completed"
            | "response.output_item.added"
            | "response.output_item.done"
            | "response.output_text.delta"
            | "response.custom_tool_call_input.delta"
            | "response.reasoning_summary_text.delta"
            | "response.reasoning_summary_text.done"
            | "response.reasoning_text.delta"
            | "response.reasoning_summary_part.added"
            | RESPONSES_WEBSOCKET_TIMING_KIND),
        ) => kind,
        Some("parse_error") => "parse_error",
        Some(_) => "other",
        None => "unknown",
    }
}

fn bounded_terminal_type(terminal_type: &str) -> &'static str {
    let family = terminal_type
        .split_once('/')
        .map_or(terminal_type, |(family, _)| family)
        .to_ascii_lowercase();
    match family.as_str() {
        "apple_terminal" => "apple_terminal",
        "ghostty" => "ghostty",
        "iterm.app" => "iterm2",
        "warpterminal" => "warp",
        "vscode" => "vscode",
        "wezterm" => "wezterm",
        "kitty" => "kitty",
        "alacritty" => "alacritty",
        "konsole" => "konsole",
        "gnome-terminal" => "gnome_terminal",
        "vte" => "vte",
        "windowsterminal" => "windows_terminal",
        "dumb" => "dumb",
        "unknown" => "unknown",
        _ => "other",
    }
}

fn bounded_provider_category(provider: &str) -> &'static str {
    match provider.to_ascii_lowercase().as_str() {
        "openai" => "openai",
        "amazon bedrock" | "amazon-bedrock" => "amazon_bedrock",
        "ollama" => "ollama",
        "lmstudio" => "lmstudio",
        _ => "other",
    }
}

fn bounded_plugin_tool_type(tool_type: &str) -> &'static str {
    match tool_type {
        "connector" => "connector",
        "plugin" => "plugin",
        _ => "other",
    }
}

fn bounded_plugin_response_action(response_action: &str) -> &'static str {
    match response_action {
        "accept" => "accept",
        "decline" => "decline",
        "cancel" => "cancel",
        "unavailable" => "unavailable",
        _ => "unknown",
    }
}

fn bounded_tool_category(tool_name: &str, is_mcp: bool) -> &'static str {
    if is_mcp || tool_name.starts_with("mcp__") {
        return "mcp";
    }
    match tool_name {
        "shell_command" | "exec_command" | "write_stdin" | "unified_exec" => "shell",
        "apply_patch" => "apply_patch",
        "update_plan" | "request_user_input" => "planning",
        "view_image" | "image_gen" => "image",
        "web_search" => "web_search",
        "spawn_agent" | "send_message" | "followup_task" | "wait" | "close_agent" => "agent",
        _ => "custom",
    }
}

fn allowed_tool_metric_tag(key: &str) -> bool {
    matches!(key, "sandbox" | "sandbox_policy" | "command_category")
}

fn trace_field_value<'a>(fields: &'a [(&str, &str)], key: &str) -> Option<&'a str> {
    fields
        .iter()
        .find_map(|(field_key, value)| (*field_key == key).then_some(*value))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AuthEnvTelemetryMetadata {
    pub openai_api_key_env_present: bool,
    pub codex_api_key_env_present: bool,
    pub codex_api_key_env_enabled: bool,
    pub provider_env_key_name: Option<String>,
    pub provider_env_key_present: Option<bool>,
    pub refresh_token_url_override_present: bool,
}

#[derive(Debug, Clone)]
pub struct SessionTelemetryMetadata {
    pub(crate) _conversation_id: ThreadId,
    pub(crate) auth_mode: Option<String>,
    pub(crate) auth_env: AuthEnvTelemetryMetadata,
    pub(crate) _account_id: Option<String>,
    pub(crate) _account_email: Option<String>,
    pub(crate) originator: String,
    pub(crate) service_name: Option<String>,
    pub(crate) session_source: String,
    pub(crate) model_category: &'static str,
    pub(crate) service_tier: Option<String>,
    pub(crate) model_reasoning_effort: Option<String>,
    pub(crate) _log_user_prompts: bool,
    pub(crate) app_version: &'static str,
    pub(crate) terminal_type: String,
}

#[derive(Debug, Clone)]
pub struct SessionTelemetry {
    pub(crate) metadata: SessionTelemetryMetadata,
    pub(crate) metrics: Option<MetricsClient>,
    pub(crate) metrics_use_metadata_tags: bool,
}

impl SessionTelemetry {
    pub fn with_auth_env(mut self, auth_env: AuthEnvTelemetryMetadata) -> Self {
        self.metadata.auth_env = auth_env;
        self
    }

    pub fn with_model(mut self, model: &str, _slug: &str) -> Self {
        self.metadata.model_category = bounded_model_category(model);
        self
    }

    pub fn with_inference_request(
        mut self,
        service_tier: Option<&str>,
        model_reasoning_effort: Option<&ReasoningEffort>,
    ) -> Self {
        self.metadata.service_tier = service_tier
            .map(bounded_service_tier_category)
            .map(str::to_owned);
        self.metadata.model_reasoning_effort = model_reasoning_effort
            .map(ReasoningEffort::as_str)
            .map(bounded_reasoning_effort_category)
            .map(str::to_owned);
        self
    }

    pub fn with_metrics_service_name(mut self, service_name: &str) -> Self {
        self.metadata.service_name = Some(bounded_originator_tag_value(service_name).to_string());
        self
    }

    pub fn with_metrics(mut self, metrics: MetricsClient) -> Self {
        self.metrics = Some(metrics);
        self.metrics_use_metadata_tags = true;
        self
    }

    pub fn with_metrics_without_metadata_tags(mut self, metrics: MetricsClient) -> Self {
        self.metrics = Some(metrics);
        self.metrics_use_metadata_tags = false;
        self
    }

    pub fn with_metrics_config(self, config: MetricsConfig) -> MetricsResult<Self> {
        let metrics = MetricsClient::new(config)?;
        Ok(self.with_metrics(metrics))
    }

    pub fn with_provider_metrics(self, provider: &OtelProvider) -> Self {
        match provider.metrics() {
            Some(metrics) => self.with_metrics(metrics.clone()),
            None => self,
        }
    }

    pub fn counter(&self, name: &str, inc: i64, tags: &[(&str, &str)]) {
        let res: MetricsResult<()> = (|| {
            let Some(metrics) = &self.metrics else {
                return Ok(());
            };

            let tags = self.tags_with_metadata(tags)?;
            metrics.counter(name, inc, &tags)
        })();

        if let Err(e) = res {
            tracing::warn!("metrics counter [{name}] failed: {e}");
        }
    }

    pub fn histogram(&self, name: &str, value: i64, tags: &[(&str, &str)]) {
        let res: MetricsResult<()> = (|| {
            let Some(metrics) = &self.metrics else {
                return Ok(());
            };

            let tags = self.tags_with_metadata(tags)?;
            metrics.histogram(name, value, &tags)
        })();

        if let Err(e) = res {
            tracing::warn!("metrics histogram [{name}] failed: {e}");
        }
    }

    pub fn record_duration(&self, name: &str, duration: Duration, tags: &[(&str, &str)]) {
        let res: MetricsResult<()> = (|| {
            let Some(metrics) = &self.metrics else {
                return Ok(());
            };

            let tags = self.tags_with_metadata(tags)?;
            metrics.record_duration(name, duration, &tags)
        })();

        if let Err(e) = res {
            tracing::warn!("metrics duration [{name}] failed: {e}");
        }
    }

    fn record_duration_ms_f64(&self, name: &str, duration_ms: f64, tags: &[(&str, &str)]) {
        let res: MetricsResult<()> = (|| {
            let Some(metrics) = &self.metrics else {
                return Ok(());
            };

            let tags = self.tags_with_metadata(tags)?;
            metrics.record_duration_ms_f64(name, duration_ms, &tags)
        })();

        if let Err(e) = res {
            tracing::warn!("metrics duration [{name}] failed: {e}");
        }
    }

    /// Records a coarse startup phase for production latency breakdowns.
    pub fn record_startup_phase(
        &self,
        phase: &'static str,
        duration: Duration,
        status: Option<&'static str>,
    ) {
        let tags = match status {
            Some(status) => vec![("phase", phase), ("status", status)],
            None => vec![("phase", phase)],
        };
        self.record_duration(STARTUP_PHASE_DURATION_METRIC, duration, &tags);
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.startup_phase",
                startup.phase = phase,
                startup.status = status,
                duration_ms = %duration.as_millis(),
            },
            log: {},
            trace: {},
        );
    }

    /// Records time to first token as both a metric and a production telemetry event.
    pub fn record_turn_ttft(&self, duration: Duration) {
        self.record_duration(TURN_TTFT_DURATION_METRIC, duration, &[]);
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.turn_ttft",
                duration_ms = %duration.as_millis(),
            },
            log: {},
            trace: {},
        );
    }

    /// Records the moment a plugin or connector install elicitation is dispatched.
    pub fn record_plugin_install_elicitation_sent(
        &self,
        tool_type: &str,
        _tool_id: &str,
        _tool_name: &str,
    ) {
        let tool_type = bounded_plugin_tool_type(tool_type);
        self.counter(
            PLUGIN_INSTALL_ELICITATION_SENT_METRIC,
            /*inc*/ 1,
            &[("tool_type", tool_type)],
        );
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.plugin_install_elicitation_sent",
                plugin_install.tool_type = tool_type,
            },
            log: {},
            trace: {},
        );
    }

    /// Records the outcome of a surfaced plugin or connector install suggestion.
    pub fn record_plugin_install_suggestion(
        &self,
        tool_type: &str,
        _tool_id: &str,
        _tool_name: &str,
        response_action: &str,
        user_confirmed: bool,
        completed: bool,
    ) {
        let tool_type = bounded_plugin_tool_type(tool_type);
        let response_action = bounded_plugin_response_action(response_action);
        let completed_tag = if completed { "true" } else { "false" };
        self.counter(
            PLUGIN_INSTALL_SUGGESTION_METRIC,
            /*inc*/ 1,
            &[
                ("tool_type", tool_type),
                ("response_action", response_action),
                ("completed", completed_tag),
            ],
        );
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.plugin_install_suggestion",
                plugin_install.tool_type = tool_type,
                plugin_install.response_action = response_action,
                plugin_install.user_confirmed = user_confirmed,
                plugin_install.completed = completed,
            },
            log: {},
            trace: {},
        );
    }

    pub fn start_timer(&self, name: &str, tags: &[(&str, &str)]) -> Result<Timer, MetricsError> {
        let Some(metrics) = &self.metrics else {
            return Err(MetricsError::ExporterDisabled);
        };
        let tags = self.tags_with_metadata(tags)?;
        metrics.start_timer(name, &tags)
    }

    pub fn shutdown_metrics(&self) -> MetricsResult<()> {
        let Some(metrics) = &self.metrics else {
            return Ok(());
        };
        metrics.shutdown()
    }

    pub fn snapshot_metrics(&self) -> MetricsResult<ResourceMetrics> {
        let Some(metrics) = &self.metrics else {
            return Err(MetricsError::ExporterDisabled);
        };
        metrics.snapshot()
    }

    /// Collect and discard a runtime metrics snapshot to reset delta accumulators.
    pub fn reset_runtime_metrics(&self) {
        if self.metrics.is_none() {
            return;
        }
        if let Err(err) = self.snapshot_metrics() {
            tracing::debug!("runtime metrics reset skipped: {err}");
        }
    }

    /// Collect a runtime metrics summary if debug snapshots are available.
    pub fn runtime_metrics_summary(&self) -> Option<RuntimeMetricsSummary> {
        let snapshot = match self.snapshot_metrics() {
            Ok(snapshot) => snapshot,
            Err(_) => {
                return None;
            }
        };
        let summary = RuntimeMetricsSummary::from_snapshot(&snapshot);
        if summary.is_empty() {
            None
        } else {
            Some(summary)
        }
    }

    fn tags_with_metadata<'a>(
        &'a self,
        tags: &'a [(&'a str, &'a str)],
    ) -> MetricsResult<Vec<(&'a str, &'a str)>> {
        let mut merged = tags.to_vec();
        merged.extend(self.metadata_tag_refs()?);
        Ok(merged)
    }

    fn metadata_tag_refs(&self) -> MetricsResult<Vec<(&str, &str)>> {
        if !self.metrics_use_metadata_tags {
            return Ok(Vec::new());
        }
        SessionMetricTagValues {
            auth_mode: self.metadata.auth_mode.as_deref(),
            session_source: self.metadata.session_source.as_str(),
            originator: self.metadata.originator.as_str(),
            service_name: self.metadata.service_name.as_deref(),
            model: self.metadata.model_category,
            app_version: self.metadata.app_version,
        }
        .into_tags()
    }

    #[allow(clippy::too_many_arguments)]
    pub fn new(
        conversation_id: ThreadId,
        model: &str,
        _slug: &str,
        account_id: Option<String>,
        account_email: Option<String>,
        auth_mode: Option<TelemetryAuthMode>,
        originator: String,
        log_user_prompts: bool,
        terminal_type: String,
        session_source: SessionSource,
    ) -> SessionTelemetry {
        Self {
            metadata: SessionTelemetryMetadata {
                _conversation_id: conversation_id,
                auth_mode: auth_mode.map(|m| m.to_string()),
                auth_env: AuthEnvTelemetryMetadata::default(),
                _account_id: account_id,
                _account_email: account_email,
                originator: bounded_originator_tag_value(originator.as_str()).to_string(),
                service_name: None,
                session_source: session_source.to_string(),
                model_category: bounded_model_category(model),
                service_tier: None,
                model_reasoning_effort: None,
                _log_user_prompts: log_user_prompts,
                app_version: env!("CARGO_PKG_VERSION"),
                terminal_type: bounded_terminal_type(&terminal_type).to_string(),
            },
            metrics: crate::metrics::global(),
            metrics_use_metadata_tags: true,
        }
    }

    pub fn record_responses(&self, handle_responses_span: &Span, event: &ResponseEvent) {
        handle_responses_span.record("otel.name", SessionTelemetry::responses_type(event));

        match event {
            ResponseEvent::OutputItemDone(item) => {
                handle_responses_span.record("from", "output_item_done");
                if let ResponseItem::FunctionCall { name, .. } = item {
                    handle_responses_span.record("tool_name", name.as_str());
                }
            }
            ResponseEvent::OutputItemAdded(item) => {
                handle_responses_span.record("from", "output_item_added");
                if let ResponseItem::FunctionCall { name, .. } = item {
                    handle_responses_span.record("tool_name", name.as_str());
                }
            }
            ResponseEvent::Completed {
                token_usage: Some(token_usage),
                ..
            } => {
                handle_responses_span.record("gen_ai.usage.input_tokens", token_usage.input_tokens);
                handle_responses_span.record(
                    "gen_ai.usage.cache_read.input_tokens",
                    token_usage.cached_input(),
                );
                handle_responses_span.record(
                    "gen_ai.usage.cache_write.input_tokens",
                    token_usage.cache_write_input_tokens,
                );
                handle_responses_span
                    .record("gen_ai.usage.output_tokens", token_usage.output_tokens);
                handle_responses_span.record(
                    "codex.usage.reasoning_output_tokens",
                    token_usage.reasoning_output_tokens,
                );
                handle_responses_span.record("codex.usage.total_tokens", token_usage.total_tokens);
            }
            _ => {}
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn conversation_starts(
        &self,
        provider_name: &str,
        reasoning_effort: Option<ReasoningEffort>,
        reasoning_summary: ReasoningSummary,
        context_window: Option<i64>,
        auto_compact_token_limit: Option<i64>,
        approval_policy: AskForApproval,
        sandbox_policy: SandboxPolicy,
        mcp_servers: Vec<&str>,
    ) {
        let provider_category = bounded_provider_category(provider_name);
        let reasoning_effort = reasoning_effort
            .as_ref()
            .map(ReasoningEffort::as_str)
            .map(bounded_reasoning_effort_category);
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.conversation_starts",
                provider.category = provider_category,
                auth.env_openai_api_key_present = self.metadata.auth_env.openai_api_key_env_present,
                auth.env_codex_api_key_present = self.metadata.auth_env.codex_api_key_env_present,
                auth.env_codex_api_key_enabled = self.metadata.auth_env.codex_api_key_env_enabled,
                auth.env_provider_key_present = self.metadata.auth_env.provider_env_key_present,
                auth.env_refresh_token_url_override_present = self.metadata.auth_env.refresh_token_url_override_present,
                reasoning_effort = reasoning_effort,
                reasoning_summary = %reasoning_summary,
                context_window = context_window,
                auto_compact_token_limit = auto_compact_token_limit,
                approval_policy = %approval_policy,
                sandbox_policy = %sandbox_policy,
            },
            log: {},
            trace: {
                mcp_server_count = mcp_servers.len() as i64,
            },
        );
    }

    pub async fn log_request<F, Fut>(&self, attempt: u64, f: F) -> Result<Response, Error>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<Response, Error>>,
    {
        let start = Instant::now();
        let response = f().await;
        let duration = start.elapsed();

        let (status, error_type) = match &response {
            Ok(response) => (Some(response.status().as_u16()), None),
            Err(error) => (
                error.status().map(|s| s.as_u16()),
                Some(reqwest_error_type(error)),
            ),
        };
        self.record_api_request(
            attempt, status, error_type, duration, /*auth_header_attached*/ false,
            /*auth_header_name*/ None, /*retry_after_unauthorized*/ false,
            /*recovery_mode*/ None, /*recovery_phase*/ None, "unknown",
            /*auth_error*/ None, /*auth_error_code*/ None,
        );

        response
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_api_request(
        &self,
        attempt: u64,
        status: Option<u16>,
        error_type: Option<&str>,
        duration: Duration,
        auth_header_attached: bool,
        auth_header_name: Option<&str>,
        retry_after_unauthorized: bool,
        recovery_mode: Option<&str>,
        recovery_phase: Option<&str>,
        endpoint: &str,
        auth_error: Option<&str>,
        auth_error_code: Option<&str>,
    ) {
        let error_type = auth_error
            .map(|_| "authentication")
            .or_else(|| bounded_error_type(error_type));
        let auth_header_name = bounded_auth_header_name(auth_header_name);
        let recovery_mode = bounded_auth_recovery_mode(recovery_mode);
        let recovery_phase = bounded_auth_recovery_phase(recovery_phase);
        let endpoint = bounded_endpoint(endpoint);
        let success =
            status.is_some_and(|code| (200..=299).contains(&code)) && error_type.is_none();
        let success_str = if success { "true" } else { "false" };
        let status_str = status
            .map(|code| code.to_string())
            .unwrap_or_else(|| "none".to_string());
        self.counter(
            API_CALL_COUNT_METRIC,
            /*inc*/ 1,
            &[("status", status_str.as_str()), ("success", success_str)],
        );
        self.record_duration(
            API_CALL_DURATION_METRIC,
            duration,
            &[("status", status_str.as_str()), ("success", success_str)],
        );
        let auth_error_code = bucket_auth_error_code(auth_error_code);
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.api_request",
                duration_ms = %duration.as_millis(),
                http.response.status_code = status,
                error.type = error_type,
                attempt = attempt,
                auth.header_attached = auth_header_attached,
                auth.header_name = auth_header_name,
                auth.retry_after_unauthorized = retry_after_unauthorized,
                auth.recovery_mode = recovery_mode,
                auth.recovery_phase = recovery_phase,
                endpoint = endpoint,
                auth.env_openai_api_key_present = self.metadata.auth_env.openai_api_key_env_present,
                auth.env_codex_api_key_present = self.metadata.auth_env.codex_api_key_env_present,
                auth.env_codex_api_key_enabled = self.metadata.auth_env.codex_api_key_env_enabled,
                auth.env_provider_key_present = self.metadata.auth_env.provider_env_key_present,
                auth.env_refresh_token_url_override_present = self.metadata.auth_env.refresh_token_url_override_present,
                auth.error_code = auth_error_code,
            },
            log: {},
            trace: {},
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_websocket_connect(
        &self,
        duration: Duration,
        status: Option<u16>,
        error_type: Option<&str>,
        auth_header_attached: bool,
        auth_header_name: Option<&str>,
        retry_after_unauthorized: bool,
        recovery_mode: Option<&str>,
        recovery_phase: Option<&str>,
        endpoint: &str,
        connection_reused: bool,
        auth_error: Option<&str>,
        auth_error_code: Option<&str>,
    ) {
        let error_type = auth_error
            .map(|_| "authentication")
            .or_else(|| bounded_error_type(error_type));
        let auth_header_name = bounded_auth_header_name(auth_header_name);
        let recovery_mode = bounded_auth_recovery_mode(recovery_mode);
        let recovery_phase = bounded_auth_recovery_phase(recovery_phase);
        let endpoint = bounded_endpoint(endpoint);
        let success = error_type.is_none()
            && status
                .map(|code| (200..=299).contains(&code))
                .unwrap_or(true);
        let success_str = if success { "true" } else { "false" };
        let auth_error_code = bucket_auth_error_code(auth_error_code);
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.websocket_connect",
                duration_ms = %duration.as_millis(),
                http.response.status_code = status,
                success = success_str,
                error.type = error_type,
                auth.header_attached = auth_header_attached,
                auth.header_name = auth_header_name,
                auth.retry_after_unauthorized = retry_after_unauthorized,
                auth.recovery_mode = recovery_mode,
                auth.recovery_phase = recovery_phase,
                endpoint = endpoint,
                auth.env_openai_api_key_present = self.metadata.auth_env.openai_api_key_env_present,
                auth.env_codex_api_key_present = self.metadata.auth_env.codex_api_key_env_present,
                auth.env_codex_api_key_enabled = self.metadata.auth_env.codex_api_key_env_enabled,
                auth.env_provider_key_present = self.metadata.auth_env.provider_env_key_present,
                auth.env_refresh_token_url_override_present = self.metadata.auth_env.refresh_token_url_override_present,
                auth.connection_reused = connection_reused,
                auth.error_code = auth_error_code,
            },
            log: {},
            trace: {},
        );
    }

    pub fn record_websocket_request(
        &self,
        duration: Duration,
        error_type: Option<&str>,
        connection_reused: bool,
    ) {
        let error_type = bounded_error_type(error_type);
        let success_str = if error_type.is_none() {
            "true"
        } else {
            "false"
        };
        self.counter(
            WEBSOCKET_REQUEST_COUNT_METRIC,
            /*inc*/ 1,
            &[("success", success_str)],
        );
        self.record_duration(
            WEBSOCKET_REQUEST_DURATION_METRIC,
            duration,
            &[("success", success_str)],
        );
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.websocket_request",
                duration_ms = %duration.as_millis(),
                success = success_str,
                error.type = error_type,
                auth.env_openai_api_key_present = self.metadata.auth_env.openai_api_key_env_present,
                auth.env_codex_api_key_present = self.metadata.auth_env.codex_api_key_env_present,
                auth.env_codex_api_key_enabled = self.metadata.auth_env.codex_api_key_env_enabled,
                auth.env_provider_key_present = self.metadata.auth_env.provider_env_key_present,
                auth.env_refresh_token_url_override_present = self.metadata.auth_env.refresh_token_url_override_present,
                auth.connection_reused = connection_reused,
            },
            log: {},
            trace: {},
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_auth_recovery(
        &self,
        mode: &str,
        step: &str,
        outcome: &str,
        auth_error: Option<&str>,
        auth_error_code: Option<&str>,
        recovery_reason: Option<&str>,
        auth_state_changed: Option<bool>,
    ) {
        let mode = bounded_auth_recovery_mode(Some(mode));
        let step = bounded_auth_recovery_phase(Some(step));
        let outcome = bounded_auth_recovery_outcome(outcome);
        let recovery_reason = bounded_auth_recovery_reason(recovery_reason);
        let error_type = auth_error.map(|_| "authentication");
        let auth_error_code = bucket_auth_error_code(auth_error_code);
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.auth_recovery",
                auth.mode = mode,
                auth.step = step,
                auth.outcome = outcome,
                error.type = error_type,
                auth.error_code = auth_error_code,
                auth.recovery_reason = recovery_reason,
                auth.state_changed = auth_state_changed,
            },
            log: {},
            trace: {},
        );
    }

    pub fn record_websocket_event(
        &self,
        result: &Result<
            Option<
                Result<
                    tokio_tungstenite::tungstenite::Message,
                    tokio_tungstenite::tungstenite::Error,
                >,
            >,
            ApiError,
        >,
        duration: Duration,
    ) {
        let mut kind = None;
        let mut success = true;

        match result {
            Ok(Some(Ok(message))) => match message {
                tokio_tungstenite::tungstenite::Message::Text(text) => {
                    match serde_json::from_str::<serde_json::Value>(text) {
                        Ok(value) => {
                            kind = value
                                .get("type")
                                .and_then(|value| value.as_str())
                                .map(std::string::ToString::to_string);
                            if kind.as_deref() == Some(RESPONSES_WEBSOCKET_TIMING_KIND) {
                                self.record_responses_websocket_timing_metrics(&value);
                            }
                            if kind.as_deref() == Some("response.failed") {
                                success = false;
                            }
                        }
                        Err(_) => {
                            kind = Some("parse_error".to_string());
                            success = false;
                        }
                    }
                }
                tokio_tungstenite::tungstenite::Message::Ping(_)
                | tokio_tungstenite::tungstenite::Message::Pong(_) => {
                    return;
                }
                tokio_tungstenite::tungstenite::Message::Binary(_)
                | tokio_tungstenite::tungstenite::Message::Close(_)
                | tokio_tungstenite::tungstenite::Message::Frame(_) => {
                    success = false;
                }
            },
            Ok(Some(Err(_))) | Ok(None) | Err(_) => {
                success = false;
            }
        }

        let kind_str = bounded_response_event_kind(kind.as_deref());
        let success_str = if success { "true" } else { "false" };
        let tags = [("kind", kind_str), ("success", success_str)];
        self.counter(WEBSOCKET_EVENT_COUNT_METRIC, /*inc*/ 1, &tags);
        self.record_duration(WEBSOCKET_EVENT_DURATION_METRIC, duration, &tags);
    }

    pub fn log_sse_event<E>(
        &self,
        response: &Result<Option<Result<StreamEvent, StreamError<E>>>, Elapsed>,
        duration: Duration,
    ) where
        E: std::fmt::Display,
    {
        match response {
            Ok(Some(Ok(sse))) => {
                if sse.data.trim() == "[DONE]" {
                    self.sse_event(&sse.event, duration);
                } else {
                    match serde_json::from_str::<serde_json::Value>(&sse.data) {
                        Ok(error) if sse.event == "response.failed" => {
                            self.sse_event_failed(Some(&sse.event), duration, &error);
                        }
                        Ok(content) if sse.event == "response.output_item.done" => {
                            match serde_json::from_value::<ResponseItem>(content) {
                                Ok(_) => self.sse_event(&sse.event, duration),
                                Err(_) => {
                                    self.sse_event_failed(
                                        Some(&sse.event),
                                        duration,
                                        &"failed to parse response.output_item.done",
                                    );
                                }
                            };
                        }
                        Ok(_) => {
                            self.sse_event(&sse.event, duration);
                        }
                        Err(error) => {
                            self.sse_event_failed(Some(&sse.event), duration, &error);
                        }
                    }
                }
            }
            Ok(Some(Err(error))) => {
                self.sse_event_failed(/*kind*/ None, duration, error);
            }
            Ok(None) => {}
            Err(_) => {
                self.sse_event_failed(
                    /*kind*/ None,
                    duration,
                    &"idle timeout waiting for SSE",
                );
            }
        }
    }

    fn sse_event(&self, kind: &str, duration: Duration) {
        let kind = bounded_response_event_kind(Some(kind));
        self.counter(
            SSE_EVENT_COUNT_METRIC,
            /*inc*/ 1,
            &[("kind", kind), ("success", "true")],
        );
        self.record_duration(
            SSE_EVENT_DURATION_METRIC,
            duration,
            &[("kind", kind), ("success", "true")],
        );
        log_event!(
            self,
            event.name = "codex.sse_event",
            event.kind = %kind,
            success = true,
            duration_ms = %duration.as_millis(),
        );
    }

    pub fn sse_event_failed<T>(&self, kind: Option<&String>, duration: Duration, _error: &T)
    where
        T: std::fmt::Display,
    {
        let kind_str = bounded_response_event_kind(kind.map(String::as_str));
        self.counter(
            SSE_EVENT_COUNT_METRIC,
            /*inc*/ 1,
            &[("kind", kind_str), ("success", "false")],
        );
        self.record_duration(
            SSE_EVENT_DURATION_METRIC,
            duration,
            &[("kind", kind_str), ("success", "false")],
        );
        let error_type = sse_error_type(kind.map(String::as_str));
        log_event!(
            self,
            event.name = "codex.sse_event",
            event.kind = %kind_str,
            success = false,
            duration_ms = %duration.as_millis(),
            error.type = error_type,
        );
        trace_event!(
            self,
            event.name = "codex.sse_event",
            event.kind = %kind_str,
            success = false,
            duration_ms = %duration.as_millis(),
            error.type = error_type,
        );
    }

    pub fn see_event_completed_failed<T>(&self, _error: &T)
    where
        T: std::fmt::Display,
    {
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.sse_event",
                event.kind = %"response.completed",
                success = false,
                error.type = "response_completed",
            },
            log: {},
            trace: {},
        );
    }

    pub fn sse_event_completed(&self, usage: &TokenUsage, ttft_ms: Option<i64>) {
        log_and_trace_event!(
            self,
            common: {
                event.name = "codex.sse_event",
                event.kind = %"response.completed",
                input_token_count = %usage.input_tokens,
                output_token_count = %usage.output_tokens,
                cached_token_count = usage.cached_input_tokens,
                cache_write_token_count = usage.cache_write_input_tokens,
                reasoning_token_count = usage.reasoning_output_tokens,
                tool_token_count = %usage.total_tokens,
                ttft_ms = ttft_ms,
                service_tier = self.metadata.service_tier.as_deref(),
                model_reasoning_effort = self.metadata.model_reasoning_effort.as_deref(),
            },
            log: {},
            trace: {},
        );
    }

    pub fn user_prompt(&self, items: &[UserInput]) {
        let prompt_length = items
            .iter()
            .map(|item| match item {
                UserInput::Text { text, .. } => text.chars().count(),
                _ => 0,
            })
            .sum::<usize>();
        let text_input_count = items
            .iter()
            .filter(|item| matches!(item, UserInput::Text { .. }))
            .count();
        let image_input_count = items
            .iter()
            .filter(|item| matches!(item, UserInput::Image { .. }))
            .count();
        let local_image_input_count = items
            .iter()
            .filter(|item| matches!(item, UserInput::LocalImage { .. }))
            .count();

        log_event!(
            self,
            event.name = "codex.user_prompt",
            prompt_length = prompt_length as i64,
            text_input_count = text_input_count as i64,
            image_input_count = image_input_count as i64,
            local_image_input_count = local_image_input_count as i64,
        );
        trace_event!(
            self,
            event.name = "codex.user_prompt",
            prompt_length = prompt_length as i64,
            text_input_count = text_input_count as i64,
            image_input_count = image_input_count as i64,
            local_image_input_count = local_image_input_count as i64,
        );
    }

    pub fn tool_decision(
        &self,
        tool_name: &str,
        _call_id: &str,
        decision: &ReviewDecision,
        source: Option<ToolDecisionSource>,
    ) {
        let tool_name = bounded_tool_category(tool_name, /*is_mcp*/ false);
        let decision = decision.to_opaque_string();
        match source {
            Some(source) => log_event!(
                self,
                event.name = "codex.tool_decision",
                tool_name = %tool_name,
                decision = %decision,
                source = %source.to_string(),
            ),
            None => log_event!(
                self,
                event.name = "codex.tool_decision",
                tool_name = %tool_name,
                decision = %decision,
            ),
        }
    }

    pub fn sandbox_outcome(
        &self,
        tool_name: &str,
        _call_id: &str,
        outcome: &str,
        initial_duration: Duration,
        escalated_duration: Option<Duration>,
    ) {
        let tool_name = bounded_tool_category(tool_name, /*is_mcp*/ false);
        let outcome = bounded_sandbox_outcome(outcome);
        let initial_duration_ms = initial_duration.as_millis().min(i64::MAX as u128) as i64;
        let escalated_duration_ms =
            escalated_duration.map(|duration| duration.as_millis().min(i64::MAX as u128) as i64);
        log_event!(
            self,
            event.name = "codex.sandbox_outcome",
            tool_name = %tool_name,
            outcome = %outcome,
            initial_duration_ms = initial_duration_ms,
            escalated_duration_ms = escalated_duration_ms,
        );
        trace_event!(
            self,
            event.name = "codex.sandbox_outcome",
            tool_name = %tool_name,
            outcome = %outcome,
            initial_duration_ms = initial_duration_ms,
            escalated_duration_ms = escalated_duration_ms,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn log_tool_result_with_tags<F, Fut, E>(
        &self,
        tool_name: &str,
        call_id: &str,
        arguments: &str,
        extra_tags: &[(&str, &str)],
        extra_trace_fields: &[(&str, &str)],
        f: F,
    ) -> Result<(String, bool), E>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<(String, bool), E>>,
        E: std::fmt::Display,
    {
        let start = Instant::now();
        let result = f().await;
        let duration = start.elapsed();

        let (output, success) = match &result {
            Ok((preview, success)) => (Cow::Borrowed(preview.as_str()), *success),
            Err(error) => (Cow::Owned(error.to_string()), false),
        };

        self.tool_result_with_tags(
            tool_name,
            call_id,
            arguments,
            duration,
            success,
            output.as_ref(),
            extra_tags,
            extra_trace_fields,
        );

        result
    }

    pub fn log_tool_failed(&self, tool_name: &str, error: &str) {
        let tool_name = bounded_tool_category(tool_name, /*is_mcp*/ false);
        log_event!(
            self,
            event.name = "codex.tool_result",
            tool_name = %tool_name,
            duration_ms = %Duration::ZERO.as_millis(),
            success = %false,
            output_length = error.len() as i64,
            output_line_count = error.lines().count() as i64,
            tool_origin = "builtin",
            mcp_tool = false,
        );
        trace_event!(
            self,
            event.name = "codex.tool_result",
            tool_name = %tool_name,
            duration_ms = %Duration::ZERO.as_millis(),
            success = %false,
            output_length = error.len() as i64,
            output_line_count = error.lines().count() as i64,
            tool_origin = %"builtin",
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tool_result_with_tags(
        &self,
        tool_name: &str,
        _call_id: &str,
        arguments: &str,
        duration: Duration,
        success: bool,
        output: &str,
        extra_tags: &[(&str, &str)],
        extra_trace_fields: &[(&str, &str)],
    ) {
        let success_str = if success { "true" } else { "false" };
        let mcp_server = trace_field_value(extra_trace_fields, "mcp_server").unwrap_or("");
        let is_mcp = !mcp_server.is_empty();
        let tool_name = bounded_tool_category(tool_name, is_mcp);
        let mut tags = Vec::with_capacity(2 + extra_tags.len());
        tags.push(("tool", tool_name));
        tags.push(("success", success_str));
        tags.extend(
            extra_tags
                .iter()
                .copied()
                .filter(|(key, _)| allowed_tool_metric_tag(key)),
        );
        self.counter(TOOL_CALL_COUNT_METRIC, /*inc*/ 1, &tags);
        self.record_duration(TOOL_CALL_DURATION_METRIC, duration, &tags);
        log_event!(
            self,
            event.name = "codex.tool_result",
            tool_name = %tool_name,
            arguments_length = arguments.len() as i64,
            duration_ms = %duration.as_millis(),
            success = %success_str,
            output_length = output.len() as i64,
            output_line_count = output.lines().count() as i64,
            tool_origin = if is_mcp { "mcp" } else { "builtin" },
            mcp_tool = is_mcp,
        );
        trace_event!(
            self,
            event.name = "codex.tool_result",
            tool_name = %tool_name,
            duration_ms = %duration.as_millis(),
            success = %success_str,
            arguments_length = arguments.len() as i64,
            output_length = output.len() as i64,
            output_line_count = output.lines().count() as i64,
            tool_origin = if is_mcp { "mcp" } else { "builtin" },
            mcp_tool = is_mcp,
        );
    }

    fn record_responses_websocket_timing_metrics(&self, value: &serde_json::Value) {
        let timing_metrics = value.get(RESPONSES_WEBSOCKET_TIMING_METRICS_FIELD);

        let overhead_value =
            timing_metrics.and_then(|value| value.get(RESPONSES_API_OVERHEAD_FIELD));
        if let Some(duration) = duration_from_ms_value(overhead_value) {
            self.record_duration(RESPONSES_API_OVERHEAD_DURATION_METRIC, duration, &[]);
        }

        let inference_value =
            timing_metrics.and_then(|value| value.get(RESPONSES_API_INFERENCE_FIELD));
        if let Some(duration) = duration_from_ms_value(inference_value) {
            self.record_duration(RESPONSES_API_INFERENCE_TIME_DURATION_METRIC, duration, &[]);
        }

        let engine_iapi_ttft_value =
            timing_metrics.and_then(|value| value.get(RESPONSES_API_ENGINE_IAPI_TTFT_FIELD));
        if let Some(duration) = duration_from_ms_value(engine_iapi_ttft_value) {
            self.record_duration(
                RESPONSES_API_ENGINE_IAPI_TTFT_DURATION_METRIC,
                duration,
                &[],
            );
        }

        let engine_service_ttft_value =
            timing_metrics.and_then(|value| value.get(RESPONSES_API_ENGINE_SERVICE_TTFT_FIELD));
        if let Some(duration) = duration_from_ms_value(engine_service_ttft_value) {
            self.record_duration(
                RESPONSES_API_ENGINE_SERVICE_TTFT_DURATION_METRIC,
                duration,
                &[],
            );
        }

        let engine_iapi_tbt_value =
            timing_metrics.and_then(|value| value.get(RESPONSES_API_ENGINE_IAPI_TBT_FIELD));
        if let Some(duration_ms) = f64_ms_value(engine_iapi_tbt_value) {
            self.record_duration_ms_f64(
                RESPONSES_API_ENGINE_IAPI_TBT_DURATION_METRIC,
                duration_ms,
                &[],
            );
        }

        let engine_service_tbt_value =
            timing_metrics.and_then(|value| value.get(RESPONSES_API_ENGINE_SERVICE_TBT_FIELD));
        if let Some(duration_ms) = f64_ms_value(engine_service_tbt_value) {
            self.record_duration_ms_f64(
                RESPONSES_API_ENGINE_SERVICE_TBT_DURATION_METRIC,
                duration_ms,
                &[],
            );
        }
    }

    fn responses_type(event: &ResponseEvent) -> String {
        match event {
            ResponseEvent::Created => "created".into(),
            ResponseEvent::OutputItemDone(item) | ResponseEvent::OutputItemAdded(item) => {
                SessionTelemetry::responses_item_type(item)
            }
            ResponseEvent::Completed { .. } => "completed".into(),
            ResponseEvent::OutputTextDelta(_) => "text_delta".into(),
            ResponseEvent::ToolCallInputDelta { .. } => "tool_input_delta".into(),
            ResponseEvent::ReasoningSummaryDelta { .. } => "reasoning_summary_delta".into(),
            ResponseEvent::ReasoningSummaryDone { .. } => "reasoning_summary_done".into(),
            ResponseEvent::ReasoningContentDelta { .. } => "reasoning_content_delta".into(),
            ResponseEvent::ReasoningSummaryPartAdded { .. } => {
                "reasoning_summary_part_added".into()
            }
            ResponseEvent::ServerModel(_) => "server_model".into(),
            ResponseEvent::ModelVerifications(_) => "model_verifications".into(),
            ResponseEvent::TurnModerationMetadata(_) => "turn_moderation_metadata".into(),
            ResponseEvent::SafetyBuffering(_) => "safety_buffering".into(),
            ResponseEvent::ServerReasoningIncluded(_) => "server_reasoning_included".into(),
            ResponseEvent::RateLimits(_) => "rate_limits".into(),
            ResponseEvent::ModelsEtag(_) => "models_etag".into(),
        }
    }

    fn responses_item_type(item: &ResponseItem) -> String {
        match item {
            ResponseItem::AdditionalTools { .. } => "additional_tools".into(),
            ResponseItem::Message { role, .. } => format!("message_from_{role}"),
            ResponseItem::AgentMessage { .. } => "agent_message".into(),
            ResponseItem::Reasoning { .. } => "reasoning".into(),
            ResponseItem::LocalShellCall { .. } => "local_shell_call".into(),
            ResponseItem::FunctionCall { .. } => "function_call".into(),
            ResponseItem::ToolSearchCall { .. } => "tool_search_call".into(),
            ResponseItem::FunctionCallOutput { .. } => "function_call_output".into(),
            ResponseItem::ToolSearchOutput { .. } => "tool_search_output".into(),
            ResponseItem::CustomToolCall { .. } => "custom_tool_call".into(),
            ResponseItem::CustomToolCallOutput { .. } => "custom_tool_call_output".into(),
            ResponseItem::WebSearchCall { .. } => "web_search_call".into(),
            ResponseItem::ImageGenerationCall { .. } => "image_generation_call".into(),
            ResponseItem::Compaction { .. } => "compaction".into(),
            ResponseItem::CompactionTrigger { .. } => "compaction_trigger".into(),
            ResponseItem::ContextCompaction { .. } => "context_compaction".into(),
            ResponseItem::Other => "other".into(),
        }
    }
}

fn duration_from_ms_value(value: Option<&serde_json::Value>) -> Option<Duration> {
    let ms = f64_ms_value(value)?;
    Some(Duration::from_millis(ms.round() as u64))
}

fn f64_ms_value(value: Option<&serde_json::Value>) -> Option<f64> {
    let value = value?;
    let ms = value
        .as_f64()
        .or_else(|| value.as_i64().map(|v| v as f64))
        .or_else(|| value.as_u64().map(|v| v as f64))?;
    if !ms.is_finite() || ms < 0.0 {
        return None;
    }
    Some(ms.min(u64::MAX as f64))
}
