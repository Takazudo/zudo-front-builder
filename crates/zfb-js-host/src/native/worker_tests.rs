//! Focused native lifecycle/identity evidence; paired policy corpus runs separately.
use super::NativeHost;
use crate::dispatch::{DispatchMode, HttpRequestLike};
use crate::modules::BundleModuleLoader;
use serde_json::json;

fn host() -> NativeHost {
    NativeHost::with_loader(BundleModuleLoader::new()).unwrap()
}
#[tokio::test(flavor = "current_thread")]
async fn static_dynamic_and_cyclic_imports_share_single_module_identity() {
    let loader = BundleModuleLoader::new();
    loader.register_module("file:///zfb/dependency.mjs","globalThis.count=(globalThis.count||0)+1; export const marker={value:42}; export const meta={url:import.meta.url,main:import.meta.main};");
    loader.register_module(
        "file:///zfb/a.mjs",
        "import {b} from './b.mjs';export const a=20;export function sum(){return a+b;}",
    );
    loader.register_module(
        "file:///zfb/b.mjs",
        "import {a} from './a.mjs';export const b=22;export const read=()=>a;",
    );
    let mut host = NativeHost::with_loader(loader).unwrap();
    let handle=host.execute_module("identity.mjs","import * as first from './dependency.mjs';import {sum} from './a.mjs';const second=await import('./dependency.mjs');const third=await import('./dependency.mjs');export const probe={same:first===second&&second===third,count:globalThis.count,value:first.marker.value,sum:sum(),url:import.meta.url,main:import.meta.main,child:first.meta};").await.unwrap();
    assert_eq!(
        host.get_export(&handle, "probe").await.unwrap(),
        json!({"same":true,"count":1,"value":42,"sum":42,"url":"file:///zfb/identity.mjs","main":true,"child":{"url":"file:///zfb/dependency.mjs","main":false}})
    );
}
#[tokio::test(flavor = "current_thread")]
async fn utility_default_and_exports_preserve_non_worker_behavior() {
    let mut host = host();
    let module=host.execute_module("utility.mjs","export const data={answer:42,missing:undefined,nan:NaN,toJSON(){return 'not-called'}};export default async function(props){await Promise.resolve();return {toString(){return 'hello '+props.name;}};}").await.unwrap();
    assert_eq!(
        host.call_default(&module, json!({"name":"native"}))
            .await
            .unwrap(),
        "hello native"
    );
    assert_eq!(
        host.get_export(&module, "data").await.unwrap(),
        json!({"answer":42,"nan":null,"toJSON":{}})
    );
    assert_eq!(
        host.get_export(&module, "missing").await.unwrap(),
        serde_json::Value::Null
    );
    let error = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .unwrap_err();
    assert!(error.to_string().contains("last install error"));
}
#[tokio::test(flavor = "current_thread")]
async fn separately_validated_inner_runs_once_and_keeps_its_own_error_frame() {
    let mut host = host();
    let inner="globalThis.boots=(globalThis.boots||0)+1;\nconsole.log('inner boot');\nexport default {fetch(req){if(req.url.endsWith('/throw'))throw new Error('inner-marker');return Response.json({boots:globalThis.boots,main:import.meta.main});}};";
    host.validate_worker_module_shape("file:///zfb/inner.mjs", inner)
        .await
        .unwrap();
    let before = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .unwrap_err();
    assert!(before.to_string().contains("before any bundle"));
    host.execute_worker_module("file:///zfb/wrapper.mjs","import inner from './inner.mjs';console.info('wrapper boot');export default {async fetch(request){return inner.fetch(request)}};").await.unwrap();
    for _ in 0..2 {
        let result = host
            .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
            .await
            .unwrap();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&result.body).unwrap(),
            json!({"boots":1,"main":false})
        );
    }
    let error = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/throw"))
        .await
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("inner-marker") && error.contains("file:///zfb/inner.mjs:3:"),
        "{error}"
    );
    assert!(error.contains("file:///zfb/wrapper.mjs:1:"), "{error}");
    assert_eq!(
        host.drain_console_logs(),
        "[log] inner boot\n[info] wrapper boot"
    );
    assert_eq!(host.drain_console_logs(), "");
}
#[tokio::test(flavor = "current_thread")]
async fn strict_invalid_inner_fails_at_boot_with_console() {
    let mut host = host();
    let error = host
        .validate_worker_module_shape(
            "file:///zfb/invalid.mjs",
            "console.error('invalid inner');export default {fetch:42};",
        )
        .await
        .unwrap_err();
    assert!(error.to_string().contains("default.fetch` is not callable"));
    assert_eq!(host.drain_console_logs(), "[error] invalid inner");
}
#[tokio::test(flavor = "current_thread")]
async fn worker_binary_headers_and_tolerant_utility_retain_installed_worker() {
    let mut host = host();
    host.execute_worker_module("bundle.mjs","export default {async fetch(req){return new Response(new Uint8Array(await req.arrayBuffer()),{status:201,headers:[['set-cookie','a=1'],['set-cookie','b=2'],['x-test','one'],['x-test','two']]});}};").await.unwrap();
    host.execute_module("utility.mjs", "export const unrelated=true;")
        .await
        .unwrap();
    let mut request = HttpRequestLike::get("http://zfb.local/");
    request.method = "POST".into();
    request.body = Some(vec![0, 128, 255]);
    let response = host.dispatch_fetch(request).await.unwrap();
    assert_eq!(response.status, 201);
    assert_eq!(response.body, vec![0, 128, 255]);
    assert_eq!(
        response.headers,
        vec![
            ("set-cookie".into(), "a=1".into()),
            ("set-cookie".into(), "b=2".into()),
            ("x-test".into(), "one, two".into())
        ]
    );
}
#[tokio::test(flavor = "current_thread")]
async fn module_evaluation_resets_mode_and_fresh_host_drops_globals() {
    let mut host = host();
    host.execute_worker_module(
        "bundle.mjs",
        "globalThis.marker=42;export default {fetch(){return Response.json(__zfb.mode)}};",
    )
    .await
    .unwrap();
    let mut request = HttpRequestLike::get("http://zfb.local/");
    request.mode = DispatchMode::RequestTime;
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(
            &host.dispatch_fetch(request).await.unwrap().body
        )
        .unwrap(),
        "request-time"
    );
    let module = host
        .execute_module("next.mjs", "export const mode=String(__zfb.mode);")
        .await
        .unwrap();
    assert_eq!(host.get_export(&module, "mode").await.unwrap(), "undefined");
    assert_eq!(
        host.services.as_ref().unwrap().borrow().mode,
        DispatchMode::BuildTime
    );
    drop(host);
    let mut host = NativeHost::with_loader(BundleModuleLoader::new()).unwrap();
    let module = host
        .execute_module("fresh.mjs", "export const value=typeof globalThis.marker;")
        .await
        .unwrap();
    assert_eq!(
        host.get_export(&module, "value").await.unwrap(),
        "undefined"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn empty_get_body_is_the_baseline_bodyless_request() {
    let mut host = host();
    host.execute_worker_module("empty.mjs","export default {async fetch(req){return Response.json({method:req.method,bytes:Array.from(new Uint8Array(await req.arrayBuffer()))});}};").await.unwrap();
    let mut request = HttpRequestLike::get("http://zfb.local/");
    request.body = Some(Vec::new());
    let result = host.dispatch_fetch(request).await.unwrap();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&result.body).unwrap(),
        json!({"method":"GET","bytes":[]})
    );
}

