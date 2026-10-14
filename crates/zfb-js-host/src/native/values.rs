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

/// Only the JSON-valued utility export contract, not a general serialization
/// engine. Match enumerable properties without invoking JSON.stringify/toJSON.
pub(super) fn json_value<'s>(
    scope: &mut v8::PinScope<'s, '_>,
    value: v8::Local<'s, v8::Value>,
) -> Result<serde_json::Value, super::NativeError> {
    fn convert<'s>(
        scope: &mut v8::PinScope<'s, '_>,
        value: v8::Local<'s, v8::Value>,
        ancestors: &mut Vec<v8::Local<'s, v8::Object>>,
    ) -> Result<serde_json::Value, super::NativeError> {
        use serde_json::Value;
        let error = |detail: &str| {
            super::NativeError::Evaluation(format!("export is not JSON-serialisable: {detail}"))
        };
        if value.is_null_or_undefined() {
            return Ok(Value::Null);
        }
        if value.is_boolean() {
            return Ok(Value::Bool(value.is_true()));
        }
        if value.is_uint32() {
            return Ok(Value::from(value.uint32_value(scope).unwrap()));
        }
        if value.is_int32() {
            return Ok(Value::from(value.int32_value(scope).unwrap()));
        }
        if value.is_number() {
            return Ok(value
                .number_value(scope)
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number)
                .unwrap_or(Value::Null));
        }
        if value.is_string() {
            return value
                .to_string(scope)
                .map(|v| Value::String(v.to_rust_string_lossy(scope)))
                .ok_or_else(|| error("string conversion threw"));
        }
        if value.is_big_int() {
            return Err(error("BigInt is unsupported"));
        }
        // serde_json::Value's byte-buffer visitor rejects native buffer values.
        if value.is_array_buffer() || value.is_array_buffer_view() {
            return Err(error("buffer is not a JSON value"));
        }
        let object =
            v8::Local::<v8::Object>::try_from(value).map_err(|_| error("unsupported value"))?;
        if ancestors.contains(&object) {
            return Err(error("circular object"));
        }
        ancestors.push(object);
        let result = if let Ok(map) = v8::Local::<v8::Map>::try_from(value) {
            let pairs = map.as_array(scope);
            let mut out = serde_json::Map::new();
            for index in (0..pairs.length()).step_by(2) {
                let key = pairs
                    .get_index(scope, index)
                    .ok_or_else(|| error("Map key missing"))?;
                if !key.is_string() && !key.is_string_object() {
                    return Err(error("Map key must be a string"));
                }
                let key = key
                    .to_string(scope)
                    .ok_or_else(|| error("Map key conversion threw"))?
                    .to_rust_string_lossy(scope);
                let value = pairs
                    .get_index(scope, index + 1)
                    .ok_or_else(|| error("Map value missing"))?;
                out.insert(key, convert(scope, value, ancestors)?);
            }
            Value::Object(out)
        } else if let Ok(array) = v8::Local::<v8::Array>::try_from(value) {
            let mut out = Vec::with_capacity(array.length() as usize);
            for index in 0..array.length() {
                let value = array
                    .get_index(scope, index)
                    .ok_or_else(|| error("array getter threw"))?;
                out.push(convert(scope, value, ancestors)?);
            }
            Value::Array(out)
        } else {
            let keys = object
                .get_own_property_names(
                    scope,
                    v8::GetPropertyNamesArgsBuilder::new()
                        .key_conversion(v8::KeyConversionMode::ConvertToString)
                        .build(),
                )
                .ok_or_else(|| error("property enumeration threw"))?;
            let mut out = serde_json::Map::new();
            for index in 0..keys.length() {
                let key = keys
                    .get_index(scope, index)
                    .ok_or_else(|| error("property key missing"))?;
                let value = object
                    .get(scope, key)
                    .ok_or_else(|| error("property getter threw"))?;
                if value.is_undefined() {
                    continue;
                }
                let key = key
                    .to_string(scope)
                    .ok_or_else(|| error("property key conversion threw"))?
                    .to_rust_string_lossy(scope);
                out.insert(key, convert(scope, value, ancestors)?);
            }
            Value::Object(out)
        };
        ancestors.pop();
        Ok(result)
    }
    convert(scope, value, &mut Vec::new())
}
