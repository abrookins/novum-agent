pub(crate) const OTEL_AGENT_COMMUNICATION_TARGET: &str = "codex_otel.agent_communication";
pub(crate) const OTEL_LOG_ONLY_TARGET: &str = "codex_otel.log_only";
pub(crate) const OTEL_NETWORK_PROXY_TARGET: &str = "codex_otel.network_proxy";
pub(crate) const OTEL_TRACE_SAFE_TARGET: &str = "codex_otel.trace_safe";

pub(crate) fn is_log_export_target(target: &str) -> bool {
    matches!(
        target,
        OTEL_AGENT_COMMUNICATION_TARGET | OTEL_LOG_ONLY_TARGET | OTEL_NETWORK_PROXY_TARGET
    )
}

pub(crate) fn is_trace_safe_target(target: &str) -> bool {
    target.starts_with(OTEL_TRACE_SAFE_TARGET)
}
