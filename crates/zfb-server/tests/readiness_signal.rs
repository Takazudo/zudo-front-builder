//! Integration coverage for the Dev publication readiness surface (issue
//! #2556).
//!
//! The harness uses the public `ServeOpts`/`serve_with_listener` API so the
//! assertions cover real route mounting, response shaping, base prefixes, and
//! mode gates rather than only calling private helpers.

use std::net::SocketAddr;
use std::sync::{Arc, RwLock};

use async_trait::async_trait;
use tempfile::TempDir;
use tokio::net::TcpListener;
use tokio::sync::{broadcast, Notify};
use tokio::time::{timeout, Duration};
use zfb_server::livereload::ReloadEvent;
use zfb_server::{
    serve_with_listener, DevPublicationState, PageCache, RenderOnRequestHandle,
    RenderOnRequestHook, ServeOpts,
};

struct Harness {
    addr: SocketAddr,
    pages: PageCache,
    server: tokio::task::JoinHandle<anyhow::Result<()>>,
    _tmp: TempDir,
}

impl Harness {
    async fn start(
        mode: zfb_server::ServerMode,
        base: Option<&str>,
        publication: Option<zfb_server::IslandsBundleUrl>,
    ) -> Self {
        Self::start_with_hook(mode, base, publication, None).await
    }

    async fn start_with_hook(
        mode: zfb_server::ServerMode,
        base: Option<&str>,
        publication: Option<zfb_server::IslandsBundleUrl>,
        render_on_request_hook: Option<RenderOnRequestHandle>,
    ) -> Self {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().to_path_buf();
        let dist_root = root.join("dist");
        let public_root = root.join("public");
        std::fs::create_dir_all(dist_root.join("assets")).expect("dist assets");
        std::fs::create_dir_all(&public_root).expect("public root");

        let pages = PageCache::new();
        let (tx, _rx) = broadcast::channel::<ReloadEvent>(16);
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))
            .await
            .expect("bind ephemeral");
        let addr = listener.local_addr().expect("local_addr");

        let opts = ServeOpts {
            project_root: root,
            dist_root: dist_root.clone(),
            dev_assets_root: None,
            html_root: dist_root,
            public_root,
            addr,
            pages: pages.clone(),
            broadcast: tx,
            plugins: None,
            injected_routes: None,
            ssr_routes: None,
            base: base.map(str::to_owned),
            trailing_slash: false,
            mode,
            islands_bundle_url: publication,
            css_bundle_url: None,
            allowed_hosts: Vec::new(),
            bound_host: None,
            render_on_request_hook,
            redirects: None,
        };
        let server = tokio::spawn(async move {
            serve_with_listener(opts, listener, std::future::pending::<()>()).await
        });
        tokio::task::yield_now().await;

        Self {
            addr,
            pages,
            server,
            _tmp: tmp,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("http://{}{}", self.addr, path)
    }
}

struct BlockingRenderHook {
    entered: Arc<Notify>,
    release: Arc<Notify>,
}

