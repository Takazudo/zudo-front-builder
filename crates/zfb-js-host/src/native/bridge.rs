//! The five fixed native callbacks required by the canonical JavaScript shims.
use std::panic::{catch_unwind, AssertUnwindSafe};

use crate::crypto::{digest::digest_bytes, fill_random_bytes};
use crate::dispatch_mode::DispatchMode;
use crate::fetch::{perform_fetch, FetchError, FetchRequestSpec};

use super::{services, values};

pub(super) fn install(scope: &mut v8::PinScope) -> Result<(), String> {
    values::install_errors(scope).ok_or("error registry installation failed")?;
    let bridge = v8::Object::new(scope);
    macro_rules! bind {
        ($name:literal, $callback:ident) => {{
            let key = v8::String::new(scope, $name).ok_or("bridge name allocation failed")?;
            let callback =
                v8::Function::new(scope, $callback).ok_or("bridge function allocation failed")?;
            if bridge.create_data_property(scope, key.into(), callback.into()) != Some(true) {
                return Err("bridge property installation failed".into());
            }
        }};
    }
    bind!("fetch", fetch);
    bind!("cancelFetch", cancel_fetch);
    bind!("randomBytes", random_bytes);
    bind!("digest", digest);
    bind!("registerErrorClass", register_error_class);
    let global = scope.get_current_context().global(scope);
    let key = v8::String::new(scope, "__zfbHost").ok_or("bridge global allocation failed")?;
    if global.create_data_property(scope, key.into(), bridge.into()) != Some(true) {
        return Err("bridge global installation failed".into());
    }
    Ok(())
}

fn fetch(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    if catch_unwind(AssertUnwindSafe(|| {
        let Some(resolver) = v8::PromiseResolver::new(scope) else {
            return;
        };
        rv.set(resolver.get_promise(scope).into());
        let result = start_fetch(scope, &args, resolver);
        if let Err(error) = result {
            if let Some(error) =
                values::named_error(scope, error.js_error_class(), &error.to_string())
            {
                resolver.reject(scope, error);
            }
        }
    }))
    .is_err()
    {
        values::throw_named(scope, "TypeError", "native fetch callback failed");
    }
}

fn start_fetch(
    scope: &mut v8::PinScope,
    args: &v8::FunctionCallbackArguments,
    resolver: v8::Local<v8::PromiseResolver>,
) -> Result<(), FetchError> {
    let unavailable = |detail: &str| FetchError::HostUnavailable {
        url: "<native bridge>".into(),
        detail: detail.into(),
    };
    let state =
        services::state(scope).ok_or_else(|| unavailable("native services are not installed"))?;
    // Snapshot authority before touching user properties (which can run getters).
    // No state borrow may survive a V8 conversion or promise settlement.
    let (mode, client, counter, config) = {
        let state = state
            .try_borrow()
            .map_err(|_| unavailable("native state is busy"))?;
        (
            state.mode,
            state.client.clone(),
            state.budget.clone(),
            state.config,
        )
    };
    let value = args.get(0);
    let json = v8::json::stringify(scope, value)
        .ok_or_else(|| unavailable("invalid fetch specification"))?;
    let spec: FetchRequestSpec = serde_json::from_str(&json.to_rust_string_lossy(scope))
        .map_err(|error| unavailable(&format!("invalid fetch specification: {error}")))?;
    if mode != DispatchMode::RequestTime {
        return Err(FetchError::BuildTimeDenied { url: spec.url });
    }
    let client = client.ok_or_else(|| FetchError::HostUnavailable {
        url: spec.url.clone(),
        detail: "outbound HTTP client is not installed in this runtime".into(),
    })?;
    let view = v8::Local::<v8::ArrayBufferView>::try_from(args.get(1))
        .map_err(|_| unavailable("fetch body must be an ArrayBufferView"))?;
    if spec.has_body && view.byte_length() > config.max_request_body_bytes {
        return Err(FetchError::RequestBodyTooLarge {
            url: spec.url,
            limit: config.max_request_body_bytes,
        });
    }
    let body = if spec.has_body {
        values::copy_view(args.get(1), config.max_request_body_bytes)
            .map_err(|detail| unavailable(&detail))?
    } else {
        Vec::new()
    };
    let url = spec.url.clone();
    let cancel_id = spec.cancel_id;
    let future =
        Box::pin(async move { perform_fetch(&client, &counter, &config, &spec, body).await });
    let resolver = v8::Global::new(scope, resolver);
    let mut state = state
        .try_borrow_mut()
        .map_err(|_| unavailable("native state is busy"))?;
    let id = state.next_id;
    state.next_id = state.next_id.wrapping_add(1);
    if let Some(cancel_id) = cancel_id {
        state.cancellations.insert(cancel_id, id);
    }
    state.pending.push(services::PendingFetch {
        id,
        url,
        cancel_id,
        resolver,
        transport: future,
    });
    Ok(())
}

