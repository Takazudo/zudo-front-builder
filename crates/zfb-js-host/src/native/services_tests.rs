//! Native temporal tests reuse the baseline's independent TCP fixture.
use std::rc::Rc;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use super::NativeHost;
use crate::crypto::{EntropySource, OsEntropy, OS_CSPRNG_KIND, OS_ENTROPY_CALLS};
use crate::dispatch_mode::DispatchMode;
use crate::fetch::FetchConfig;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::Semaphore;

#[allow(dead_code)]
#[path = "../../../zfb-render/src/embedded_v8/loopback_test_server.rs"]
mod loopback;
use loopback::{ok_response, redirect_response, LoopbackServer};

// Hang guard only; no assertion depends on elapsed wall-clock time.
const BOUND: Duration = Duration::from_secs(30);
async fn run(host: &mut NativeHost, source: &str) -> serde_json::Value {
    tokio::time::timeout(
        BOUND,
        host.evaluate_script(source, DispatchMode::RequestTime),
    )
    .await
    .expect("native dispatch progresses")
    .expect("native dispatch succeeds")
}

#[tokio::test(flavor = "current_thread")]
async fn foreground_wakes_complete_async_wasm_compilation() {
    let mut host = NativeHost::new_services().unwrap();
    assert_eq!(run(&mut host, "(async()=>{const bytes=new Uint8Array([0,97,115,109,1,0,0,0,1,5,1,96,0,1,127,3,2,1,0,7,10,1,6,97,110,115,119,101,114,0,0,10,6,1,4,0,65,42,11]);const m=await WebAssembly.compile(bytes);return new WebAssembly.Instance(m).exports.answer();})()").await, 42);
}

#[tokio::test(flavor = "current_thread")]
async fn immediate_response_abort_closes_mid_body_socket_while_host_stays_alive() {
    let started = Arc::new(Semaphore::new(0));
    let closed = Arc::new(Semaphore::new(0));
    let closed_server = closed.clone();
    let server = LoopbackServer::spawn(move |req, mut stream| {
        let started = started.clone();
        let closed = closed_server.clone();
        async move {
            match req.target.as_str() {
                "/slow" => {
                    stream
                        .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 64\r\n\r\n01234567")
                        .await
                        .unwrap();
                    started.add_permits(1);
                    let mut buf = [0; 64];
                    loop {
                        match stream.read(&mut buf).await {
                            Ok(0) | Err(_) => {
                                closed.add_permits(1);
                                break;
                            }
                            Ok(_) => {}
                        }
                    }
                }
                "/gate" => {
                    started.acquire().await.unwrap().forget();
                    stream.write_all(&ok_response("go")).await.unwrap();
                }
                _ => panic!("unexpected path"),
            }
        }
    })
    .await;
    let mut host = NativeHost::new_services().unwrap();
    let source = format!("(async()=>{{const c=new AbortController();globalThis.abortResult='pending';fetch({:?},{{signal:c.signal}}).catch(e=>globalThis.abortResult=e.message);await fetch({:?});c.abort(new Error('caller-abort-wins'));return 'returned-immediately';}})()", server.url("/slow"),server.url("/gate"));
    assert_eq!(run(&mut host, &source).await, "returned-immediately");
    assert_eq!(host.pending_fetches(), 0);
    tokio::time::timeout(BOUND, closed.acquire())
        .await
        .expect("socket closes while live host is idle")
        .unwrap()
        .forget();
    // No trailing JS/native operation was used to make the connection close.
    assert_eq!(
        run(&mut host, "globalThis.abortResult").await,
        "caller-abort-wins"
    );
    drop(host);
}

