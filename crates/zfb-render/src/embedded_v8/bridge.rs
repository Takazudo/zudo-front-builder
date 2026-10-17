//! Backend adapter for the fixed ZFB-owned JavaScript bridge.
pub const DENO_BRIDGE_SRC: &str = r#"
globalThis.__zfbHost = {
  get fetch() { return globalThis.Deno?.core?.ops?.op_zfb_fetch; },
  get cancelFetch() { return globalThis.Deno?.core?.ops?.op_zfb_fetch_cancel; },
  get randomBytes() { return globalThis.Deno?.core?.ops?.op_zfb_random_bytes; },
  get digest() { return globalThis.Deno?.core?.ops?.op_zfb_digest; },
  registerErrorClass: (name, constructor) => Deno.core.registerErrorClass(name, constructor),
};
"#;
