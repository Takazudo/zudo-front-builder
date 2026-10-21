//! Literal expectations from deno_core 0.399.0, not a name-only comparison.
use super::super::{services, NativeHost};

fn emit(scope: &mut v8::PinScope, args: v8::FunctionCallbackArguments, mut rv: v8::ReturnValue) {
    let name = args
        .get(0)
        .to_string(scope)
        .unwrap()
        .to_rust_string_lossy(scope);
    rv.set(super::named_error(scope, &name, "native message").unwrap());
}

fn host(polyfills: &str) -> NativeHost {
    let mut host = NativeHost::new_inner(polyfills, Some(services::Services::defaults())).unwrap();
    {
        v8::scope!(let scope, &mut host.isolate);
        let context = v8::Local::new(scope, &host.context);
        let scope = &mut v8::ContextScope::new(scope, context);
        let global = context.global(scope);
        let key = v8::String::new(scope, "emit").unwrap();
        let callback = v8::Function::new(scope, emit).unwrap();
        assert_eq!(
            global.create_data_property(scope, key.into(), callback.into()),
            Some(true)
        );
    }
    host.execute_script(
        "error-test:assert",
        r#"
        function assert(value) { if (!value) throw new Error('error contract assertion failed'); }
        function duplicate(name) {
            try { __zfbHost.registerErrorClass(name, class {}); }
            catch (e) {
                assert(e instanceof TypeError);
                assert(e.message === `Error class for "${name}" already registered`);
                return;
            }
            throw new Error('duplicate registration succeeded');
        }
    "#,
    )
    .unwrap();
    host
}

#[test]
fn registered_constructor_identity_and_behavior() {
    let mut host = host("");
    host.execute_script(
        "error-test:identity",
        r#"
        let calls = 0;
        class Custom extends Error {
            constructor(message) {
                super('decorated ' + message);
                calls++;
                this.name = 'Custom';
                this.extra = 42;
            }
        }
        __zfbHost.registerErrorClass('TimeoutError', Custom);
        const e = emit('TimeoutError');
        assert(calls === 1 && e instanceof Custom && e instanceof Error);
        assert(Object.getPrototypeOf(e) === Custom.prototype && e.constructor === Custom);
        assert(e.message === 'decorated native message' && e.extra === 42);
        assert(e.stack.split('\n')[0] === 'Custom: decorated native message');
        duplicate('TimeoutError');
        assert(emit('TimeoutError') instanceof Custom);
        __zfbHost.registerErrorClass('Replacement', function(message) {
            return { name: 'Replacement', message, marker: 7 };
        });
        const replacement = emit('Replacement');
        assert(replacement.marker === 7 && replacement.message === 'native message');
        assert(replacement.stack.split('\n')[0] === 'Replacement: native message');
    "#,
    )
    .unwrap();
}

#[test]
fn registration_negative_contracts_and_constructor_exceptions() {
    let mut host = host("");
    host.execute_script("error-test:negative", r#"
        assert(emit('Unknown') === undefined);
        let unknownCaught = false;
        try { __zfbHost.digest('unsupported', new Uint8Array()); }
        catch (e) { unknownCaught = true; assert(e === undefined); }
        assert(unknownCaught);
        for (const name of ['Error','TypeError','RangeError','ReferenceError','SyntaxError','URIError',
                            'toString','constructor','__proto__']) duplicate(name);
        for (const [name, value] of [['Null',null],['Number',42],['Arrow',()=>{}],['Missing',undefined]]) {
            assert(__zfbHost.registerErrorClass(name, value) === undefined);
            assert(emit(name) === 'native message');
            duplicate(name);
        }
        for (const [name, thrown] of [['ThrowsError',new Error('constructor failure')],['ThrowsNull',null]]) {
            __zfbHost.registerErrorClass(name, function() { throw thrown; });
            assert(emit(name) === 'native message');
        }
        let inspected = false;
        __zfbHost.registerErrorClass('ThrowGetter', function() {
            throw { get message() { inspected = true; throw null; } };
        });
        assert(emit('ThrowGetter') === 'native message' && inspected);
        const sentinel = {};
        try {
            __zfbHost.registerErrorClass({toString() { throw sentinel; }}, Error);
            assert(false);
        } catch (e) { assert(e === sentinel); }
        __zfbHost.registerErrorClass(undefined, Error);
        assert(emit('undefined') instanceof Error);
        __zfbHost.registerErrorClass('NotSupportedError', function() { throw null; });
        let failureCaught = false;
        try { __zfbHost.digest('unsupported', new Uint8Array()); }
        catch (e) {
            failureCaught = true;
            assert(e === 'crypto.subtle.digest: unsupported algorithm "unsupported". This host implements SHA-1, SHA-256, SHA-384, SHA-512.');
        }
        assert(failureCaught);
        // Builder failures must leave no pending V8 exception.
        assert(emit('TypeError').message === 'native message');
    "#).unwrap();
}

#[test]
fn canonical_classes_and_context_lifetime() {
    assert!(NativeHost::new_inner(
        "throw new Error('bootstrap failure after registry installation')",
        Some(services::Services::defaults()),
    )
    .is_err());
    for _ in 0..3 {
        let mut host = host(crate::shims::WEB_POLYFILLS_SRC);
        host.isolate.low_memory_notification();
        host.execute_script(
            "error-test:canonical",
            r#"
            for (const name of ['TimeoutError','QuotaExceededError','NotSupportedError']) {
                const e = emit(name);
                assert(e instanceof Error && e.constructor !== Error);
                assert(Object.getPrototypeOf(e) === e.constructor.prototype);
                assert(e instanceof e.constructor && e.name === name);
                assert(e.message === 'native message');
                assert(e.stack.split('\n')[0] === name + ': native message');
                assert(new e.constructor('again').message === 'again');
                duplicate(name);
            }
            const capturedTypeError = TypeError;
            Error.captureStackTrace = () => { throw new Error('replaced capture'); };
            globalThis.TypeError = class Fake {};
            assert(emit('TypeError') instanceof capturedTypeError);
            delete globalThis.__zfbHost;
            assert(emit('QuotaExceededError').name === 'QuotaExceededError');
        "#,
        )
        .unwrap();
    }
    let mut fresh = host("");
    fresh
        .execute_script(
            "error-test:fresh",
            "assert(emit('TimeoutError') === undefined)",
        )
        .unwrap();
}

#[test]
fn native_crypto_errors_use_canonical_registered_constructors() {
    let mut host = host(crate::shims::WEB_POLYFILLS_SRC);
    host.execute_script(
        "error-test:crypto",
        r#"
        for (const [name, invoke] of [
            ['QuotaExceededError', () => __zfbHost.randomBytes(new Uint8Array(65537))],
            ['NotSupportedError', () => __zfbHost.digest('unsupported', new Uint8Array())],
        ]) {
            const constructor = emit(name).constructor;
            let caught = false;
            try { invoke(); } catch(e) {
                caught = true;
                assert(e instanceof constructor && e.constructor === constructor);
                assert(e.name === name && e.message.length > 0);
                assert(e.stack.split('\n')[0] === name + ': ' + e.message);
            }
            assert(caught);
        }
    "#,
    )
    .unwrap();
}
