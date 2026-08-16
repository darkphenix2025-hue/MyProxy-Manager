use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub const MAX_CALLBACK_HEADER_BYTES: usize = 8 * 1024;
const CALLBACK_READ_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub enum CodexLoopbackError {
    #[error("failed to bind Codex loopback listener")]
    Bind(#[source] std::io::Error),
    #[error("failed to accept Codex loopback callback")]
    Accept(#[source] std::io::Error),
    #[error("Codex callback request is invalid")]
    InvalidRequest,
    #[error("Codex callback request is too large")]
    RequestTooLarge,
    #[error("Codex callback request timed out")]
    Timeout,
    #[error("failed to send Codex callback response")]
    Response(#[source] std::io::Error),
}

#[derive(Debug)]
pub struct CodexLoopbackListener {
    listener: tokio::net::TcpListener,
    local_addr: std::net::SocketAddr,
}

impl CodexLoopbackListener {
    pub async fn bind(address: impl ToString) -> Result<Self, CodexLoopbackError> {
        let address = address
            .to_string()
            .parse::<std::net::SocketAddr>()
            .map_err(|error| {
                CodexLoopbackError::Bind(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    error,
                ))
            })?;
        if !address.ip().is_loopback() {
            return Err(CodexLoopbackError::Bind(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "callback listener must bind to loopback",
            )));
        }
        let listener = tokio::net::TcpListener::bind(address)
            .await
            .map_err(CodexLoopbackError::Bind)?;
        let local_addr = listener.local_addr().map_err(CodexLoopbackError::Bind)?;
        Ok(Self {
            listener,
            local_addr,
        })
    }

    pub fn local_addr(&self) -> std::net::SocketAddr {
        self.local_addr
    }

    pub fn redirect_uri(&self) -> String {
        match self.local_addr {
            std::net::SocketAddr::V4(address) => {
                format!("http://{}:{}/auth/callback", address.ip(), address.port())
            }
            std::net::SocketAddr::V6(address) => {
                format!("http://[{}]:{}/auth/callback", address.ip(), address.port())
            }
        }
    }

    pub async fn accept(&self) -> Result<CodexPendingCallback, CodexLoopbackError> {
        loop {
            let (mut stream, peer) = self
                .listener
                .accept()
                .await
                .map_err(CodexLoopbackError::Accept)?;
            if !peer.ip().is_loopback() {
                let _ = write_response(&mut stream, false).await;
                continue;
            }

            let request = match tokio::time::timeout(
                CALLBACK_READ_TIMEOUT,
                read_headers(&mut stream),
            )
            .await
            {
                Ok(Ok(request)) => request,
                Ok(Err(_)) | Err(_) => {
                    let _ = write_response(&mut stream, false).await;
                    continue;
                }
            };
            let (code, state) = match parse_callback_request(&request) {
                Ok(callback) => callback,
                Err(_) => {
                    let _ = write_response(&mut stream, false).await;
                    continue;
                }
            };
            return Ok(CodexPendingCallback {
                code: zeroize::Zeroizing::new(code),
                state: zeroize::Zeroizing::new(state),
                stream,
            });
        }
    }
}

async fn read_headers(stream: &mut tokio::net::TcpStream) -> Result<Vec<u8>, CodexLoopbackError> {
    let mut request = Vec::with_capacity(1024);
    let mut chunk = [0_u8; 1024];
    loop {
        let read = stream
            .read(&mut chunk)
            .await
            .map_err(CodexLoopbackError::Accept)?;
        if read == 0 {
            return Err(CodexLoopbackError::InvalidRequest);
        }
        request.extend_from_slice(&chunk[..read]);
        if request.len() > MAX_CALLBACK_HEADER_BYTES {
            return Err(CodexLoopbackError::RequestTooLarge);
        }
        if request.windows(4).any(|window| window == b"\r\n\r\n") {
            return Ok(request);
        }
    }
}

fn parse_callback_request(request: &[u8]) -> Result<(String, String), CodexLoopbackError> {
    let request = std::str::from_utf8(request).map_err(|_| CodexLoopbackError::InvalidRequest)?;
    let request_line = request
        .split("\r\n")
        .next()
        .ok_or(CodexLoopbackError::InvalidRequest)?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next().ok_or(CodexLoopbackError::InvalidRequest)?;
    let target = parts.next().ok_or(CodexLoopbackError::InvalidRequest)?;
    let version = parts.next().ok_or(CodexLoopbackError::InvalidRequest)?;
    if method != "GET" || !version.starts_with("HTTP/1.") || parts.next().is_some() {
        return Err(CodexLoopbackError::InvalidRequest);
    }
    let url = url::Url::parse(&format!("http://localhost{target}"))
        .map_err(|_| CodexLoopbackError::InvalidRequest)?;
    if url.path() != "/auth/callback" || url.fragment().is_some() {
        return Err(CodexLoopbackError::InvalidRequest);
    }
    let mut codes = url.query_pairs().filter(|(key, _)| key == "code");
    let code = codes
        .next()
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.trim().is_empty())
        .ok_or(CodexLoopbackError::InvalidRequest)?;
    if codes.next().is_some() {
        return Err(CodexLoopbackError::InvalidRequest);
    }
    let mut states = url.query_pairs().filter(|(key, _)| key == "state");
    let state = states
        .next()
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.trim().is_empty())
        .ok_or(CodexLoopbackError::InvalidRequest)?;
    if states.next().is_some() {
        return Err(CodexLoopbackError::InvalidRequest);
    }
    Ok((code, state))
}