#[async_trait]
impl RenderOnRequestHook for BlockingRenderHook {
    async fn render_if_stale(&self, _url_path: &str) {
        self.entered.notify_one();
        self.release.notified().await;
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}

fn publication_handle() -> zfb_server::IslandsBundleUrl {
    Arc::new(RwLock::new(DevPublicationState::pending()))
}

fn publish_all(handle: &zfb_server::IslandsBundleUrl, prefix: &str) {
    let mut state = handle.write().expect("publication write lock");
    state.publish_islands(vec![format!("{prefix}/assets/islands.js")]);
    state.publish_client_scripts(vec![format!("{prefix}/assets/client/main.js")]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_headers_and_ready_endpoint_follow_pending_to_published_state() {
    let handle = publication_handle();
    let h = Harness::start(zfb_server::ServerMode::Dev, None, Some(Arc::clone(&handle))).await;
    h.pages
        .insert(
            "/",
            "<!doctype html><html><head></head><body>home</body></html>",
        )
        .await;
    h.pages.insert("/feed.xml", "<feed />").await;

    let pending_page = reqwest::get(h.url("/")).await.expect("pending page");
    assert_eq!(pending_page.status(), 200);
    assert_eq!(
        pending_page
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(
        pending_page
            .headers()
            .get("x-zfb-dev-generation")
            .and_then(|v| v.to_str().ok()),
        Some("0")
    );
    assert_eq!(
        pending_page
            .headers()
            .get("x-zfb-dev-ready")
            .and_then(|v| v.to_str().ok()),
        Some("false")
    );
    assert_eq!(
        pending_page
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );

    let pending_ready = reqwest::get(h.url("/__zfb/ready"))
        .await
        .expect("pending ready");
    assert_eq!(pending_ready.status(), 200);
    assert_eq!(
        pending_ready
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(
        pending_ready
            .headers()
            .get("cache-control")
            .and_then(|v| v.to_str().ok()),
        Some("no-store")
    );
    let pending_json: serde_json::Value =
        serde_json::from_str(&pending_ready.text().await.expect("pending JSON body"))
            .expect("pending JSON");
    assert_eq!(pending_json["generation"], 0);
    assert_eq!(pending_json["ready"], false);
    assert_eq!(pending_json["islands"]["status"], "pending");
    assert_eq!(pending_json["client_scripts"]["status"], "pending");
    assert!(pending_json["exclusions"]["dist_root_boot_lazy_seed"]
        .as_str()
        .is_some_and(|text| text.contains("boot-lazy seed")));
    assert!(
        pending_json["exclusions"]["stale_dev_pages_html_across_restart"]
            .as_str()
            .is_some_and(|text| text.contains("stale .zfb-build/dev-pages"))
    );
    assert!(
        pending_json["exclusions"]["public_html_and_user_authored_scripts"]
            .as_str()
            .is_some_and(|text| text.contains("public/*.html") && text.contains("user-authored"))
    );
    assert!(
        pending_json["exclusions"]["deferred_islands_companions_and_chunks"]
            .as_str()
            .is_some_and(|text| text.contains("islands-chunk-*"))
    );

    let xml = reqwest::get(h.url("/feed.xml")).await.expect("xml page");
    assert_eq!(xml.status(), 200);
    assert_eq!(
        xml.headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok()),
        Some("application/xml")
    );
    assert!(xml.headers().get("x-zfb-dev-generation").is_none());
    assert!(xml.headers().get("x-zfb-dev-ready").is_none());

    {
        let mut state = handle.write().expect("publication write lock");
        state.publish_islands(vec!["/assets/islands.js".to_string()]);
    }
    let islands_only_page = reqwest::get(h.url("/")).await.expect("islands-only page");
    assert_eq!(
        islands_only_page
            .headers()
            .get("x-zfb-dev-generation")
            .and_then(|v| v.to_str().ok()),
        Some("1")
    );
    assert_eq!(
        islands_only_page
            .headers()
            .get("x-zfb-dev-ready")
            .and_then(|v| v.to_str().ok()),
        Some("false")
    );
    let islands_only_body = islands_only_page
        .text()
        .await
        .expect("islands-only page body");
    assert!(islands_only_body.contains("src=\"/assets/islands.js\""));

    {
        let mut state = handle.write().expect("publication write lock");
        state.publish_client_scripts(vec!["/assets/client/main.js".to_string()]);
    }
    let published_page = reqwest::get(h.url("/")).await.expect("published page");
    assert_eq!(
        published_page
            .headers()
            .get("x-zfb-dev-generation")
            .and_then(|v| v.to_str().ok()),
        Some("2")
    );
    assert_eq!(
        published_page
            .headers()
            .get("x-zfb-dev-ready")
            .and_then(|v| v.to_str().ok()),
        Some("true")
    );
    let published_body = published_page.text().await.expect("published body");
    assert!(published_body.contains("src=\"/assets/islands.js\""));

    let published_ready = reqwest::get(h.url("/__zfb/ready"))
        .await
        .expect("published ready");
    let published_json: serde_json::Value =
        serde_json::from_str(&published_ready.text().await.expect("published JSON body"))
            .expect("published JSON");
    assert_eq!(published_json["generation"], 2);
    assert_eq!(published_json["ready"], true);
    assert_eq!(published_json["islands"]["status"], "published");
    assert_eq!(
        published_json["islands"]["urls"],
        serde_json::json!(["/assets/islands.js"])
    );
    assert_eq!(published_json["client_scripts"]["status"], "published");
    assert_eq!(
        published_json["client_scripts"]["urls"],
        serde_json::json!(["/assets/client/main.js"])
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dev_without_publication_handle_reports_initial_pending_signal() {
    let h = Harness::start(zfb_server::ServerMode::Dev, None, None).await;
    h.pages
        .insert("/", "<html><head></head><body>legacy</body></html>")
        .await;

    let page = reqwest::get(h.url("/")).await.expect("legacy page");
    assert_eq!(
        page.headers()
            .get("x-zfb-dev-generation")
            .and_then(|v| v.to_str().ok()),
        Some("0")
    );
    assert_eq!(
        page.headers()
            .get("x-zfb-dev-ready")
            .and_then(|v| v.to_str().ok()),
        Some("false")
    );

    let ready = reqwest::get(h.url("/__zfb/ready"))
        .await
        .expect("legacy ready");
    assert_eq!(ready.status(), 200);
    let json: serde_json::Value =
        serde_json::from_str(&ready.text().await.expect("legacy ready JSON body"))
            .expect("legacy ready JSON");
    assert_eq!(json["generation"], 0);
    assert_eq!(json["ready"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_snapshot_is_taken_after_render_hook_unblocks() {
    let handle = publication_handle();
    let entered = Arc::new(Notify::new());
    let release = Arc::new(Notify::new());
    let hook: Arc<dyn RenderOnRequestHook> = Arc::new(BlockingRenderHook {
        entered: Arc::clone(&entered),
        release: Arc::clone(&release),
    });
    let hook_handle: RenderOnRequestHandle = Arc::new(RwLock::new(Some(hook)));
    let h = Harness::start_with_hook(
        zfb_server::ServerMode::Dev,
        None,
        Some(Arc::clone(&handle)),
        Some(hook_handle),
    )
    .await;
    h.pages
        .insert("/", "<html><head></head><body>blocked</body></html>")
        .await;

    let response_task = tokio::spawn({
        let url = h.url("/");
        async move { reqwest::get(url).await.expect("blocked page") }
    });
    timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("render hook did not receive the request");

    // The request has already entered the render-on-request await while the
    // publication state is still generation 0. Publish both framework-owned
    // slots before releasing it; final response shaping must use generation 2.
    publish_all(&handle, "");
    release.notify_one();

    let page = timeout(Duration::from_secs(5), response_task)
        .await
        .expect("blocked response did not finish")
        .expect("response task");
    assert_eq!(page.status(), 200);
    assert_eq!(
        page.headers()
            .get("x-zfb-dev-generation")
            .and_then(|v| v.to_str().ok()),
        Some("2")
    );
    assert_eq!(
        page.headers()
            .get("x-zfb-dev-ready")
            .and_then(|v| v.to_str().ok()),
        Some("true")
    );
    let body = page.text().await.expect("blocked page body");
    assert!(body.contains("src=\"/assets/islands.js\""));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn islandless_and_scriptless_publications_are_ready() {
    let handle = publication_handle();
    let h = Harness::start(zfb_server::ServerMode::Dev, None, Some(Arc::clone(&handle))).await;
    h.pages
        .insert("/", "<html><head></head><body>empty</body></html>")
        .await;

    {
        let mut state = handle.write().expect("publication write lock");
        state.publish_islands(Vec::new());
        state.publish_client_scripts(Vec::new());
    }

    let page = reqwest::get(h.url("/")).await.expect("ready page");
    assert_eq!(
        page.headers()
            .get("x-zfb-dev-generation")
            .and_then(|v| v.to_str().ok()),
        Some("2")
    );
    assert_eq!(
        page.headers()
            .get("x-zfb-dev-ready")
            .and_then(|v| v.to_str().ok()),
        Some("true")
    );
    let body = page.text().await.expect("ready page body");
    assert!(!body.contains("islands.js"));

    let ready = reqwest::get(h.url("/__zfb/ready")).await.expect("ready");
    let json: serde_json::Value =
        serde_json::from_str(&ready.text().await.expect("ready JSON body")).expect("ready JSON");
    assert_eq!(json["ready"], true);
    assert_eq!(json["islands"]["status"], "not_expected");
    assert_eq!(json["client_scripts"]["status"], "not_expected");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn base_prefix_preserves_urls_and_shapes_base_mismatch_404() {
    let handle = publication_handle();
    publish_all(&handle, "/site");
    let h = Harness::start(
        zfb_server::ServerMode::Dev,
        Some("/site/"),
        Some(Arc::clone(&handle)),
    )
    .await;
    h.pages
        .insert("/", "<html><head></head><body>base</body></html>")
        .await;

    let page = reqwest::get(h.url("/site/")).await.expect("base page");
    assert_eq!(page.status(), 200);
    assert_eq!(
        page.headers()
            .get("x-zfb-dev-generation")
            .and_then(|v| v.to_str().ok()),
        Some("2")
    );
    let body = page.text().await.expect("base page body");
    assert!(body.contains("src=\"/site/assets/islands.js\""));

    let ready = reqwest::get(h.url("/site/__zfb/ready"))
        .await
        .expect("base ready");
    assert_eq!(ready.status(), 200);
    let json: serde_json::Value =
        serde_json::from_str(&ready.text().await.expect("base ready JSON body"))
            .expect("base ready JSON");
    assert_eq!(
        json["islands"]["urls"],
        serde_json::json!(["/site/assets/islands.js"])
    );
    assert_eq!(
        json["client_scripts"]["urls"],
        serde_json::json!(["/site/assets/client/main.js"])
    );

    let unprefixed_ready = reqwest::get(h.url("/__zfb/ready"))
        .await
        .expect("unprefixed ready");
    assert_eq!(unprefixed_ready.status(), 404);

    let mismatch = reqwest::get(h.url("/wrong")).await.expect("base mismatch");
    assert_eq!(mismatch.status(), 404);
    assert_eq!(
        mismatch
            .headers()
            .get("x-zfb-dev-generation")
            .and_then(|v| v.to_str().ok()),
        Some("2")
    );
    assert_eq!(
        mismatch
            .headers()
            .get("x-zfb-dev-ready")
            .and_then(|v| v.to_str().ok()),
        Some("true")
    );
    let mismatch_body = mismatch.text().await.expect("base mismatch body");
    assert!(mismatch_body.contains("/site/__zfb/livereload.js"));
    assert!(!mismatch_body.contains("/site/assets/islands.js"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn preview_and_embed_have_neither_readiness_headers_nor_endpoint() {
    for mode in [
        zfb_server::ServerMode::Preview,
        zfb_server::ServerMode::Embed,
    ] {
        let handle = publication_handle();
        publish_all(&handle, "");
        let h = Harness::start(mode, None, Some(handle)).await;
        h.pages
            .insert("/", "<html><head></head><body>production</body></html>")
            .await;

        let page = reqwest::get(h.url("/")).await.expect("production page");
        assert_eq!(page.status(), 200);
        assert!(page.headers().get("x-zfb-dev-generation").is_none());
        assert!(page.headers().get("x-zfb-dev-ready").is_none());

        let ready = reqwest::get(h.url("/__zfb/ready"))
            .await
            .expect("production ready");
        assert_eq!(ready.status(), 404);
        assert!(ready.headers().get("x-zfb-dev-generation").is_none());
        assert!(ready.headers().get("x-zfb-dev-ready").is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn poisoned_publication_lock_is_recovered_for_page_and_ready_routes() {
    let handle = publication_handle();
    publish_all(&handle, "");
    let h = Harness::start(zfb_server::ServerMode::Dev, None, Some(Arc::clone(&handle))).await;
    h.pages
        .insert("/", "<html><head></head><body>poison</body></html>")
        .await;

    let poison = Arc::clone(&handle);
    let join = std::thread::spawn(move || {
        let _guard = poison.write().expect("poison write lock");
        panic!("intentional publication lock poison");
    })
    .join();
    assert!(join.is_err());

    let page = reqwest::get(h.url("/")).await.expect("poisoned page");
    assert_eq!(page.status(), 200);
    assert_eq!(
        page.headers()
            .get("x-zfb-dev-ready")
            .and_then(|v| v.to_str().ok()),
        Some("true")
    );
    let ready = reqwest::get(h.url("/__zfb/ready"))
        .await
        .expect("poisoned ready");
    assert_eq!(ready.status(), 200);
    let json: serde_json::Value =
        serde_json::from_str(&ready.text().await.expect("poisoned ready JSON body"))
            .expect("poisoned ready JSON");
    assert_eq!(json["generation"], 2);
    assert_eq!(json["ready"], true);
}
