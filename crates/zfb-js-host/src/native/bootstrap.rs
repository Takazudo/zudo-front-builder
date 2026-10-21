//! Process initialization and the globals that bare V8 does not supply.
use std::io::Write;
use std::panic::{catch_unwind, AssertUnwindSafe};

use super::diagnostics::throw_error;

pub(super) use crate::process::initialize;

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
