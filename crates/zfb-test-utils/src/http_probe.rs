use anyhow::Context;
use std::time::Duration;

/// Distinguish a broken received body from a connection that is not ready yet.
#[derive(Debug)]
pub struct HttpBodyReadFailure;

impl std::fmt::Display for HttpBodyReadFailure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("HTTP response body could not be read completely")
    }
}

/// Read headers AND body within the caller's remaining phase budget.
/// A client timeout alone restarts for each probe and can overrun that budget.
pub async fn http_get_before(
    client: &reqwest::Client,
    url: &str,
    deadline: tokio::time::Instant,
) -> anyhow::Result<(u16, String)> {
    anyhow::ensure!(
        tokio::time::Instant::now() < deadline,
        "HTTP phase budget exhausted: {url}"
    );
    let response = tokio::time::timeout_at(deadline, client.get(url).send())
        .await
        .map_err(|_| anyhow::anyhow!("HTTP headers exceeded phase budget: {url}"))??;
    let status = response.status().as_u16();
    let body = tokio::time::timeout_at(deadline, response.text())
        .await
        .map_err(|_| anyhow::anyhow!("HTTP body exceeded phase budget: {url}"))
        .and_then(|body| body.map_err(anyhow::Error::from))
        .context(HttpBodyReadFailure)?;
    anyhow::ensure!(
        tokio::time::Instant::now() < deadline,
        "HTTP response arrived after phase budget: {url}"
    );
    Ok((status, body))
}

/// Polling cadence never extends an absolute phase budget.
pub async fn poll_before(deadline: tokio::time::Instant, interval: Duration) {
    tokio::time::sleep_until(deadline.min(tokio::time::Instant::now() + interval)).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    async fn stalled_response(headers: &'static [u8]) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            // Only require proof that the request reached this socket.
            let mut request_byte = [0; 1];
            socket.read_exact(&mut request_byte).await.unwrap();
            socket.write_all(headers).await.unwrap();
            std::future::pending::<()>().await;
        });
        let result = http_get_before(
            &reqwest::Client::new(),
            &url,
            tokio::time::Instant::now() + Duration::from_millis(50),
        )
        .await;
        server.abort();
        let _ = server.await;
        assert!(format!("{:#}", result.unwrap_err()).contains("phase budget"));
    }

    #[tokio::test]
    async fn stalled_headers_fail_within_phase_budget() {
        stalled_response(b"").await;
    }

    #[tokio::test]
    async fn stalled_body_fails_within_phase_budget() {
        stalled_response(b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nx").await;
    }

    #[tokio::test]
    async fn truncated_body_is_distinct_from_connection_not_ready() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            // Only require proof that the request reached this socket.
            let mut request_byte = [0; 1];
            socket.read_exact(&mut request_byte).await.unwrap();
            socket
                .write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 100\r\n\r\nx")
                .await
                .unwrap();
        });
        let result = http_get_before(
            &reqwest::Client::new(),
            &url,
            tokio::time::Instant::now() + Duration::from_secs(2),
        )
        .await;
        server.await.unwrap();
        let error = result.unwrap_err();
        assert!(
            error.downcast_ref::<HttpBodyReadFailure>().is_some(),
            "{error:#}"
        );
    }

    #[tokio::test]
    async fn expired_budget_never_starts_a_request() {
        assert!(http_get_before(
            &reqwest::Client::new(),
            "http://127.0.0.1:1/",
            tokio::time::Instant::now()
        )
        .await
        .unwrap_err()
        .to_string()
        .contains("exhausted"));
    }
}