pub struct CodexPendingCallback {
    code: zeroize::Zeroizing<String>,
    state: zeroize::Zeroizing<String>,
    stream: tokio::net::TcpStream,
}

impl std::fmt::Debug for CodexPendingCallback {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CodexPendingCallback")
            .field("code", &"[REDACTED]")
            .field("state", &"[REDACTED]")
            .finish_non_exhaustive()
    }
}

impl CodexPendingCallback {
    pub fn code(&self) -> &str {
        self.code.as_str()
    }

    pub fn state(&self) -> &str {
        self.state.as_str()
    }

    pub async fn respond_success(mut self) -> Result<(), CodexLoopbackError> {
        write_response(&mut self.stream, true).await
    }

    pub async fn respond_failure(mut self) -> Result<(), CodexLoopbackError> {
        write_response(&mut self.stream, false).await
    }
}

async fn write_response(
    stream: &mut tokio::net::TcpStream,
    success: bool,
) -> Result<(), CodexLoopbackError> {
    let (status, title, message) = if success {
        (
            "200 OK",
            "Authentication complete",
            "You can close this window and return to MyProxy Manager.",
        )
    } else {
        (
            "400 Bad Request",
            "Authentication failed",
            "Return to MyProxy Manager and start the sign-in flow again.",
        )
    };
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{title}</title></head><body><main><h1>{title}</h1><p>{message}</p></main></body></html>"
    );
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nCache-Control: no-store\r\nContent-Security-Policy: default-src 'none'; style-src 'unsafe-inline'\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(CodexLoopbackError::Response)?;
    stream
        .shutdown()
        .await
        .map_err(CodexLoopbackError::Response)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn send_request(address: std::net::SocketAddr, request: &str) -> String {
        let mut stream = tokio::net::TcpStream::connect(address).await.unwrap();
        stream.write_all(request.as_bytes()).await.unwrap();
        stream.shutdown().await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        response
    }

    #[tokio::test]
    async fn listener_accepts_one_exact_callback_and_responds_after_validation() {
        let listener = CodexLoopbackListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr();
        let client = tokio::spawn(async move {
            send_request(
                address,
                "GET /auth/callback?code=code-value&state=state-value HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n",
            )
            .await
        });

        let callback = listener.accept().await.unwrap();
        assert_eq!(callback.code(), "code-value");
        assert_eq!(callback.state(), "state-value");
        callback.respond_success().await.unwrap();

        let response = client.await.unwrap();
        assert!(response.starts_with("HTTP/1.1 200 OK"));
        assert!(!response.contains("code-value"));
        assert!(!response.contains("state-value"));
    }

    #[tokio::test]
    async fn listener_ignores_invalid_requests_until_valid_callback_arrives() {
        for request in [
            "POST /auth/callback?code=x&state=y HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string(),
            "GET /wrong?code=x&state=y HTTP/1.1\r\nHost: localhost\r\n\r\n".to_string(),
            format!(
                "GET /auth/callback?code=x&state=y HTTP/1.1\r\nX-Large: {}\r\n\r\n",
                "a".repeat(MAX_CALLBACK_HEADER_BYTES)
            ),
        ] {
            let listener = CodexLoopbackListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr();
            let invalid = tokio::spawn(async move { send_request(address, &request).await });
            let accepting = tokio::spawn(async move { listener.accept().await.unwrap() });
            assert!(invalid
                .await
                .unwrap()
                .starts_with("HTTP/1.1 400 Bad Request"));
            let valid = tokio::spawn(async move {
                send_request(
                    address,
                    "GET /auth/callback?code=valid-code&state=valid-state HTTP/1.1\r\nHost: localhost\r\n\r\n",
                )
                .await
            });
            let callback = accepting.await.unwrap();
            assert_eq!(callback.code(), "valid-code");
            callback.respond_success().await.unwrap();
            assert!(valid.await.unwrap().starts_with("HTTP/1.1 200 OK"));
        }
    }

    #[tokio::test]
    async fn listener_reports_port_conflict() {
        let occupied = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = occupied.local_addr().unwrap();
        assert!(matches!(
            CodexLoopbackListener::bind(address).await,
            Err(CodexLoopbackError::Bind(_))
        ));
    }
}
