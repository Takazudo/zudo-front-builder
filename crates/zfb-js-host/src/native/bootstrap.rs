//! Process initialization and the globals that bare V8 does not supply.
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::OnceLock;

use super::diagnostics::throw_error;

// Match deno_core 0.399 runtime/setup.rs's ordinary (non-snapshot,
// non-expose-natives) flags. Engine upgrades are outside this experiment.
pub(super) fn initialize() -> Result<(), String> {
    // Exactly the baseline ICU77 bytes, prepared and checksum-verified by the
    // prototype setup script. Missing input is a build error, never a fallback.
    // ICU requires >=8-byte alignment and recommends 16-byte alignment.
    #[repr(align(16))]
    struct AlignedIcu<const N: usize>([u8; N]);
    static ICU: AlignedIcu<{ include_bytes!(env!("ZFB_V8_ICU_DATA")).len() }> =
        AlignedIcu(*include_bytes!(env!("ZFB_V8_ICU_DATA")));
    static INITIALIZE: OnceLock<Result<(), String>> = OnceLock::new();
    INITIALIZE
        .get_or_init(|| {
            v8::icu::set_common_data_77(&ICU.0)
                .map_err(|code| format!("ICU77 data initialization failed: {code}"))?;
            v8::V8::set_flags_from_string(concat!(
                "--wasm-test-streaming --no-validate-asm --turbo_fast_api_calls ",
                "--harmony-temporal --js-float16array --js-explicit-resource-management ",
                "--js-source-phase-imports"
            ));
            let platform = v8::new_default_platform(0, false).make_shared();
            v8::V8::initialize_platform(platform);
            v8::V8::initialize();
            Ok(())
        })
        .clone()
}

pub(super) fn install(scope: &mut v8::PinScope) -> Result<(), String> {
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, "queueMicrotask").ok_or("V8 string allocation failed")?;
    let function =
        v8::Function::new(scope, queue_microtask).ok_or("V8 function allocation failed")?;
    // Define an own data property: ordinary V8 Set may report success for a
    // silent sloppy-mode write to a non-writable property without installing it.
    if global.create_data_property(scope, key.into(), function.into()) != Some(true) {
        return Err("installing queueMicrotask failed".into());
    }
    let console = v8::Object::new(scope);
    for method in ["log", "info", "warn", "error", "debug", "trace"] {
        let key = v8::String::new(scope, method).ok_or("V8 string allocation failed")?;
        let function =
            v8::Function::new(scope, console_write).ok_or("V8 function allocation failed")?;
        if console.create_data_property(scope, key.into(), function.into()) != Some(true) {
            return Err("installing console method failed".into());
        }
    }
    let key = v8::String::new(scope, "console").ok_or("V8 string allocation failed")?;
    if global.create_data_property(scope, key.into(), console.into()) != Some(true) {
        return Err("installing console failed".into());
    }
    Ok(())
}

fn queue_microtask(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    if catch_unwind(AssertUnwindSafe(
        || match v8::Local::<v8::Function>::try_from(args.get(0)) {
            Ok(function) => scope.enqueue_microtask(function),
            Err(_) => {
                if let Some(message) = v8::String::new(scope, "queueMicrotask requires a function")
                {
                    let error = v8::Exception::type_error(scope, message);
                    scope.throw_exception(error);
                }
            }
        },
    ))
    .is_err()
    {
        throw_error(scope, "native queueMicrotask callback failed");
    }
}

fn console_write(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    if catch_unwind(AssertUnwindSafe(|| {
        let line = (0..args.length())
            .map(|index| args.get(index).to_rust_string_lossy(scope))
            .collect::<Vec<_>>()
            .join(" ");
        // Receipts are stdout-only; console output is diagnostic stderr.
        // Ignore broken stderr instead of panicking through the V8 callback.
        let _ = writeln!(std::io::stderr().lock(), "{line}");
    }))
    .is_err()
    {
        throw_error(scope, "native console callback failed");
    }
}