#[tokio::test(flavor = "current_thread")]
async fn utility_boxed_strings_and_dynamic_ext_errors_match_baseline() {
    let mut host = host();
    let handle=host.execute_module("probe.mjs","export const boxed=new String('ab');export const resolved=import.meta.resolve('ext:zfb_node_stubs/node_stub.js');let name;try{await import('ext:zfb_node_stubs/node_stub.js')}catch(e){name=e.name}export {name};").await.unwrap();
    assert_eq!(
        host.get_export(&handle, "boxed").await.unwrap(),
        json!({"0":"a","1":"b"})
    );
    assert_eq!(host.get_export(&handle, "name").await.unwrap(), "TypeError");
    assert_eq!(
        host.get_export(&handle, "resolved").await.unwrap(),
        "ext:zfb_node_stubs/node_stub.js"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn dependency_syntax_error_retains_exact_authored_location() {
    let loader = BundleModuleLoader::new();
    loader.register_module("file:///zfb/syntax-dependency.mjs", "\n\nexport const = ;");
    let mut host = NativeHost::with_loader(loader).unwrap();
    let error=host.execute_worker_module("bundle.mjs","import './syntax-dependency.mjs';export default {fetch(){return new Response('unreachable')}};").await.unwrap_err().to_string();
    assert!(
        error.contains("SyntaxError") && error.contains("file:///zfb/syntax-dependency.mjs:3:14"),
        "{error}"
    );
}
#[tokio::test(flavor = "current_thread")]
async fn utility_string_key_map_matches_json_and_other_keys_fail_clearly() {
    let mut host = host();
    let module=host.execute_module("map.mjs","export const good=new Map([['answer',42],['nested',{ok:true}]]);export const bad=new Map([[42,'value']]);").await.unwrap();
    assert_eq!(
        host.get_export(&module, "good").await.unwrap(),
        json!({"answer":42,"nested":{"ok":true}})
    );
    assert!(host
        .get_export(&module, "bad")
        .await
        .unwrap_err()
        .to_string()
        .contains("Map key must be a string"));
}

#[tokio::test(flavor = "current_thread")]
async fn native_resolver_error_without_source_position_cannot_overflow_diagnostics() {
    let mut host = host();
    let error=host.execute_worker_module("bundle.mjs","import 'fixture-alias/deep';export default {fetch(){return new Response('unreachable')}};").await.unwrap_err().to_string();
    assert!(
        error.contains("failed to resolve") && error.contains("fixture-alias/deep"),
        "{error}"
    );
    drop(host);
    assert_eq!(
        super::evaluate_config_bundle("export default {recovered:true}")
            .await
            .unwrap(),
        json!({"recovered":true})
    );
}
