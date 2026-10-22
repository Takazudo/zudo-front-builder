//! Shared process initialization and thread ownership for both embedded hosts.
use std::cell::Cell;
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::OnceLock;

/// Pinned ICU 77 common data (see `icu/README.md`); refresh only with V8.
#[cfg(feature = "direct-v8")]
const ICU_DATA: &[u8; 10_822_192] =
    include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/icu/icudtl.dat"));

/// Initialize before spawning host threads. Library/test callers may initialize
/// lazily; they must otherwise arrange the same main-thread startup contract.
pub fn initialize() -> Result<(), String> {
    static INITIALIZE: OnceLock<Result<(), String>> = OnceLock::new();
    INITIALIZE
        .get_or_init(|| {
            #[cfg(feature = "direct-v8")]
            let platform = {
                #[repr(align(16))]
                struct AlignedIcu<const N: usize>([u8; N]);
                static ICU: AlignedIcu<{ ICU_DATA.len() }> = AlignedIcu(*ICU_DATA);
                v8::icu::set_common_data_77(&ICU.0)
                    .map_err(|code| format!("ICU77 data initialization failed: {code}"))?;
                Some(
                    v8::new_custom_platform(
                        0,
                        false,
                        cfg!(test),
                        crate::native::platform::NativePlatform,
                    )
                    .make_shared(),
                )
            };
            #[cfg(not(feature = "direct-v8"))]
            let platform = None;
            #[cfg(feature = "deno-platform")]
            deno_core::JsRuntime::init_platform(platform);
            #[cfg(not(feature = "deno-platform"))]
            {
                v8::V8::set_flags_from_string(concat!(
                    "--wasm-test-streaming --no-validate-asm --turbo_fast_api_calls ",
                    "--harmony-temporal --js-float16array --js-explicit-resource-management ",
                    "--js-source-phase-imports"
                ));
                v8::V8::initialize_platform(platform.expect("direct-V8 platform"));
                v8::V8::initialize();
            }
            Ok(())
        })
        .clone()
}

thread_local! {
    static HOST_ACTIVE: Cell<bool> = const { Cell::new(false) };
}

/// One live isolate per thread prevents non-LIFO Rust drops violating V8's
/// entered-isolate stack. The guard itself cannot migrate to another thread.
pub struct HostThreadGuard(PhantomData<Rc<()>>);
impl HostThreadGuard {
    pub fn acquire() -> Result<Self, String> {
        if HOST_ACTIVE.with(|active| active.replace(true)) {
            Err("a native host is already alive on this thread".into())
        } else {
            Ok(Self(PhantomData))
        }
    }
}
impl Drop for HostThreadGuard {
    fn drop(&mut self) {
        HOST_ACTIVE.with(|active| active.set(false));
    }
}

#[cfg(all(test, feature = "direct-v8"))]
mod tests {
    use sha2::{Digest, Sha256};

    #[test]
    fn embedded_icu_data_matches_the_pin() {
        let digest: String = Sha256::digest(super::ICU_DATA)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            digest,
            "1cf67874b5a87a8363a86fb3f81e3cbbed54d389062dab8fb52308d5cf8c8612"
        );
    }
}
