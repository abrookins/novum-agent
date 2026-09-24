const AGENT_COMMUNICATION_TARGET: &str = "codex_otel.agent_communication";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AgentCommunicationKind {
    Spawn,
    Message,
    Followup,
    Result,
}

impl AgentCommunicationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Spawn => "spawn",
            Self::Message => "message",
            Self::Followup => "followup",
            Self::Result => "result",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentCommunicationContext {
    kind: AgentCommunicationKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AgentCommunicationTelemetry {
    pub(crate) content_length: usize,
    pub(crate) encrypted_content_present: bool,
}

impl AgentCommunicationContext {
    pub(crate) fn new(kind: AgentCommunicationKind) -> Self {
        Self { kind }
    }
}

pub(crate) fn logging_enabled() -> bool {
    tracing::enabled!(target: AGENT_COMMUNICATION_TARGET, tracing::Level::INFO)
}

pub(crate) fn emit_agent_communication_send(
    context: &AgentCommunicationContext,
    communication: &AgentCommunicationTelemetry,
) {
    tracing::info!(
        target: AGENT_COMMUNICATION_TARGET,
        parent: None,
        {
            event.name = "codex.agent_communication",
            kind = context.kind.as_str(),
            state = "send",
            content_length = communication.content_length,
            encrypted_content_present = communication.encrypted_content_present,
        },
        "agent communication"
    );
}

pub(crate) fn emit_agent_communication_receive() {
    tracing::info!(
        target: AGENT_COMMUNICATION_TARGET,
        parent: None,
        {
            event.name = "codex.agent_communication",
            state = "receive",
        },
        "agent communication"
    );
}
