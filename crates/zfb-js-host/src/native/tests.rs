//! Native process tests. Baseline comparison runs in a separate executable.
use super::{bootstrap, NativeError, NativeHost};
use std::cell::RefCell;
use std::sync::{Arc, Weak};

#[derive(Default)]
struct ConstructionProbe {
    queue: Weak<super::platform::ForegroundQueue>,
    fail_bootstrap: bool,
}

thread_local! {
    static CONSTRUCTION: RefCell<ConstructionProbe> = RefCell::default();
}

pub(super) fn observe_construction(queue: &Arc<super::platform::ForegroundQueue>) {
    CONSTRUCTION.with(|probe| probe.borrow_mut().queue = Arc::downgrade(queue));
}

pub(super) fn prepare_bootstrap(scope: &mut v8::PinScope) {
    if CONSTRUCTION.with(|probe| probe.borrow().fail_bootstrap) {
        let source = v8::String::new(
            scope,
            "Object.defineProperty(globalThis, 'queueMicrotask', {value: 1, configurable: false});",
        )
        .unwrap();
        v8::Script::compile(scope, source, None)
            .unwrap()
            .run(scope)
            .unwrap();
    }
}

fn observed_queue() -> Weak<super::platform::ForegroundQueue> {
    CONSTRUCTION.with(|probe| probe.borrow().queue.clone())
}

#[test]
fn foreground_queue_is_destroyed_after_successful_host_drop() {
    let host = NativeHost::new_config().unwrap();
    let queue = observed_queue();
    assert!(queue.upgrade().is_some());
    drop(host);
    assert!(!super::platform::is_registered(&queue));
    assert!(
        queue.upgrade().is_none(),
        "foreground queue still has an owner"
    );
}

#[test]
fn foreground_queue_is_destroyed_after_construction_failures_and_recovery() {
    for _ in 0..4 {
        CONSTRUCTION.with(|probe| probe.borrow_mut().fail_bootstrap = true);
        let bootstrap_error = NativeHost::new_config().err().unwrap();
        CONSTRUCTION.with(|probe| probe.borrow_mut().fail_bootstrap = false);
        assert!(matches!(bootstrap_error, NativeError::Bootstrap(_)));
        assert!(!super::platform::is_registered(&observed_queue()));
        assert!(
            observed_queue().upgrade().is_none(),
            "pre-transfer queue retained"
        );
        for source in ["function {", "throw new Error('polyfill-marker')"] {
            let error = NativeHost::new_config_with_polyfills(source).err().unwrap();
            assert!(matches!(error, NativeError::Evaluation(_)));
            assert!(!super::platform::is_registered(&observed_queue()));
            assert!(
                observed_queue().upgrade().is_none(),
                "post-transfer queue retained"
            );
        }
        assert_eq!(
            evaluate_config_bundle("export default {fresh:true};").unwrap(),
            serde_json::json!({"fresh":true})
        );
        assert!(!super::platform::is_registered(&observed_queue()));
        assert!(
            observed_queue().upgrade().is_none(),
            "recovered host queue retained"
        );
    }
}

fn evaluate_config_bundle(source: &str) -> Result<serde_json::Value, NativeError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(super::evaluate_config_bundle(source))
}

#[test]
fn bootstrap_installation_failure_is_distinct_from_polyfill_evaluation() {
    let mut host = NativeHost::new_config().expect("native host starts");
    host.execute_script(
        "zfb:bootstrap_failure_fixture",
        "Object.defineProperty(globalThis, 'queueMicrotask', {value: 1, writable: false, configurable: false});",
    ).expect("freeze bootstrap property");
    v8::scope!(let scope, &mut host.isolate);
    let context = v8::Local::new(scope, &host.context);
    let scope = &mut v8::ContextScope::new(scope, context);
    assert_eq!(
        bootstrap::install(scope).unwrap_err(),
        "installing queueMicrotask failed"
    );
}

#[test]
fn polyfill_compilation_and_execution_failures_name_the_bootstrap_stage() {
    for (source, expected) in [
        ("function {", "SyntaxError"),
        ("throw new Error('polyfill-marker')", "polyfill-marker"),
    ] {
        let error = NativeHost::new_config_with_polyfills(source)
            .err()
            .expect("polyfill fixture must fail");
        assert!(matches!(error, NativeError::Evaluation(_)));
        assert!(error
            .to_string()
            .contains("config-eval polyfill init failed"));
        assert!(error.to_string().contains(expected));
    }
}

