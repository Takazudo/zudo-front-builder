//! Fixed bridge values only; this is not a general Rust/V8 serializer.
use crate::fetch::FetchOutcome;

pub(super) fn named_error<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    name: &str,
    message: &str,
) -> Option<v8::Local<'s, v8::Value>> {
    let message = v8::String::new(scope, message)?;
    let error = if name == "TypeError" {
        v8::Exception::type_error(scope, message)
    } else {
        v8::Exception::error(scope, message)
    };
    let object = v8::Local::<v8::Object>::try_from(error).ok()?;
    let key = v8::String::new(scope, "name")?;
    let name = v8::String::new(scope, name)?;
    object.create_data_property(scope, key.into(), name.into())?;
    Some(error)
}

pub(super) fn throw_named(scope: &mut v8::PinScope, name: &str, message: &str) {
    if let Some(error) = named_error(scope, name, message) {
        scope.throw_exception(error);
    }
}

pub(super) fn copy_view(value: v8::Local<v8::Value>, limit: usize) -> Result<Vec<u8>, String> {
    let view = v8::Local::<v8::ArrayBufferView>::try_from(value)
        .map_err(|_| "expected an ArrayBufferView")?;
    let length = view.byte_length();
    if length > limit || length > i32::MAX as usize {
        return Err("buffer exceeds the native byte limit".into());
    }
    let mut bytes = vec![0; length];
    if view.copy_contents(&mut bytes) != length {
        return Err("buffer was detached or changed during native copy".into());
    }
    Ok(bytes)
}

pub(super) fn bytes<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    bytes: Vec<u8>,
) -> Option<v8::Local<'s, v8::Uint8Array>> {
    let length = bytes.len();
    let store =
        v8::ArrayBuffer::new_backing_store_from_boxed_slice(bytes.into_boxed_slice()).make_shared();
    let buffer = v8::ArrayBuffer::with_backing_store(scope, &store);
    v8::Uint8Array::new(scope, buffer, 0, length)
}

pub(super) fn outcome<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    outcome: FetchOutcome,
) -> Option<v8::Local<'s, v8::Value>> {
    // The body stays binary. The small fixed response metadata takes V8's JSON
    // parser; no generic serde-to-V8 graph converter or new serializer exists.
    let metadata = serde_json::json!({"status":outcome.status,"statusText":outcome.status_text,"headers":outcome.headers,"url":outcome.url,"redirected":outcome.redirected}).to_string();
    let metadata = v8::String::new(scope, &metadata)?;
    let object = v8::json::parse(scope, metadata)?;
    let object = v8::Local::<v8::Object>::try_from(object).ok()?;
    let key = v8::String::new(scope, "body")?;
    let body = bytes(scope, outcome.body)?;
    object.create_data_property(scope, key.into(), body.into())?;
    Some(object.into())
}
