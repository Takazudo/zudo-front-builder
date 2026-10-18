use std::time::Duration;

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
    let response = tokio::time::timeout_at(deadline, async {
        let response = client.get(url).send().await?;
        let status = response.status().as_u16();
        let body = response.text().await?;
        Ok::<_, reqwest::Error>((status, body))
    })
    .await
    .map_err(|_| anyhow::anyhow!("HTTP headers/body exceeded phase budget: {url}"))??;
    anyhow::ensure!(
        tokio::time::Instant::now() < deadline,
        "HTTP response arrived after phase budget: {url}"
    );
    Ok(response)
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
            let mut request = [0; 2048];
            socket.read(&mut request).await.unwrap();
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
        assert!(result.unwrap_err().to_string().contains("phase budget"));
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
