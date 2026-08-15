use base64::Engine;
use codex_api::ApiError;
use codex_api::TransportError;

const REQUEST_ID_HEADER: &str = "x-request-id";
const OAI_REQUEST_ID_HEADER: &str = "x-oai-request-id";
const CF_RAY_HEADER: &str = "cf-ray";
const AUTH_ERROR_HEADER: &str = "x-openai-authorization-error";
const X_ERROR_JSON_HEADER: &str = "x-error-json";

fn bucket_auth_error_code(code: &str) -> &'static str {
    match code {
        "token_expired" => "token_expired",
        "refresh_token_expired" => "refresh_token_expired",
        _ => "other",
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ResponseDebugContext {
    pub request_id: Option<String>,
    pub cf_ray: Option<String>,
    pub auth_error: Option<String>,
    pub auth_error_code: Option<String>,
}

pub fn extract_response_debug_context(transport: &TransportError) -> ResponseDebugContext {
    let mut context = ResponseDebugContext::default();

    let TransportError::Http { headers, .. } = transport else {
        return context;
    };

    let extract_header = |name: &str| {
        headers
            .as_ref()
            .and_then(|headers| headers.get(name))
            .and_then(|value| value.to_str().ok())
            .map(str::to_string)
    };

    context.request_id =
        extract_header(REQUEST_ID_HEADER).or_else(|| extract_header(OAI_REQUEST_ID_HEADER));
    context.cf_ray = extract_header(CF_RAY_HEADER);
    context.auth_error = extract_header(AUTH_ERROR_HEADER);
    context.auth_error_code = extract_header(X_ERROR_JSON_HEADER).and_then(|encoded| {
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .ok()?;
        let parsed = serde_json::from_slice::<serde_json::Value>(&decoded).ok()?;
        parsed
            .get("error")
            .and_then(|error| error.get("code"))
            .and_then(serde_json::Value::as_str)
            .map(bucket_auth_error_code)
            .map(str::to_string)
    });

    context
}

pub fn extract_response_debug_context_from_api_error(error: &ApiError) -> ResponseDebugContext {
    match error {
        ApiError::Transport(transport) => extract_response_debug_context(transport),
        _ => ResponseDebugContext::default(),
    }
}

pub fn telemetry_transport_error_type(error: &TransportError) -> &'static str {
    match error {
        TransportError::Http { .. } => "http",
        TransportError::RetryLimit => "retry_limit",
        TransportError::Timeout => "timeout",
        TransportError::Connection(_) => "connection",
        TransportError::Network(_) => "network",
        TransportError::Build(_) => "build",
    }
}

pub fn telemetry_api_error_type(error: &ApiError) -> &'static str {
    match error {
        ApiError::Transport(transport) => telemetry_transport_error_type(transport),
        ApiError::Api { .. } => "api",
        ApiError::Stream(_) => "stream",
        ApiError::ContextWindowExceeded => "context_window_exceeded",
        ApiError::QuotaExceeded => "quota_exceeded",
        ApiError::UsageNotIncluded => "usage_not_included",
        ApiError::Retryable { .. } => "retryable",
        ApiError::RateLimit(_) => "rate_limit",
        ApiError::InvalidRequest { .. } => "invalid_request",
        ApiError::CyberPolicy { .. } => "cyber_policy",
        ApiError::MisalignmentPolicyViolation { .. } => "misalignment_policy_violation",
        ApiError::ServerOverloaded => "server_overloaded",
    }
}

#[cfg(test)]
mod tests {
    use super::ResponseDebugContext;
    use super::extract_response_debug_context;
    use super::telemetry_api_error_type;
    use super::telemetry_transport_error_type;
    use codex_api::ApiError;
    use codex_api::TransportError;
    use http::HeaderMap;
    use http::HeaderValue;
    use http::StatusCode;
    use pretty_assertions::assert_eq;

    #[test]
    fn extract_response_debug_context_decodes_identity_headers() {
        let mut headers = HeaderMap::new();
        headers.insert("x-oai-request-id", HeaderValue::from_static("req-auth"));
        headers.insert("cf-ray", HeaderValue::from_static("ray-auth"));
        headers.insert(
            "x-openai-authorization-error",
            HeaderValue::from_static("missing_authorization_header"),
        );
        headers.insert(
            "x-error-json",
            HeaderValue::from_static("eyJlcnJvciI6eyJjb2RlIjoidG9rZW5fZXhwaXJlZCJ9fQ=="),
        );

        let context = extract_response_debug_context(&TransportError::Http {
            status: StatusCode::UNAUTHORIZED,
            url: Some("https://chatgpt.com/backend-api/codex/models".to_string()),
            headers: Some(headers),
            body: Some(r#"{"error":{"message":"plain text error"},"status":401}"#.to_string()),
        });

        assert_eq!(
            context,
            ResponseDebugContext {
                request_id: Some("req-auth".to_string()),
                cf_ray: Some("ray-auth".to_string()),
                auth_error: Some("missing_authorization_header".to_string()),
                auth_error_code: Some("token_expired".to_string()),
            }
        );
    }

    #[test]
    fn telemetry_error_types_omit_http_bodies() {
        let transport = TransportError::Http {
            status: StatusCode::UNAUTHORIZED,
            url: Some("https://chatgpt.com/backend-api/codex/responses".to_string()),
            headers: None,
            body: Some(r#"{"error":{"message":"secret token leaked"}}"#.to_string()),
        };

        assert_eq!(telemetry_transport_error_type(&transport), "http");
        assert_eq!(
            telemetry_api_error_type(&ApiError::Transport(transport)),
            "http"
        );
    }

    #[test]
    fn telemetry_error_types_bucket_non_http_details() {
        let network = TransportError::Network("dns lookup failed".to_string());
        let build = TransportError::Build("invalid header value".to_string());
        let stream = ApiError::Stream("socket closed".to_string());

        assert_eq!(telemetry_transport_error_type(&network), "network");
        assert_eq!(telemetry_transport_error_type(&build), "build");
        assert_eq!(telemetry_api_error_type(&stream), "stream");
    }
}
