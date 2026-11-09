//! Embedding a `.ts` config: the host supplies its own compiler worker
//! launcher (`zfb-server-test-worker`, an executable that is not the zfb
//! CLI) and the builder bundles and evaluates the config through it.
use std::time::Duration;

use zfb_server::{Server, ServerMode, WorkerLauncher, COMPILER_WORKER_ARG};

#[test]
fn ts_config_loads_through_the_embedders_compiler_worker() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    std::fs::create_dir_all(root.join("static")).unwrap();
    std::fs::write(root.join("static/hello.txt"), "hello from static").unwrap();
    std::fs::write(
        root.join("dirs.ts"),
        "export const publicDir: string = \"static\";\n",
    )
    .unwrap();
    // The CLI-only plugin is absent from this deployment: the embed path
    // skips plugin resolution, so it must not fail the load.
    std::fs::write(
        root.join("zfb.config.ts"),
        "import { defineConfig } from \"@takazudo/zfb/config\";\n\
         import { publicDir } from \"./dirs\";\n\
         export default defineConfig({ publicDir, plugins: [{ name: \"@absent/cli-only\" }] });\n",
    )
    .unwrap();

    let launcher =
        WorkerLauncher::new(env!("CARGO_BIN_EXE_zfb-server-test-worker")).arg(COMPILER_WORKER_ARG);
    let server = Server::builder()
        .config_path(root.join("zfb.config.ts"))
        .compiler_worker(launcher)
        .mode(ServerMode::Embed)
        .bind("127.0.0.1:0".parse().unwrap())
        .build()
        .expect("the .ts config loads through the supplied worker");

    let handle = server.serve_in_thread().expect("serve_in_thread");
    let addr = handle.addr();
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .worker_threads(2)
        .build()
        .unwrap();
    let (status, body) = rt.block_on(async {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(5))
            .build()
            .unwrap();
        let resp = client
            .get(format!("http://{addr}/hello.txt"))
            .send()
            .await
            .expect("GET /hello.txt");
        (
            resp.status().as_u16(),
            resp.text().await.unwrap_or_default(),
        )
    });
    handle.shutdown().expect("shutdown");
    handle.join().expect("join").expect("server exits cleanly");

    assert_eq!(status, 200, "publicDir from the .ts config is served");
    assert_eq!(body, "hello from static");
}
