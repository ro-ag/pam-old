#![forbid(unsafe_code)]

mod codec;
mod contract;

#[cfg(test)]
mod codec_test;
#[cfg(test)]
mod contract_test;

pub use codec::{
    CodecError, decode_request, decode_request_envelope, decode_server_message,
    decode_server_message_envelope, encode,
};
pub use contract::{
    ActivityEventSummary, ActivityResult, ApprovalChallenge, ApprovalDecision,
    ApprovalDecisionDisposition, ApprovalDecisionResult, BriefItem, BriefProvenance, BriefResult,
    CallerListResult, CallerSummary, CancellationDisposition, CancellationResult, Capability,
    ConfigurationPresence, ConnectorConfigureResult, ConnectorCredentialAction,
    ConnectorListResult, ConnectorSecret, ConnectorSummary, ConnectorTestDisposition,
    ConnectorTestResult, DaemonLifecycleResult, DaemonLogEntry, DaemonLogsResult, Event,
    EventEnvelope, EvidenceChunk, EvidenceMetadata, EvidenceRedaction, EvidenceRetention,
    ExpectedTargetKind, Failure, FailureCode, FlowDefinitionDocument, FlowProjectRoot,
    LogSeverity, MAX_CONNECTOR_BASE_URL_BYTES,
    MAX_CONNECTOR_ID_BYTES, MAX_CONNECTOR_SECRET_BYTES, MAX_FLOW_PROJECT_ROOT_BYTES,
    MAX_PROJECT_CURRENT_QUEUED, MAX_PROJECT_OPERATION_KIND_BYTES, ModelFinishReason,
    ModelGenerationResult, ModelMessage, ModelRole, ModelStatusResult, ModelSummary, ModelUsage,
    NetworkDiagnosticsResult, OperationTruth, PacState, ProjectCurrentResult, ProjectRequestState,
    ProjectRequestSummary, ProtocolContractError, ReplayResult, RequestEnvelope, RequestPayload,
    ResultBody, ResultEnvelope, ResultPayload, ServerMessage, SourceAvailability, StatusResult,
};

pub const PROTOCOL_VERSION: u16 = 7;
pub const MAX_FRAME_SIZE: usize = 1024 * 1024;
pub const MAX_EVIDENCE_CHUNK_SIZE: usize = 256 * 1024;
pub const MAX_MODEL_MESSAGES: usize = 32;
pub const MAX_MODEL_MESSAGE_BYTES: usize = 64 * 1024;
pub const MAX_MODEL_PROMPT_BYTES: usize = 256 * 1024;
pub const MAX_MODEL_OUTPUT_TOKENS: u32 = 4_096;
pub const MAX_MODEL_OUTPUT_BYTES: usize = 512 * 1024;
