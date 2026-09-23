use super::*;
use http::HeaderValue;
use std::io::Read;
use std::io::Write;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::Mutex;
use tracing_subscriber::Layer;
use tracing_subscriber::filter::Targets;
use tracing_subscriber::layer::SubscriberExt;

struct CapturedLogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for CapturedLogWriter {
    fn write(&mut self, buffer: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .expect("captured log lock should not be poisoned")
            .extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn custom_ca_fallback_preserves_builder_configuration() {
    let output = Arc::new(Mutex::new(Vec::new()));
    let writer_output = Arc::clone(&output);
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .without_time()
            .with_writer(move || CapturedLogWriter(Arc::clone(&writer_output)))
            .with_filter(Targets::new().with_target("codex_otel.log_only", tracing::Level::INFO)),
    );
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::callsite::rebuild_interest_cache();

    let listener =
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("HTTP listener should bind");
    let address = listener
        .local_addr()
        .expect("HTTP listener should have an address");
    let server = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("HTTP listener should accept");
        let mut request = Vec::new();
        let mut chunk = [0_u8; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let bytes_read = stream.read(&mut chunk).expect("HTTP request should read");
            assert!(bytes_read > 0, "HTTP request should include headers");
            request.extend_from_slice(&chunk[..bytes_read]);
        }
        stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
            .expect("HTTP listener should write response");
        String::from_utf8(request).expect("HTTP request should be UTF-8")
    });
    let mut headers = HeaderMap::new();
    headers.insert("x-builder-test", HeaderValue::from_static("preserved"));
    let client = HttpClientBuilder::new()
        .default_headers(headers)
        .build_with_custom_ca_fallback_using(ProxyRouting::Direct, |_| {
            Err(BuildCustomCaTransportError::InvalidCaFile {
                source_env: "TEST_CA_ENV",
                path: PathBuf::from("canary-private-ca.pem"),
                detail: "canary custom CA build error".to_string(),
            })
        });

    let client = HttpClient {
        backend: HttpClientBackend::Direct(client),
    };

    let response = client
        .get(format!("http://{address}/fallback"))
        .send()
        .await
        .expect("fallback client should send request");
    assert!(response.status().is_success());
    let request = server.join().expect("HTTP listener should finish");
    assert!(
        request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("x-builder-test: preserved"))
    );
    let logs = String::from_utf8(
        output
            .lock()
            .expect("captured log lock should not be poisoned")
            .clone(),
    )
    .expect("captured logs should be UTF-8");
    assert!(logs.contains("event.name=\"codex.http_client.custom_ca_fallback\""));
    assert!(!logs.contains("canary-private-ca.pem"));
    assert!(!logs.contains("canary custom CA build error"));
}