fn cancel_fetch(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    if catch_unwind(AssertUnwindSafe(|| {
        let Some(id) = args.get(0).uint32_value(scope) else {
            return;
        };
        let Some(state) = services::state(scope) else {
            return;
        };
        let cancelled = state
            .try_borrow_mut()
            .ok()
            .and_then(|mut state| state.remove_cancelled(id));
        if let Some(operation) = cancelled {
            let services::PendingFetch {
                resolver,
                transport,
                url,
                ..
            } = operation;
            // This is cancellation, not merely promise rejection: drop reqwest's
            // future BEFORE returning to JS, even if the host then becomes idle.
            drop(transport);
            let resolver = v8::Local::new(scope, resolver);
            if let Some(error) =
                values::named_error(scope, "TypeError", &FetchError::Aborted { url }.to_string())
            {
                resolver.reject(scope, error);
            }
        }
    }))
    .is_err()
    {
        values::throw_named(
            scope,
            "TypeError",
            "native fetch cancellation callback failed",
        );
    }
}

fn random_bytes(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    if catch_unwind(AssertUnwindSafe(|| {
        let Ok(view) = v8::Local::<v8::ArrayBufferView>::try_from(args.get(0)) else {
            values::throw_named(scope, "TypeError", "randomBytes expects an ArrayBufferView");
            return;
        };
        let length = view.byte_length();
        if length == 0 {
            return;
        }
        if length > crate::limits::MAX_RANDOM_BYTES_PER_CALL {
            let error = crate::crypto::EntropyError::QuotaExceeded {
                requested: length,
                limit: crate::limits::MAX_RANDOM_BYTES_PER_CALL,
            };
            values::throw_named(scope, error.js_error_class(), &error.to_string());
            return;
        }
        let source = services::state(scope)
            .and_then(|state| state.try_borrow().ok().map(|state| state.entropy.clone()));
        let Some(source) = source else {
            values::throw_named(
                scope,
                "Error",
                "OS entropy unavailable: OS entropy source is not installed in this runtime",
            );
            return;
        };
        let mut bytes = vec![0; length];
        if let Err(error) = fill_random_bytes(source.as_ref(), &mut bytes) {
            values::throw_named(scope, error.js_error_class(), &error.to_string());
            return;
        }
        let Some(store) = view.get_backing_store() else {
            values::throw_named(scope, "TypeError", "randomBytes buffer is detached");
            return;
        };
        let Some(end) = view.byte_offset().checked_add(length) else {
            values::throw_named(scope, "TypeError", "randomBytes byte range overflow");
            return;
        };
        let Some(destination) = store.get(view.byte_offset()..end) else {
            values::throw_named(scope, "TypeError", "randomBytes buffer is detached");
            return;
        };
        for (cell, byte) in destination.iter().zip(bytes) {
            cell.set(byte);
        }
    }))
    .is_err()
    {
        values::throw_named(scope, "Error", "native entropy callback failed");
    }
}

fn digest(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    if catch_unwind(AssertUnwindSafe(|| {
        let Some(algorithm) = args.get(0).to_string(scope) else {
            return;
        };
        let data = match values::copy_view(args.get(1), i32::MAX as usize) {
            Ok(bytes) => bytes,
            Err(error) => {
                values::throw_named(scope, "TypeError", &error);
                return;
            }
        };
        match digest_bytes(&algorithm.to_rust_string_lossy(scope), &data) {
            Ok(result) => {
                if let Some(value) = values::bytes(scope, result) {
                    rv.set(value.into());
                }
            }
            Err(error) => values::throw_named(scope, error.js_error_class(), &error.to_string()),
        }
    }))
    .is_err()
    {
        values::throw_named(scope, "Error", "native digest callback failed");
    }
}

fn register_error_class(
    scope: &mut v8::PinScope,
    args: v8::FunctionCallbackArguments,
    _rv: v8::ReturnValue,
) {
    if catch_unwind(AssertUnwindSafe(|| {
        values::register_error(scope, args.get(0), args.get(1));
    }))
    .is_err()
    {
        values::throw_named(scope, "Error", "native error registration callback failed");
    }
}
