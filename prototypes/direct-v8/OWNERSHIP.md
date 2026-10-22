# Direct-V8 ownership obligations

Since #4395 the in-process V8 host in `crates/zfb-js-host` is zfb's only JS
runtime. No deno_core release brings in V8, ICU or callback-safety fixes any
more, so zfb owns each item below directly (#4398, epic #4386).

## V8 pin and upgrade path

- `crates/zfb-js-host/Cargo.toml` pins `v8 = "=147.4.0"`, with no default
  features plus `use_custom_libcxx`. The exact pin is deliberate. Every
  graph, size and closure gate is keyed to this one `v8` row, and
  `record-graph.py` requires exactly one `v8 v147.4.0` in every selection on
  all five targets.
- An upgrade is a reviewed Cargo transition, not a routine `cargo update`.
  It needs:
  1. A bump of the exact pin.
  2. A re-check of the native surface below (`UnsafeRawIsolatePtr` and the
     `callback_scope!` sites).
  3. The ICU refresh when the ICU version moves.
  4. The `direct-v8.yml` rows (`selection=default`).
  5. The owner-run Darwin rows (`PLATFORMS.md`).
  6. A paired island-size re-pin through its Cargo-transition reason.
- Upstream advisories reach zfb through `cargo deny` and `security-audit.yml`.
  `deny.toml` keeps RUSTSEC-2024-0436 (`paste`, reached only through `v8`).
  Re-check that exception on every V8 bump.

## ICU data

`crates/zfb-js-host/icu/icudtl.dat` is vendored ICU 77 data, matching V8
147.4.0. Its size, SHA-256, source and license are in `icu/README.md`, and
`prepare-icu.py --verify` together with the `embedded_icu_data_matches_the_pin`
unit test guard the bytes. Refresh it only together with a V8 upgrade that
changes the ICU version, following that README.

## Unsafe and native callback surface

`crates/zfb-js-host/src` has six `unsafe` sites:

- **Five `v8::callback_scope!(unsafe …)` entries** for V8 callbacks: two in
  `native/modules.rs`, two in `native/diagnostics.rs`, one in
  `native/worker_modules.rs`. Each enters from a V8-owned callback with a live
  context or message. Re-read them when the `v8` crate changes its callback API.
- **One `transmute` in `native/platform.rs`.** It turns
  `v8::UnsafeRawIsolatePtr` into a `usize` registry key for the foreground task
  queue. The key is never dereferenced or used from another thread. A
  compile-time assertion checks only that the sizes match, so the layout
  assumption (`repr(transparent)` in 147.4.0) needs a manual re-check on every
  V8 bump.

Any new `unsafe` in this crate needs the same SAFETY comment and review.

## Harness watchdog limitation (#4289)

The qualification checkers' POSIX watchdog drains output best-effort at the
deadline (#4383, `WATCHDOG-OWNERSHIP-4350.md`):

- Bytes a killed child wrote are recovered within one shared one-second
  window, and `watchdog_cleanup.drain_complete` says whether the drain
  finished.
- Descendant cleanup (U5) and `accept.py` containment (U6) are not claimed.
  `accept.py` is archived with the Deno host.
- Windows keeps `subprocess.run`.

This is a limitation of the evidence harness, not of the shipped CLI. Treat a
best-effort drain as incomplete evidence, never as a pass.
