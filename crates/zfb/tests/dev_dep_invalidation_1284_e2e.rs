//! Wave-3 ACCEPTANCE gate for bug #1284 (epic #1285), authored during the
//! Wave-1 diagnosis (#1286). Level 4 — real `zfb dev` process, edit→serve loop.
//!
//! These scenarios reproduce the THREE symptoms that the existing
//! `dev_serve_e2e.rs` scenario 4 does NOT cover (that scenario edits a
//! *directly-imported* `components/**` file, which already re-renders today via
//! the orchestrator's blunt `PageSelection::All` fallback):
//!
//! - **A** — editing a component under `src/**` (NOT a watch root) does not
//!   re-render the consuming route. Acceptance: after the fix, editing
//!   `src/components/*.tsx` makes the route serve the new marker on next
//!   request.
//! - **B** — editing a transitively-imported CSS file (incl. a symlinked
//!   workspace dep reached via `@import`) does not refresh `/assets/styles.css`.
//!   Acceptance: after the fix, editing the imported CSS makes
//!   `/assets/styles.css` serve the new bytes.
//! - **C** — a NEW Tailwind utility class added inside a component is not
//!   emitted into `/assets/styles.css` until the CSS entry is touched.
//!   Acceptance: after the fix, the new class appears in `/assets/styles.css`
//!   without touching the CSS entry.
//!
//! ## D3 — the observable (locked by #1286)
//!
//! Under the lazy dev model (`lazy_render_tick` marks routes STALE; it does NOT
//! eagerly write), the test observable is **served-HTML / served-asset on the
//! NEXT request**, polled via `poll_until_*` — NOT an eager disk write. The SSE
//! `page` event is asserted as a secondary signal (it fires via the
//! `pages_stale` gate), exactly as `dev_serve_e2e.rs` scenario 4 does. For the
//! CSS symptoms the observable is the body of `GET /assets/styles.css`.
//!
//! ## Status — these are STUBS, intentionally `#[ignore]`d
//!
//! They are tagged `#[ignore = "pending fix: #1284"]` so they neither block the
//! T1 gate nor force a 15-30 min V8 first-compile in this diagnosis wave. The
//! Wave-3 author un-ignores them and wires them into the shared `dev_serve_e2e`
//! harness (reusing `spawn_dev` / `boot_and_handshake` / `poll_until_contains`
//! / `subscribe_sse`), OR fills in the `todo!()` bodies below against a local
//! copy of those helpers. They are kept as a separate file so the acceptance
//! contract is reviewable independently of the fix.
//!
//! Falsifiability is noted per scenario: revert the corresponding fix and the
//! served-on-next-request assertion times out on the OLD marker.

// The bodies are deferred to Wave-3 (see module docs). Keeping them as
// `#[ignore]`d `todo!()` stubs documents the exact acceptance contract without
// pulling the private `dev_serve_e2e` harness into this file or forcing a V8
// build now. `cargo test` skips `#[ignore]`d tests, so this stays green.

/// SYMPTOM A acceptance — editing `src/components/Widget.tsx` re-renders the
/// route that imports it. Observable (D3): `GET /` serves the new marker on the
/// next request after the edit; an SSE `page` event fires.
///
/// Falsifiability: with `src/` still outside the watch roots (or the All-only
/// selection), no tick fires / the bundle is not refreshed and the route keeps
/// serving the old marker until timeout.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "pending fix: #1284"]
async fn e2e_src_component_edit_rerenders_route() {
    todo!(
        "Wave-3: copy dev-loop-basic fixture, add src/components/Widget.tsx imported \
         by pages/index.tsx, spawn `zfb dev`, subscribe SSE, edit the widget, assert \
         an SSE `page` event then poll `GET /` until it serves the NEW marker."
    );
}

/// SYMPTOM B acceptance — editing a transitively-imported CSS file (a local
/// `@import './tokens.css'` and a symlinked workspace dep `@import
/// '@scope/design-system'`) refreshes `/assets/styles.css`. Observable (D3):
/// `GET /assets/styles.css` serves the new bytes on the next request.
///
/// Falsifiability: without the resolved-`@import` watch registration, the
/// symlinked dep edit is observed by nobody and `/assets/styles.css` stays
/// stale until timeout.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "pending fix: #1284"]
async fn e2e_transitive_css_import_refreshes_stylesheet() {
    todo!(
        "Wave-3: add styles/styles.css with `@import './tokens.css';` and a symlinked \
         workspace `@import '@scope/design-system';`, spawn `zfb dev`, edit tokens.css \
         AND the symlinked dep's real file, poll `GET /assets/styles.css` until both \
         new declarations appear."
    );
}

/// SYMPTOM C acceptance — a NEW utility class added inside a component is
/// emitted into `/assets/styles.css` WITHOUT touching the CSS entry. Observable
/// (D3): `GET /assets/styles.css` contains the generated rule for the new class
/// (e.g. `gap-x-hgap-2xs`, `xl:grid-cols-[2.35fr_1fr]`) on the next request.
///
/// Falsifiability: without the Module→`mark_css` re-scan AND `src/` in the scan
/// roots, the class never enters the content scan and the stylesheet never
/// gains the rule.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "pending fix: #1284"]
async fn e2e_new_utility_class_in_component_is_emitted() {
    todo!(
        "Wave-3: edit a component to add a previously-unused utility class \
         (e.g. `gap-x-hgap-2xs`), do NOT touch the CSS entry, poll \
         `GET /assets/styles.css` until the generated rule for that class appears."
    );
}
