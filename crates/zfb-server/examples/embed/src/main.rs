//! Minimal embed-as-library example for `zfb-server` (issue #372).
//!
//! Demonstrates, in well under 100 LOC:
//!
//! 1. Building a [`zfb_server::Server`] via the public builder.
//! 2. Binding to an ephemeral port (`127.0.0.1:0`).
//! 3. Injecting a per-process value into every request's extensions
//!    map via [`zfb_server::ServerBuilder::with_request_extension`].
//! 4. Registering a Rust HTTP handler via
//!    [`zfb_server::ServerBuilder::with_ssr_handler`] that reads the
//!    captured route param and the injected extension.
//! 5. Hitting the running server with a single HTTP request, printing
//!    the response body, then shutting the server down.
//!
//! Build / run from the workspace root:
//!
//! ```text
//! cargo run --manifest-path crates/zfb-server/examples/embed/Cargo.toml
//! ```

use std::path::PathBuf;
use std::time::Duration;

use anyhow::Result;
use axum::body::Body;
use axum::http::{Request, StatusCode};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use zfb_server::{RouteParams, Server, ServerMode};

#[derive(Clone, Debug)]
struct HostCtx {
    greeting: &'static str,
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let project = make_throwaway_project()?;
    let config_path = project.join("zfb.config.json");

    let server = Server::builder()
        .config_path(&config_path)
        .mode(ServerMode::Embed)
        .bind("127.0.0.1:0".parse()?)
        .with_request_extension(HostCtx { greeting: "hello" })
        .with_ssr_handler(
            "/greet/:name",
            |req: Request<Body>, params: RouteParams| async move {
                let ctx = req.extensions().get::<HostCtx>().cloned();
                let name = params.get("name").unwrap_or("world");
                let body = match ctx {
                    Some(c) => format!("{}, {name}!", c.greeting),
                    None => format!("(no ctx), {name}!"),
                };
                (StatusCode::OK, body)
            },
        )
        .build()?;

    let handle = server.serve_in_thread()?;
    let addr = handle.addr();
    println!("zfb-server embed example listening on http://{addr}");

    // Issue one GET so the demo prints proof the handler answered,
    // then shut the server down cleanly. A tiny inline HTTP/1.1 client
    // keeps the dep graph minimal.
    let body = http_get(addr, "/greet/alice").await?;
    println!("GET /greet/alice -> {body}");

    handle.shutdown()?;
    handle.join().map_err(|e| anyhow::anyhow!("join: {e}"))??;
    Ok(())
}

/// Minimal HTTP/1.1 GET — connects, sends one request, returns the
/// body. Avoids pulling in `reqwest`/`hyper` just for the example.
async fn http_get(addr: std::net::SocketAddr, path: &str) -> Result<String> {
    let mut stream =
        tokio::time::timeout(Duration::from_secs(2), TcpStream::connect(addr)).await??;
    let req = format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n");
    stream.write_all(req.as_bytes()).await?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).await?;
    let body = buf
        .split("\r\n\r\n")
        .nth(1)
        .unwrap_or(&buf)
        .trim()
        .to_string();
    Ok(body)
}

/// Carve out a throwaway project tree with the minimum files the
/// builder requires. The OS reclaims it on the next reboot — fine for
/// a one-shot demo.
fn make_throwaway_project() -> Result<PathBuf> {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let pid = std::process::id();
    let dir = std::env::temp_dir().join(format!("zfb-embed-example-{pid}-{stamp}"));
    std::fs::create_dir_all(dir.join("dist").join("assets"))?;
    std::fs::create_dir_all(dir.join("public"))?;
    std::fs::write(dir.join("zfb.config.json"), r#"{}"#)?;
    Ok(dir)
}