#[tokio::test(flavor = "current_thread")]
async fn orphan_redirects_keep_a_budget_while_b_gets_a_fresh_budget() {
    let release = Arc::new(Semaphore::new(0));
    let server = LoopbackServer::spawn(move |req, mut stream| {
        let release = release.clone();
        async move {
            let response = match req.target.as_str() {
                "/orphan" => {
                    release.acquire().await.unwrap().forget();
                    redirect_response(302, "Found", "/hop")
                }
                "/hop" => redirect_response(302, "Found", "/forbidden-third-hop"),
                "/b-one" => {
                    release.add_permits(1);
                    ok_response("one")
                }
                "/b-two" => ok_response("two"),
                _ => panic!("A must fail before its third socket"),
            };
            stream.write_all(&response).await.unwrap();
        }
    })
    .await;
    let config = FetchConfig {
        max_subrequests: 2,
        ..FetchConfig::default()
    };
    let mut host = NativeHost::new_services_with(config, Rc::new(OsEntropy)).unwrap();
    let a=format!("(()=>{{globalThis.orphan=fetch({:?}).then(()=> 'unexpected',e=>e.message);return 'A';}})()",server.url("/orphan"));
    assert_eq!(run(&mut host, &a).await, "A");
    assert_eq!(
        host.pending_fetches(),
        1,
        "A returns with owned native work retained"
    );
    let b=format!("(async()=>{{const values=await Promise.all([fetch({:?}).then(r=>r.text()),fetch({:?}).then(r=>r.text())]);return {{values,orphan:await globalThis.orphan}};}})()",server.url("/b-one"),server.url("/b-two"));
    let result = run(&mut host, &b).await;
    assert_eq!(result["values"], serde_json::json!(["one", "two"]));
    assert!(
        result["orphan"].as_str().unwrap().contains("subrequest"),
        "{result}"
    );
    assert_eq!(server.request_count(), 4);
    assert_eq!(host.pending_fetches(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn preabort_and_raw_mode_denial_open_no_socket() {
    let server = LoopbackServer::spawn_static(ok_response("unexpected")).await;
    let mut host = NativeHost::new_services().unwrap();
    let pre=format!("(async()=>{{const c=new AbortController();c.abort();try{{await fetch({:?},{{signal:c.signal}});return 'bad';}}catch(e){{return e.name;}}}})()",server.url("/preabort"));
    assert_eq!(run(&mut host, &pre).await, "AbortError");
    let raw=format!("(async()=>{{try{{await __zfbHost.fetch({{url:{:?},method:'GET',headers:[],hasBody:false}},new Uint8Array());return 'bad';}}catch(e){{return e.message;}}}})()",server.url("/raw"));
    let denied = host
        .evaluate_script(&raw, DispatchMode::BuildTime)
        .await
        .unwrap();
    assert!(denied.as_str().unwrap().contains("build-time"), "{denied}");
    assert_eq!(server.request_count(), 0);
    assert_eq!(host.pending_fetches(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn typed_body_is_copied_at_entry_and_binary_response_keeps_duplicates() {
    let server=LoopbackServer::spawn_static(b"HTTP/1.1 200 OK\r\ncontent-length: 3\r\nx-test: one\r\nx-test: two\r\nconnection: close\r\n\r\n\x00\x80\xff".to_vec()).await;
    let mut host = NativeHost::new_services().unwrap();
    let source=format!("(async()=>{{const bytes=new Uint8Array([99,0,128,255,99]);const p=__zfbHost.fetch({{url:{:?},method:'POST',headers:[],hasBody:true}},bytes.subarray(1,4));bytes.fill(7);const r=await p;return {{bytes:Array.from(r.body),headers:r.headers}};}})()",server.url("/bytes"));
    let result = run(&mut host, &source).await;
    assert_eq!(result["bytes"], serde_json::json!([0, 128, 255]));
    let headers = result["headers"].as_array().unwrap();
    assert!(headers.contains(&serde_json::json!(["x-test", "one"])));
    assert!(headers.contains(&serde_json::json!(["x-test", "two"])));
    assert_eq!(server.requests()[0].body, vec![0, 128, 255]);
}

#[tokio::test(flavor = "current_thread")]
async fn small_caps_and_host_timeout_preserve_native_errors() {
    let closed = Arc::new(Semaphore::new(0));
    let closed_server = closed.clone();
    let server = LoopbackServer::spawn(move |req, mut stream| {
        let closed = closed_server.clone();
        async move {
            if req.target == "/slow" {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 9\r\n\r\nx")
                    .await
                    .unwrap();
                let mut byte = [0];
                let _ = stream.read(&mut byte).await;
                closed.add_permits(1);
            } else {
                stream.write_all(&ok_response("12345")).await.unwrap();
            }
        }
    })
    .await;
    let config = FetchConfig {
        timeout_ms: 200,
        max_request_body_bytes: 4,
        max_response_body_bytes: 4,
        ..FetchConfig::default()
    };
    let mut host = NativeHost::new_services_with(config, Rc::new(OsEntropy)).unwrap();
    let request=format!("(async()=>{{try{{await fetch({:?},{{method:'POST',body:'12345'}});return 'bad';}}catch(e){{return e.name+'|'+e.message;}}}})()",server.url("/request"));
    assert!(run(&mut host, &request)
        .await
        .as_str()
        .unwrap()
        .contains("4"));
    assert_eq!(server.request_count(), 0);
    let response=format!("(async()=>{{try{{await fetch({:?});return 'bad';}}catch(e){{return e.name+'|'+e.message;}}}})()",server.url("/response"));
    assert!(run(&mut host, &response)
        .await
        .as_str()
        .unwrap()
        .contains("4"));
    // Lift the response cap to isolate the deadline from the advertised size.
    host.services
        .as_ref()
        .unwrap()
        .borrow_mut()
        .config
        .max_response_body_bytes = 64;
    let timeout=format!("(async()=>{{try{{await fetch({:?},{{signal:AbortSignal.timeout(10000)}});return 'bad';}}catch(e){{return e.name+'|'+e.message;}}}})()",server.url("/slow"));
    let result = run(&mut host, &timeout).await;
    assert!(
        result.as_str().unwrap().starts_with("TimeoutError|"),
        "{result}"
    );
    assert!(result.as_str().unwrap().contains("200ms"), "{result}");
    tokio::time::timeout(BOUND, closed.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert_eq!(host.pending_fetches(), 0);
}

struct FixedEntropy;
impl EntropySource for FixedEntropy {
    fn kind(&self) -> &'static str {
        "test-fixed"
    }
    fn fill(&self, bytes: &mut [u8]) -> Result<(), String> {
        bytes.fill(42);
        Ok(())
    }
}
struct FailedEntropy;
impl EntropySource for FailedEntropy {
    fn kind(&self) -> &'static str {
        "test-failure"
    }
    fn fill(&self, bytes: &mut [u8]) -> Result<(), String> {
        bytes.fill(42);
        Err("entropy-test-failure".into())
    }
}
#[tokio::test(flavor = "current_thread")]
async fn entropy_is_os_backed_and_buffers_are_bounded_and_fail_closed() {
    let mut host = NativeHost::new_services().unwrap();
    assert_eq!(
        host.services.as_ref().unwrap().borrow().entropy.kind(),
        OS_CSPRNG_KIND
    );
    let before = OS_ENTROPY_CALLS.load(Ordering::Relaxed);
    assert_eq!(
        run(
            &mut host,
            "(()=>{const b=new Uint8Array(4);return crypto.getRandomValues(b)===b;})()"
        )
        .await,
        true
    );
    assert!(OS_ENTROPY_CALLS.load(Ordering::Relaxed) > before);
    drop(host);
    let mut host =
        NativeHost::new_services_with(FetchConfig::default(), Rc::new(FixedEntropy)).unwrap();
    assert_eq!(run(&mut host,"(()=>{const b=new Uint8Array([99,0,0,99]);crypto.getRandomValues(b.subarray(1,3));return Array.from(b);})()").await,serde_json::json!([99,42,42,99]));
    drop(host);
    let mut host =
        NativeHost::new_services_with(FetchConfig::default(), Rc::new(FailedEntropy)).unwrap();
    assert_eq!(run(&mut host,"(()=>{const b=new Uint8Array([9,9,9]);let error;try{crypto.getRandomValues(b.subarray(1,2));}catch(e){error=e.message;}const z=new Uint8Array();return {b:Array.from(b),error,zero:crypto.getRandomValues(z)===z};})()").await,serde_json::json!({"b":[9,9,9],"error":"crypto.getRandomValues: OS entropy unavailable: entropy-test-failure","zero":true}));
}

#[tokio::test(flavor = "current_thread")]
async fn repeated_pending_wasm_disposal_allows_fresh_foreground_work() {
    // The exact upstream custom-platform API identifies an isolate by address,
    // matching the baseline. Exercise the reachable Wasm task cancellation path
    // across repeated disposal/recreation, without claiming arbitrary late tasks.
    for iteration in 0..32 {
        let mut host = NativeHost::new_services().unwrap();
        let source=format!("(()=>{{const b=new Uint8Array(65549);b.set([0,97,115,109,1,0,0,0,0,129,128,4,0]);b[b.length-1]={iteration};WebAssembly.compile(b).catch(()=>{{}});return 'orphaned';}})()");
        assert_eq!(run(&mut host, &source).await, "orphaned");
        drop(host);
    }
    let mut host = NativeHost::new_services().unwrap();
    assert_eq!(run(&mut host,"(async()=>{const module=await WebAssembly.compile(new Uint8Array([0,97,115,109,1,0,0,0]));return module instanceof WebAssembly.Module;})()").await,true);
}

#[tokio::test(flavor = "current_thread")]
async fn eager_cancellation_before_first_transport_poll_and_repeated_abort_are_safe() {
    let server = LoopbackServer::spawn_static(ok_response("unexpected")).await;
    let mut host = NativeHost::new_services().unwrap();
    let source=format!("(async()=>{{const c=new AbortController();const p=fetch({:?},{{signal:c.signal}});c.abort();c.abort();try{{await p;return 'bad';}}catch(e){{return e.name;}}}})()",server.url("/abort-before-poll"));
    assert_eq!(run(&mut host, &source).await, "AbortError");
    assert_eq!(server.request_count(), 0);
    assert_eq!(host.pending_fetches(), 0);
}

#[tokio::test(flavor = "current_thread")]
async fn timeout_and_settled_response_win_over_late_cancel() {
    let closed = Arc::new(Semaphore::new(0));
    let closed_server = closed.clone();
    let server = LoopbackServer::spawn(move |req, mut stream| {
        let closed = closed_server.clone();
        async move {
            if req.target == "/slow" {
                stream
                    .write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 64\r\n\r\nx")
                    .await
                    .unwrap();
                let mut byte = [0];
                let _ = stream.read(&mut byte).await;
                closed.add_permits(1);
            } else {
                stream.write_all(&ok_response("settled")).await.unwrap();
            }
        }
    })
    .await;
    let mut host = NativeHost::new_services_with(
        FetchConfig {
            timeout_ms: 200,
            ..FetchConfig::default()
        },
        Rc::new(OsEntropy),
    )
    .unwrap();
    let source=format!("(async()=>{{const c=new AbortController();const p=fetch({:?},{{signal:c.signal}});let first;try{{await p;}}catch(e){{first=e;}}c.abort(new Error('too-late'));try{{await p;return 'bad';}}catch(e){{return {{same:e===first,name:e.name,message:e.message}};}}}})()",server.url("/slow"));
    let result = run(&mut host, &source).await;
    assert_eq!(result["same"], true);
    assert_eq!(result["name"], "TimeoutError");
    assert!(result["message"].as_str().unwrap().contains("200ms"));
    tokio::time::timeout(BOUND, closed.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let signal=format!("(async()=>{{try{{await fetch({:?},{{signal:AbortSignal.timeout(20)}});return 'bad';}}catch(e){{return e.name+'|'+e.message;}}}})()",server.url("/slow"));
    assert_eq!(
        run(&mut host, &signal).await,
        "TimeoutError|The operation was aborted due to timeout."
    );
    tokio::time::timeout(BOUND, closed.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    let settled=format!("(async()=>{{const p=__zfbHost.fetch({{url:{:?},method:'GET',headers:[],hasBody:false,cancelId:700}},new Uint8Array());const first=await p;__zfbHost.cancelFetch(700);__zfbHost.cancelFetch(700);__zfbHost.cancelFetch(999999);return {{same:(await p)===first,body:new TextDecoder().decode(first.body)}};}})()",server.url("/settled"));
    assert_eq!(
        run(&mut host, &settled).await,
        serde_json::json!({"same":true,"body":"settled"})
    );
    assert_eq!(host.pending_fetches(), 0);
    assert_eq!(server.request_count(), 3);
}
