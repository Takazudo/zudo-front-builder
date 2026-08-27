use std::time::Duration;

/// Build a client for a long-lived SSE subscription.
///
/// `reqwest::ClientBuilder::timeout` bounds the entire response, including
/// reading its body. An SSE body never completes, so a total timeout silently
/// caps how long a subscription can observe. This is especially wrong for the
/// dev server, whose keep-alive interval is 15 seconds: a 10-second total
/// timeout can expire before the first keep-alive frame arrives.
///
/// The failure was hidden until the slower fixture from sub-issue #2094. Its
/// first tick restages injected-route bundles and was measured crossing 10
/// seconds, while the baseline fixture's cheap MDX warmup finished well under
/// 10 seconds. The old cap therefore looked harmless until that slower fixture
/// exercised it.
///
/// A five-second connect timeout remains intentional. Failure to establish the
/// TCP connection is a real error, and bounding connection establishment does
/// not truncate an already healthy stream.
pub fn sse_client() -> reqwest::Client {
    reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(5))
        .build()
        .expect("build SSE reqwest client")
}

/// Maximum time to wait for the server to return response headers.
///
/// This applies only to `send()`. The bound ends once headers arrive and never
/// truncates the streaming response body.
const SSE_HEADER_DEADLINE: Duration = Duration::from_secs(5);

/// Open the canonical dev-server SSE stream and return its response.
///
/// The caller owns status assertions and body-read diagnostics. A future
/// mount-prefixed subscription can express its prefix directly in `base`.
pub async fn open_sse(base: &str) -> reqwest::Response {
    let request = sse_client().get(format!("{base}/__zfb/reload")).send();
    tokio::time::timeout(SSE_HEADER_DEADLINE, request)
        .await
        .expect("timed out waiting for SSE response headers")
        .expect("open SSE stream")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::next_sse_event_name;
    use anyhow::Context;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    const KEEP_ALIVE_DELAY: Duration = Duration::from_millis(50);
    const TOTAL_TIMEOUT: Duration = Duration::from_millis(200);
    const EVENT_DELAY_AFTER_KEEP_ALIVE: Duration = Duration::from_millis(500);
    const EVENT_DEADLINE: Duration = Duration::from_secs(2);
    const CLEAN_EXPIRY_DEADLINE: Duration = Duration::from_millis(150);

    struct SseFixture {
        base: String,
        task: JoinHandle<anyhow::Result<()>>,
    }

    impl SseFixture {
        async fn spawn() -> Self {
            let listener = TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind raw SSE fixture");
            let address = listener.local_addr().expect("fixture local address");
            let task = tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.context("accept SSE client")?;
                socket.set_nodelay(true).context("set TCP_NODELAY")?;

                let mut request = Vec::new();
                let mut byte = [0_u8; 1];
                while !request.ends_with(b"\r\n\r\n") {
                    let read = socket
                        .read(&mut byte)
                        .await
                        .context("read request header")?;
                    anyhow::ensure!(read != 0, "client closed before request headers completed");
                    request.push(byte[0]);
                }

                socket
                    .write_all(
                        b"HTTP/1.1 200 OK\r\n\
                          Content-Type: text/event-stream\r\n\
                          Cache-Control: no-cache\r\n\
                          Connection: close\r\n\r\n",
                    )
                    .await
                    .context("write response headers")?;
                socket.flush().await.context("flush response headers")?;

                tokio::time::sleep(KEEP_ALIVE_DELAY).await;
                socket
                    .write_all(b": keep-alive\n\n")
                    .await
                    .context("write keep-alive")?;
                socket.flush().await.context("flush keep-alive")?;

                tokio::time::sleep(EVENT_DELAY_AFTER_KEEP_ALIVE).await;
                // Clean-expiry clients are expected to disconnect before this
                // delayed stimulus, so the final write and flush are best-effort.
                if socket.write_all(b"event: page\ndata: {}\n\n").await.is_ok() {
                    let _ = socket.flush().await;
                }
                Ok(())
            });

            Self {
                base: format!("http://{address}"),
                task,
            }
        }

        async fn finish(self) {
            match tokio::time::timeout(Duration::from_secs(1), self.task).await {
                Ok(result) => result
                    .expect("SSE fixture task panicked")
                    .expect("SSE fixture failed"),
                Err(_) => panic!("SSE fixture did not finish after its client disconnected"),
            }
        }
    }

    #[tokio::test]
    async fn open_sse_outlives_total_timeout_and_delivers_event() {
        let fixture = SseFixture::spawn().await;
        let response = open_sse(&fixture.base).await;

        let event = next_sse_event_name(response, EVENT_DEADLINE)
            .await
            .expect("read delayed SSE event");

        assert_eq!(event.as_deref(), Some("page"));
        fixture.finish().await;
    }

    #[tokio::test]
    async fn total_timeout_fails_during_streaming_body_and_adds_hint() {
        let fixture = SseFixture::spawn().await;
        let response = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(5))
            .timeout(TOTAL_TIMEOUT)
            .build()
            .expect("build negative-control client")
            .get(format!(
                "{}/{}",
                fixture.base,
                ["__zfb", "reload"].join("/")
            ))
            .send()
            .await
            .expect("negative control must receive headers before its body times out");

        let error = next_sse_event_name(response, EVENT_DEADLINE)
            .await
            .expect_err("a total timeout must fail the streaming body read");
        assert!(
            error
                .chain()
                .filter_map(|cause| cause.downcast_ref::<reqwest::Error>())
                .any(reqwest::Error::is_timeout),
            "error chain must contain the reqwest body timeout: {error:#}"
        );
        assert!(
            format!("{error:#}").contains("zfb_test_utils::open_sse()"),
            "timeout error must name the safe subscription helper: {error:#}"
        );
        fixture.finish().await;
    }

    #[tokio::test]
    async fn caller_deadline_expires_cleanly_without_transport_error() {
        let fixture = SseFixture::spawn().await;
        let response = open_sse(&fixture.base).await;

        let event = next_sse_event_name(response, CLEAN_EXPIRY_DEADLINE)
            .await
            .expect("caller deadline expiry must remain a clean result");

        assert_eq!(event, None);
        fixture.finish().await;
    }
}
