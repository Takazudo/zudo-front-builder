//! Same-process engine ownership probe. Run each order in a fresh process.
use zfb_js_host::native::NativeHost;
use zfb_render::EmbeddedV8RenderHost;

const CONFIG: &str = r#"
const bytes = new Uint8Array([0,97,115,109,1,0,0,0,1,5,1,96,0,1,127,3,2,1,0,7,10,1,6,97,110,115,119,101,114,0,0,10,6,1,4,0,65,42,11]);
globalThis.cached = new WebAssembly.Module(bytes);
export default { answer: new WebAssembly.Instance(await WebAssembly.compile(bytes)).exports.answer() };
"#;
fn exercise(native_first: bool) {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    for native in [native_first, !native_first] {
        let value = if native {
            runtime
                .block_on(zfb_js_host::native::evaluate_config_bundle(CONFIG))
                .unwrap()
        } else {
            runtime
                .block_on(zfb_render::config_eval::evaluate_config_bundle(CONFIG))
                .unwrap()
        };
        assert_eq!(value, serde_json::json!({"answer":42}));
    }
    let native = NativeHost::new_config().unwrap();
    assert!(EmbeddedV8RenderHost::new().is_err());
    drop(native);
    let deno = EmbeddedV8RenderHost::new().unwrap();
    assert!(NativeHost::new_config().is_err());
    drop(deno);
    drop(NativeHost::new_config().unwrap());
}
fn main() {
    zfb_render::initialize_v8().unwrap();
    let native_first = std::env::args().nth(1).as_deref() == Some("native-first");
    exercise(native_first);
    let threads: Vec<_> = (0..4)
        .map(|index| std::thread::spawn(move || exercise(index % 2 == 0)))
        .collect();
    for thread in threads {
        thread.join().unwrap();
    }
    println!("shared V8 initializer, foreground delivery and thread ownership: PASS");
}
