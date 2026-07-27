//! A deterministic loopback HTTP/1.1 server for the fetch-transport
//! tests (issue #2015).
//!
//! Guardrail 3 of epic #2012 is binding: **every fetch test drives a
//! local server and never the public internet.** A test that reaches a
//! real host is non-deterministic, will flake in CI, and proves nothing
//! about this transport.
//!
//! This is deliberately a raw-socket server rather than a framework:
//! the matrix needs shapes a normal HTTP server will not produce on
//! demand — a response that stops mid-body and waits, a connection that
//! never answers at all, a `content-length` that lies, two `set-cookie`
//! lines. Each test supplies a handler that owns the socket after the
//! request has been parsed and writes whatever bytes it likes.
//!
//! Ports are always assigned by the OS (`127.0.0.1:0`, read back from
//! `local_addr`) — never hard-coded, which would collide across the
//! parallel test binaries this crate already runs.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// One request as the server saw it on the wire.
#[derive(Debug, Clone, Default)]
pub struct RecordedRequest {
    pub method: String,
    /// The request-target exactly as written on the request line.
    pub target: String,
    /// Header names lowercased, values verbatim, in wire order and with
    /// duplicates preserved.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn header_names(&self) -> Vec<&str> {
        self.headers.iter().map(|(k, _)| k.as_str()).collect()
    }
}

/// A running loopback server. Dropping it aborts the accept loop.
pub struct LoopbackServer {
    addr: SocketAddr,
    requests: Arc<Mutex<Vec<RecordedRequest>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for LoopbackServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl LoopbackServer {
    /// Bind `127.0.0.1:0` and serve every connection with `handler`,
    /// which receives the parsed request and the still-open socket.
    pub async fn spawn<F, Fut>(handler: F) -> Self
    where
        F: Fn(RecordedRequest, TcpStream) -> Fut + Send + Sync + 'static,
        Fut: std::future::Future<Output = ()> + Send + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("read assigned local port");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let recorded = requests.clone();
        let handler = Arc::new(handler);
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut stream, _peer)) = listener.accept().await else {
                    return;
                };
                let recorded = recorded.clone();
                let handler = handler.clone();
                tokio::spawn(async move {
                    let Some(request) = read_request(&mut stream).await else {
                        return;
                    };
                    recorded
                        .lock()
                        .expect("request log mutex")
                        .push(request.clone());
                    handler(request, stream).await;
                });
            }
        });
        Self {
            addr,
            requests,
            task,
        }
    }

    /// Convenience: serve a fixed raw response to every request and
    /// close the connection.
    pub async fn spawn_static(response: impl Into<Vec<u8>>) -> Self {
        let response: Arc<Vec<u8>> = Arc::new(response.into());
        Self::spawn(move |_req, mut stream| {
            let response = response.clone();
            async move {
                let _ = stream.write_all(&response).await;
                let _ = stream.shutdown().await;
            }
        })
        .await
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// `http://127.0.0.1:<assigned port>` — no trailing slash.
    pub fn base_url(&self) -> String {
        format!("http://{}", self.addr)
    }

    /// `http://127.0.0.1:<assigned port><path>`.
    pub fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url(), path)
    }

    /// Snapshot of every request served so far, in arrival order.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        self.requests.lock().expect("request log mutex").clone()
    }

    pub fn request_count(&self) -> usize {
        self.requests.lock().expect("request log mutex").len()
    }
}

/// Bind and immediately release a loopback port, returning a URL that
/// is guaranteed to have nothing listening on it.
///
/// Used for the connection-failure case. Asking the OS for the port
/// (rather than hard-coding one) keeps it collision-free.
pub async fn closed_port_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind loopback listener");
    let addr = listener.local_addr().expect("read assigned local port");
    drop(listener);
    format!("http://{addr}/")
}

/// Minimal HTTP/1.1 request reader: headers to the blank line, then
/// exactly `content-length` body bytes. Returns `None` if the peer went
/// away before a complete head arrived.
async fn read_request(stream: &mut TcpStream) -> Option<RecordedRequest> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(pos) = find_double_crlf(&buf) {
            break pos;
        }
        let mut chunk = [0u8; 1024];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    };

    let head = String::from_utf8_lossy(&buf[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let request_line = lines.next()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
    }

    let content_length: usize = headers
        .iter()
        .find(|(k, _)| k == "content-length")
        .and_then(|(_, v)| v.parse().ok())
        .unwrap_or(0);

    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < content_length {
        let mut chunk = [0u8; 4096];
        let n = stream.read(&mut chunk).await.ok()?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    body.truncate(content_length);

    Some(RecordedRequest {
        method,
        target,
        headers,
        body,
    })
}

fn find_double_crlf(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

/// Build a complete `200 OK` response with an explicit `content-length`
/// and `connection: close`.
pub fn ok_response(body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

/// Build a redirect response with `status`, a `location`, and an empty
/// body.
pub fn redirect_response(status: u16, reason: &str, location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status} {reason}\r\nlocation: {location}\r\ncontent-length: 0\r\nconnection: close\r\n\r\n"
    )
    .into_bytes()
}