#[test]
fn module_compilation_and_stringification_failures_keep_distinct_categories() {
    let syntax = evaluate_config_bundle("export default {;").unwrap_err();
    assert!(matches!(syntax, NativeError::ModuleLoad(_)));
    let stringify = evaluate_config_bundle(
        "export default {toJSON() { throw new Error('stringify-marker'); }};",
    )
    .unwrap_err();
    assert!(matches!(stringify, NativeError::Evaluation(_)));
    assert!(stringify.to_string().contains("JSON.stringify threw"));
    assert!(stringify.to_string().contains("stringify-marker"));
}

#[test]
fn microtask_failures_are_reported_and_handled_rejections_are_allowed() {
    let microtask = evaluate_config_bundle(
        "queueMicrotask(() => {throw new Error('microtask-marker')}); export default {};",
    )
    .unwrap_err();
    assert!(microtask.to_string().contains("microtask-marker"));
    let unhandled =
        evaluate_config_bundle("Promise.reject(new Error('rejection-marker')); export default {};")
            .unwrap_err();
    assert!(unhandled.to_string().contains("rejection-marker"));
    assert_eq!(
        evaluate_config_bundle(
            "Promise.reject(new Error('handled')).catch(() => {}); export default {ok:true};"
        )
        .unwrap(),
        serde_json::json!({"ok": true})
    );
}

#[test]
fn successive_dynamic_self_imports_finish_before_config_serialization() {
    let source = "const value={done:false}; import('file:///zfb-config-eval/main.mjs').then(()=>import('file:///zfb-config-eval/main.mjs')).then(()=>value.done=true); export default value;";
    assert_eq!(
        evaluate_config_bundle(source).unwrap(),
        serde_json::json!({"done":true})
    );
}

#[test]
fn unresolved_work_is_explicit_and_a_fresh_isolate_recovers() {
    let error =
        evaluate_config_bundle("await new Promise(() => {}); export default {};").unwrap_err();
    assert!(error
        .to_string()
        .contains("unresolved with no pending native work"));
    assert_eq!(
        evaluate_config_bundle("export default {answer:42};").unwrap(),
        serde_json::json!({"answer":42})
    );
}

#[test]
fn import_meta_resolve_coerces_to_a_string_and_rejects_extra_arguments() {
    assert_eq!(evaluate_config_bundle("const result=import.meta.resolve(new String(import.meta.url)); export default {type:typeof result, value:result};").unwrap(), serde_json::json!({"type":"string","value":"file:///zfb-config-eval/main.mjs"}));
    assert_eq!(evaluate_config_bundle("let error; try { import.meta.resolve(import.meta.url, 'extra'); } catch (e) {error=e.name+'|'+e.message;} export default {error};").unwrap(), serde_json::json!({"error":"TypeError|Invalid arguments"}));
}

#[test]
fn overlapping_hosts_are_rejected_without_corrupting_the_live_isolate() {
    let mut first = NativeHost::new_config().expect("first host");
    let error = NativeHost::new_config()
        .err()
        .expect("second live host denied");
    assert!(error.to_string().contains("already alive on this thread"));
    assert_eq!(
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(first.evaluate_config("export default {alive:true};"))
            .unwrap(),
        serde_json::json!({"alive":true})
    );
    drop(first);
    assert_eq!(
        evaluate_config_bundle("export default {fresh:true};").unwrap(),
        serde_json::json!({"fresh":true})
    );
}

#[test]
fn baseline_icu_data_supports_intl_date_formatting() {
    let value = evaluate_config_bundle("export default {date: new Intl.DateTimeFormat('en-US', {timeZone:'UTC'}).format(new Date('2020-01-02T03:04:05Z'))};").unwrap();
    assert_eq!(value, serde_json::json!({"date":"1/2/2020"}));
}

#[test]
fn configuration_top_level_await_drives_native_wasm_tasks() {
    let source = "const bytes=new Uint8Array([0,97,115,109,1,0,0,0,1,5,1,96,0,1,127,3,2,1,0,7,10,1,6,97,110,115,119,101,114,0,0,10,6,1,4,0,65,42,11]);const m=await WebAssembly.compile(bytes);export default {answer:new WebAssembly.Instance(m).exports.answer()};";
    assert_eq!(
        evaluate_config_bundle(source).unwrap(),
        serde_json::json!({"answer":42})
    );
}
