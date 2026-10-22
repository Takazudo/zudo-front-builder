# Phase 1 host contract

> Phase 2 status: the actual CLI integration is on `main` (merged by
> #4368 from `feat/4301-direct-v8-cli`), and since #4390 `direct-v8` is the default
> feature with `embed_v8` as the source-build rollback until Gate B; see [CLI.md](CLI.md)
> for selection, ownership and current verification status. The phase-1 observations below remain historical
> and source-bound; an old hold is neither a current implementation claim nor
> proof of phase-2 acceptance.

This is the behavior inventory for [#4235](https://github.com/Takazudo/zudo-front-builder/issues/4235)
and [#4236](https://github.com/Takazudo/zudo-front-builder/issues/4236), frozen from
`main` at `6fb3fbe0103c5c90799c3262ca5c1ab9ade0f27e` on 2026-10-14. The epic and
children #4236–#4240 were read; they had no comments at intake. Source symbols
below refer to that baseline, even when extraction subsequently moves their
implementation. They are anchors for semantics, not candidate verification.

**S** means supported by existing code; **L** means an intentional existing
limitation; **A** means an observable boundary needs a baseline execution probe
or has conflicting documentation. A named existing test is an independent
assertion to retain, not a claim that it ran in this session. **New proof** marks
a required prototype observation not adequately covered by an existing test.
Record execution results separately in `FINDINGS.md`, keyed by these stable IDs.

## Scope, sources, and dependency inventory

The baseline pins `deno_core = "=0.399.0"` in
[`zfb-render/Cargo.toml`](../../crates/zfb-render/Cargo.toml); `Cargo.lock` resolves
`v8 147.4.0`. The candidate must use exactly that engine, with the API checked
against [rusty_v8 v147.4.0](https://github.com/denoland/rusty_v8/tree/v147.4.0).
Only an explicit, non-default feature enables direct V8. At most one unpublished
internal crate separates shared services from the existing compiler/render
graph. The normal CLI, config selector, esbuild pipeline, Node plugin host, and
production Worker execution stay unchanged.

| ID | Status | Boundary and fixed expectation | Source / observer |
| --- | --- | --- | --- |
| GRAPH-01 | S | Exact candidate executable/target/features resolve no `deno_core`, `deno_ops`, `deno_error`, `serde_v8`, or Deno runtime extensions. Workspace lockfile may retain baseline packages. Actual native link and execution required; no subprocess interpreter fallback. | New proof: resolved metadata/tree plus native engine receipt; #4237. |
| GRAPH-02 | S | Shared services/shims need no engine feature. Existing default, no-V8, md-Wasm and CSS-addon isolation remains intact. | Existing feature boundaries in Cargo manifests; manager build/check/load evidence required by #4240. |
| NATIVE-01 | S | One process-wide V8 initialization owner; one owning thread per isolate/context; no unsafe `Send`/`Sync`, local handles or borrowed buffers crossing awaits. Native callback failures become errors, never unwinds through C++. | `ThreadedConfigEvaluator`, `ThreadedV8Host`, `EmbeddedV8RenderHost`; independent lifetime review and real finite create/drop proof. |

Direct Deno integration use sites at the frozen baseline:

| Location | Coupling to replace or preserve behind the baseline adapter |
| --- | --- |
| `zfb-render/src/config_eval/runtime.rs` | `JsRuntime`, runtime options, module loader/source/specifier/error types, `mod_evaluate`, event loop, module namespace, V8 scope and `TryCatch`. |
| `zfb-render/src/embedded_v8/mod.rs` | Runtime/extension construction; module IDs/cache, side/main evaluation; promise/event-loop coordination; rooted values; `serde_v8` for dispatch/props/exports; `JsBuffer`; diagnostic formatting and cancellation drain. |
| `embedded_v8/module_loader.rs` | `ModuleLoader`, `ModuleSource`, resolution/error types, `deno_core::resolve_import`, found-URL redirects. Authorization and source selection are ZFB policy, not engine behavior. |
| `embedded_v8/fetch.rs` | `op2`, extension registration, `OpState`, async op eager polling, `JsBuffer`/`ToJsBuffer`, `CancelHandle`/`CancelFuture`, `JsErrorBox`. HTTP policy/transport stays canonical. |
| `embedded_v8/crypto.rs`, `crypto/digest.rs` | Sync op macros/registration/state and `JsErrorBox`; OS entropy and SHA computation stay canonical. |
| `embedded_v8/js/web_polyfills.js` | Access to four fixed ops and custom error-class registration through `Deno.core`; replace with a ZFB bridge, not a fake Deno namespace. |
| `embedded_v8/js/globals_shim.js` | Relies on the preinstalled minimal console and forwards to its original methods while capturing logs. |
| `zfb-render/Cargo.toml` | Optional Deno dependencies under `embed_v8`; no `deno_web`/`deno_fetch` runtime extensions. |

The remaining baseline Deno-name occurrences in production are explanatory
comments/re-exports in `config_eval/mod.rs`, `embedded_v8/extensions.rs`,
`render_host.rs`, `zfb/src/{config,v8_host_adapter}.rs`, and feature-isolation
comments in `zfb`, `zfb-build`, `zfb-content`, `zfb-islands`, and `zfb-md-wasm`
Cargo manifests. Tests directly using Deno APIs occur in `embedded_v8/{mod.rs,
crypto/tests.rs,crypto/digest.rs,fetch/tests.rs,js_fetch_tests.rs,
js_crypto_tests.rs}`. The integration tests `embedded_v8_smoke.rs`,
`embedded_v8_plugin_resolver.rs`, and
`zfb-build/tests/integration_e2e_routing_rendering.rs` also describe Deno
identity/threading. These baseline-specific controls may retain Deno; they must
not become candidate dependencies. Inventory method: `rg` over Rust, JS and
Cargo manifests for `deno_core|deno_ops|deno_error|serde_v8|Deno\.core` followed
by inspection of the actual callers below.

## Callers and bootstrap

| ID | Status | Required behavior / independent expectation | Source / existing observer |
| --- | --- | --- | --- |
| CALL-01 | S | The raw config API takes already bundled ESM and returns JSON. Each `ThreadedConfigEvaluator::eval_bundle` creates its own thread/current-thread Tokio runtime/isolate; the caller uses `spawn_blocking`. | `zfb-render/src/config_eval/mod.rs`; `tests/config_eval.rs::test_thread_pinned_two_sequential_calls`, `test_spawn_blocking_does_not_starve_tokio_workers`. |
| CALL-02 | S | `zfb-config-loader::load_from_ts_file` bundles via esbuild neutral/ESM/esnext, aliases `zfb/config` and `@takazudo/zfb/config` to the data stub, then invokes CALL-01. Plugin module resolution is outside V8 and optional via load options. | `zfb-config-loader/src/loader.rs::load_ts_via_inprocess_v8`, `resolve_plugins_from_value`; callers `zfb/src/config.rs::load_from_ts_file` and `zfb-server/src/embed.rs` config loading. |
| CALL-03 | S | `RenderHost` is `?Send`: evaluate module, call default with JSON props, get named JSON export. `call_default` supports direct/promise result and JS string conversion. Unknown handle/non-callable default/failed conversion fail. Named export conversion is the existing `serde_v8` behavior, not config's stringify contract. | `zfb-render/src/render_host.rs`; `embedded_v8/mod.rs` trait implementation. Baseline source scan finds no production calls to these three trait methods outside the implementation; tests use them. Candidate fixed-JSON representative needed, not a general serde clone. |
| CALL-04 | S | Production uses `zfb-build::EmbeddedV8Host: Send` and its factory; the adapter sends owned request/result data to a pinned thread. Legacy dispatch is GET/build-time; full request is also build-time unless explicit mode is supplied. | `zfb-build/src/renderer.rs::{EmbeddedV8Host,EmbeddedV8HostFactory,RendererState}`; `zfb/src/v8_host_adapter.rs::{ThreadedV8Host,serve_v8_host_requests}`. |
| CALL-05 | S | Build/dev prerender and lazy SSG remain build-time. Only `ssr_adapter.rs` explicitly passes request-time. Dev content-trace uses the build-time full-request method even on the same host. | `zfb/src/{render_pipeline,ssr_adapter,lazy_render_adapter}.rs`, `commands/{build,dev}.rs`; renderer mode-recording tests and `embedded_v8_dispatch_mode.rs`. |
| BOOT-01 | S | Render order: fixed native services/bootstrap primitives, web polyfills, browser-event globals, then globals shim with nonce/limits substitutions, then user modules. Config installs web polyfills only, without render `__zfb` or browser-event shim. | `EmbeddedV8RenderHost::bootstrap_host_shim`, `evaluate_config_bundle`; `config_eval.rs::test_h_url_pathname_accessible`, `embedded_v8_browser_event.rs::module_top_level_extends_event_loads`. |
| BOOT-02 | S | Supply `queueMicrotask` and minimal console before shared shims. They are host bootstrap behavior; `queueMicrotask` is not a bare ECMAScript/V8 global despite a shim comment calling it “pure-V8/host builtin.” Promises and intrinsic `WebAssembly` remain engine-provided. | `web_polyfills.js` timer/MessagePort implementation; `globals_shim.js` original-console capture; new proof: queued callback and message execute after current turn. |
| BOOT-03 | S/L | Canonical shims provide Headers/Request/Response, URL/URLSearchParams, TextEncoder/Decoder, atob/btoa, structuredClone, AbortController/Signal, crypto, MessageChannel/Port, and microtask-backed setTimeout/clearTimeout. Render also provides Event/CustomEvent/EventTarget and `__zfb.ssrDebug = true`. Preserve implementations, not assumed browser completeness. | `embedded_v8/extensions.rs`, `js/{web_polyfills,browser_event,globals_shim}.js`; existing smoke/browser-event tests. |
| BOOT-04 | L | Timer delay is ignored; clearTimeout is a no-op. MessagePort start/close are no-ops; postMessage delivers the existing data reference via microtask, not a new structured-clone/transfer API. No real timer scheduler is authorized. | `web_polyfills.js::{setTimeout,clearTimeout,MessagePort}`; new representative needed. |

`Deno.core` and its error registration/op table are incidental baseline plumbing,
not a supported ZFB application API. The replacement bridge needs only fetch,
fetch cancel, random-byte fill, digest, error construction/registration glue and
bootstrap console/microtask primitives. Rust owns authority and operation state.
Do not introduce a generic op registry, Deno emulation, another HTTP/TLS stack,
or another web-platform library to supply these operations.

## Configuration: raw evaluation versus outer validation

The synthetic raw config module is `file:///zfb-config-eval/main.mjs`.
`ConfigEvalError::NotAPlainObject` is a historical name: the actual guard is
`is_object() && !is_function()`, **not** a prototype/plain-record check.

| ID | Status | Independent expected result | Source / existing observer |
| --- | --- | --- | --- |
| CFG-01 | S | `export default { answer: 42 }` returns `{"answer":42}`; missing default, null, number, string, boolean, undefined, function and Symbol fail the top-level object guard. | `config_eval/runtime.rs::evaluate_in_runtime`; `config_eval.rs::test_a_plain_object_round_trips`, `test_b_non_object_default_export_is_error`; new missing/Symbol cases. |
| CFG-02 | S | Raw arrays and Date objects pass the object guard. `[undefined,()=>{},Symbol(),NaN,Infinity]` stringifies to five null slots. `new Date('2020-01-02T03:04:05Z')` yields the JSON string `"2020-01-02T03:04:05.000Z"`. No added array/prototype rejection. | Same guard and in-isolate stringify; `test_f_date_becomes_iso_string` covers nested Date; new top-level/array proof. |
| CFG-03 | S | Object function/undefined/Symbol-valued properties are omitted; symbol-keyed properties omitted; NaN/Infinity become null. BigInt and cycles throw. Getter/toJSON throws remain evaluation errors with the thrown message. A custom toJSON may return a scalar/null; toJSON returning undefined produces the non-string-result Evaluation error. | `evaluate_in_runtime` calls the isolate's current `globalThis.JSON.stringify` with JSON receiver; `test_e_function_field_silently_dropped`; new edge probes. |
| CFG-04 | S | V8 runs stringify, Rust parses only its returned string. Do not substitute generic V8-to-serde conversion or bypass a user-replaced JSON/stringify. Non-function stringify, non-string return, thrown stringify, malformed returned JSON are separate evaluation failures. | `evaluate_in_runtime`; new intentional tampering probes, preserving error meaning. |
| CFG-05 | S | URL and encoders exist before module evaluation. `process` and `Buffer` remain undefined. No render-only global is required. | `config_eval.rs::{test_h_url_and_text_encoder_types,test_h_no_node_globals_contract_intact}`. |
| CFG-06 | S | Every remaining import except the exact synthetic main URL is rejected: relative, bare, node and absolute/remote specifiers. Bundled authored imports are already code and are not retroactively forbidden. Dynamic import rejection may surface as Evaluation rather than static BareImportRejected; preserve category meaning. | `ConfigModuleLoader::{resolve,load}`, `NO_BARE_IMPORTS_ERROR`; tests `test_c_relative_import_rejected`, `test_g_node_fs_import_rejected`, `test_g_bare_specifier_import_rejected`; new dynamic negative. |
| CFG-07 | S | Parse/load errors, forbidden imports, top-level throw/rejected evaluation and stringify failures remain distinguishable. `throw new Error('boom')` retains `boom`, not empty success. Immediate and resolved-promise top-level await complete. | `ConfigEvalError`, `evaluate_in_runtime`; `test_d_top_level_throw_preserves_message`; new promise/startup failure probes. |
| CFG-08 | S/A | Outer V8 loader returns the raw JSON plus resolved plugin specifiers; it does not add a plain-object guard. `zfb::config::parse_loaded_config` attaches plugin resolutions, processes presets/removed keys and applies typed `Config` deserialization with field-path errors. Preserve this boundary untouched. Do not infer rejection of every array: serde struct sequence behavior needs an explicit baseline probe. | `zfb-config-loader/src/loader.rs::load_ts_via_inprocess_v8`; `zfb/src/config.rs::{parse_loaded_config,take_presets,Config}`. |
| CFG-09 | L/A | The no-V8 Node loader explicitly rejects null/non-object/arrays before serialization. This is an existing path difference, not permission to tighten the raw V8 evaluator or claim slim-build parity. | `zfb-config-loader/js/config-loader.mjs` default-export check; follow-up A-01 below. |

## Module policy, evaluation and Worker dispatch

| ID | Status | Independent expected result | Source / existing observer |
| --- | --- | --- | --- |
| MOD-01 | S | Registered ESM loads/evaluates, imports link by resolved identity, top-level await resolves before namespace use. Syntax errors, missing exports and throws fail. Side module validation followed by wrapper import reuses the evaluated instance. | `load_and_evaluate_{main,side}_module`, `BundleModuleLoader`; smoke `supports_esm_imports_and_top_level_await`; new side-effect counter equals 1 and strict namespace identity. |
| MOD-02 | S/A | Dynamic imports use the same resolver/authorization, not arbitrary disk/network access. Preserve module namespace/cache semantics. `import.meta` is engine-host initialization and needs an explicit baseline probe for URL/main/resolve fields before claiming exact parity; loader source alone does not define them. | Deno module machinery plus `BundleModuleLoader::{resolve,load}`; no dedicated existing dynamic/import.meta assertion found. New fixed identity, import.meta URL and dynamic unknown-import probes required. |
| MOD-03 | S | Source order: enumerated node/internal helper sources, pre-resolved virtual modules, explicitly registered in-memory sources, authorized copied Wasm, authorized aliases, remaining registered sources, then clear unknown-import error. In-memory `.wasm` source wins over disk. | `BundleModuleLoader::load`; `in_memory_wasm_module_takes_precedence_over_bundle_assets`. |
| MOD-04 | S | Exact bare alias matching; no prefix aliases. Canonical configured target is trusted, including configured symlinks. Allow canonical exact target or **same parent directory**, never the entire subtree. In-directory symlink escape is denied. Read the canonical path. | `alias_disk_read_authority`, `read_alias_target`; plugin resolver tests `alias_is_exact_match_only`, `in_dir_symlink_escaping_outside_is_denied`, `nested_symlink_chain_escaping_outside_is_denied`, `read_uses_canonical_path_not_symlink_spelling`. |
| MOD-05 | S | A symlink alias's relative sibling import resolves beside its canonical target; canonical found URL must be retained. Missing configured target or authorized missing sibling reports plugin-attributed read failure. Spaces and percent encoding are URL semantics, not string-prefix authority. | `ok_js_redirected`; tests `symlinked_file_target_relative_import_resolves_beside_canonical`, `symlinked_file_target_does_not_reach_sibling_beside_symlink_spelling`, `missing_sibling_in_authorized_dir_keeps_plugin_attributed_error`; new encoded-name probe. |
| MOD-06 | L | Alias `.ts`, `.tsx`, `.mts`, `.cts` targets reject with precompiled-JS guidance. No transpilation/package/npm resolver in runtime. Canonicalize-then-read does not close concurrent filesystem mutation TOCTOU. | `read_alias_target`; `alias_to_tsx_extension_rejected_with_clear_error`; loader trust-boundary documentation. |
| MOD-07 | S | Plugin host resolves virtual source before host construction, once per specifier/build. Repeated import sees the same prepared source/evaluation. Unknown virtual source fails. | `PluginRegistryHooks`, `find_virtual_module`; `virtual_module_hit_first_import`, `virtual_module_hit_second_import_uses_same_source`, `virtual_module_unregistered_specifier_rejected`. |
| MOD-08 | S/L | Only `node:fs`, `node:fs/promises`, `node:path`, `node:url`, `node:buffer`, `node:async_hooks` and the internal helper are supported stubs. They resolve at import, fail when used, and expose `__zfbThrowNodeStub`; they are not Node compatibility. Unknown node import fails. | `extensions::{NODE_STUB_SPECIFIERS,node_stub_source,ext_node_stubs_source}`, canonical `js/node_*.js`; `embedded_v8_node_stubs.rs`. |
| MOD-09 | S | Copied Wasm imports under synthetic `file:///zfb/` map only to safe relative `.wasm` assets under configured canonical bundle root. Decode before rejecting traversal; deny root escapes, host-bearing/general file URLs and symlink escapes. Missing path named in error; malformed bytes fail compilation. | `resolve_bundle_wasm_asset`, `percent_decode_path`, `is_safe_bundle_asset_relative_path`; Wasm loader tests including `bundle_wasm_import_rejects_encoded_traversal_and_absolute_file_urls`. |
| MOD-10 | S | Wasm default is an **uninstantiated WebAssembly.Module**, not instance exports. Instantiate it in JS and consume a fixed exported result (e.g. 42). | `ok_wasm_module`; `bundle_wasm_import_exports_a_webassembly_module`; new generated copied-Wasm consumption proof in both runners. |
| WORK-01 | S | Strict Worker loading rejects missing/undefined default, non-object/null default, missing fetch and non-callable fetch at boot. Tolerant utility evaluation still returns its module handle even when no Worker shape installs; subsequent dispatch reports install failure. | `execute_worker_module`, `execute_module`, `check_bundle_default_shape`; `embedded_v8_smoke.rs` strict/tolerant tests. |
| WORK-02 | S | Validate unmodified inner Worker separately as a side module; then load dev wrapper as main without re-evaluating inner. Well-shaped wrapper cannot hide malformed inner. Synthetic names percent-encode bundle basename consistently. | `ThreadedV8Host::new_with_dev_content_trace_wrapper`, `inner_bundle_specifier`, `validate_worker_module_shape`; adapter tests; new identical frozen inner/wrapper proof. |
| WORK-03 | S | Construct Request from absolute URL, method, normalized input headers and optional binary bytes; call default.fetch, await response/body, return status, ordered header pairs and copied body bytes. HTTP error status remains an HTTP result, not automatic host infrastructure error. | `globals_shim.js::__zfb.dispatch`, `dispatch_fetch`, `dispatch.rs`; smoke `dispatch_post_with_binary_body_round_trips`, `dispatch_fetch_preserves_ordered_duplicate_set_cookie_headers`. |
| WORK-04 | S | Headers sort/combine ordinary names; repeated set-cookie values remain separate and ordered. String body defaults to text/plain; URLSearchParams to form encoding; ArrayBuffer has no automatic content type; explicit header wins. | Canonical Headers/_Body shims; smoke tests named `ordinary_headers_still_combine_with_comma_separator`, `headers_entries_nonmutating_order_is_sorted_and_combined`, `response_*`. |
| WORK-05 | S | Preserve rendered content/route snapshot inputs through existing generated Worker dispatch, including exported paths/content and CSS/SDK/module preparation. Do not invent production call-export APIs beyond CALL-03. | `render_pipeline.rs`, `embedded_v8_real_bundle_smoke.rs`, `integration_routing_rendering.rs`; generated differential fixture required by #4240. |

## Async services, authority and cancellation

| ID | Status | Independent expected result | Source / existing observer |
| --- | --- | --- | --- |
| HTTP-01 | S | Build-time and config network denied before socket allocation. Direct native bridge calls and replacing `__zfb` cannot elevate authority. Rust mode is authoritative; absent mode denies. Preserve exact SSG diagnostic. | `fetch::current_dispatch_mode`, `op_zfb_fetch`, `DispatchMode::default`; JS fetch tests `build_time_fetch_is_denied_with_the_byte_identical_ssg_message_and_no_socket`, `a_build_time_dispatch_cannot_reach_the_network_through_the_raw_op`, `replacing_the_zfb_bridge_cannot_select_the_request_time_branch`. |
| HTTP-02 | S | Every started operation captures its initiating dispatch's mode/client/budget. Fresh `Rc<SubrequestCounter>` per dispatch; never reset an old shared counter. Pending A retains A's budget while B gets its own. Do not cancel every orphan merely because response completed. | `begin_dispatch_subrequest_budget`, `state_handles`, `SubrequestCounter`; transport `the_subrequest_budget_is_per_dispatch_and_counts_every_request`; new A/B native temporal proof. |
| HTTP-03 | S | JS authorized dispatch frames/closure nonce prevent forged mode selection; unauthorized nested dispatch inherits mode and never republishes stale mode. Last authorized mode remains for orphan continuations until next dispatch; module evaluation resets both views to build-time. | `install_dispatch_mode`, `reset_dispatch_mode_for_evaluation`, globals shim; `the_mode_outlives_its_dispatch_for_the_calls_that_dispatch_orphaned`, `a_floating_unauthorised_dispatch_cannot_republish_a_stale_mode`, dispatch-mode tests. |
| HTTP-04 | S | Async reqwest/rustls, no blocking sockets or busy polling. Delayed response permits sibling microtask progress and parallel fetch progress. One native future completion/rejection releases its resolver/data exactly once. | `perform_fetch`, baseline async op; new native delayed/parallel/failure probes and lifetime review. |
| HTTP-05 | S | HTTP/HTTPS only; private/loopback deliberately allowed. Methods uppercased. GET/HEAD with non-null body reject before socket. Request-body allowed shapes: string, ArrayBuffer/view, URLSearchParams, null. Stream bodies reject. | `parse_and_check_scheme`, shared fetch shim; `get_and_head_with_a_body_are_rejected_before_any_socket`, transport `disallowed_schemes_are_rejected_before_any_socket_is_opened`. |
| HTTP-06 | S | Redirects exactly 301/302/303/307/308. 301/302 POST and 303 non-GET/HEAD become GET/drop body plus body headers. 307/308 preserve method/body. Cross-origin strips credentials. Manual returns unchanged/redirected=false; error rejects; 300/304 ordinary responses. Each outgoing hop spends a slot. | `perform_fetch_inner`, `rewrite_method_for_redirect`; transport `method_rewriting_follows_the_contract_table`, `status_300_and_304_are_ordinary_responses_in_every_redirect_mode`, `cross_origin_redirect_strips_authorization_and_cookie`, `every_redirect_hop_claims_a_subrequest_slot`. |
| HTTP-07 | S/L | Drop host, connection, transfer-encoding, content-length, upgrade, keep-alive, proxy-authenticate, proxy-authorization, te, trailer. Do not add accept-encoding or automatically decompress. Return binary bytes, duplicate headers, final URL and redirected. statusText is canonical reason, not server's custom phrase. | `sanitized_request_headers`, `read_response`, reqwest features; transport header/duplicate tests. |
| HTTP-08 | S | Canonical Rust limits: 20 redirects, 30,000 ms host fetch deadline, 104,857,600-byte request/response caps, 50 subrequests, 65,536-byte random-call quota. JS reads frozen injected values; Rust checks independently. Deadline env accepts positive integer (trimmed), ignores invalid/zero with warning. | `limits.rs`, `FetchConfig`; limits unit tests; `js_visible_limits_are_the_rust_constants`, `bundle_code_cannot_move_the_js_side_request_body_cap`. |
| HTTP-09 | S | Request cap before body copy/socket; response cap both declared length and actual streamed count. Deadline spans redirect chain plus body buffering; caller can narrow but never extend it. Body-limit/deadline failure drops transport. | `perform_fetch`, `read_response`, `effective_timeout_ms`; transport cap/deadline tests and `a_host_deadline_is_not_relabelled_as_the_callers`. |
| CANCEL-01 | S | Pre-aborted signal rejects without a socket, preserving reason (else named AbortError). Mid-flight abort signals native cancellation before JS rejection, drops transport and unregisters id. Unknown/settled cancel is harmless; cancel itself grants no capability. | `op_zfb_fetch_cancel`, `CancelRegistry`, shared abort/fetch shims; `an_already_aborted_signal_rejects_without_opening_a_socket`, `a_settled_fetch_unregisters_its_cancellation_handle`. |
| CANCEL-02 | S | Abort followed immediately by returning Response must close the loopback socket **while host remains alive and idle**. A JS rejection, host drop, later dispatch, runner kill or timer expiry is insufficient proof. | `drain_cancelled_fetches`; `an_abort_mid_body_cancels_the_transport_and_closes_the_socket`; explicit candidate temporal proof mandatory. |
| CANCEL-03 | S | Host timeout versus caller timeout/abort: first completion wins; preserve caller reason and TimeoutError provenance, clean registry/future exactly once. Failure never produces a synthetic successful empty Response. | `perform_fetch`, `FetchError::js_error_class`, fetch shim; `a_signal_timeout_bounds_the_request_and_reports_as_a_timeout_error`, `the_transport_deadline_reaches_js_as_a_timeout_error_with_its_message`; new native race/failure cases. |

The full fixed SSG message is pinned in existing `expected_ssg_denial` tests:
`fetch() called from SSG runtime (url=<url>). The embedded V8 host does not support outgoing network requests during build-time render. Move the data fetch to a build step or a runtime-only branch.`
Transport errors/caps are TypeError; timeout/abort and crypto-specific errors use
the established `.name` on Error, not a newly implemented DOMException platform.
The older [request-time contract](../../research/2013-request-time-capability-contract.md)
remains supporting context; later executable assertions settle stale wording.

## Crypto, diagnostics and lifecycle

| ID | Status | Independent expected result | Source / existing observer |
| --- | --- | --- | --- |
| CRYPTO-01 | S | Random values use OS CSPRNG, synchronously, in both modes. Eligible integer views include clamped and BigInt variants; floats/DataView/non-view reject TypeMismatchError. Byte quota is 65,536; same view identity returned; offsets honored and surrounding bytes unchanged. Empty view succeeds without entropy call. | `crypto::{EntropySource,OsEntropy,fill_random_bytes}`, JS crypto shim; `every_eligible_integer_view_is_accepted_and_the_same_object_comes_back`, `ineligible_views_and_non_views_throw_type_mismatch_error`, quota/zero-view tests; new native offset/backing-store proof. |
| CRYPTO-02 | S | Entropy failure fails closed and writes no successful result; never Math.random fallback. Verify native entropy invocation, not statistical randomness. UUID consumes 16 bytes and has lower-case 8-4-4-4-12 form, version 4 and RFC variant bits. Nonce is 256 OS-CSPRNG bits. | `both_entropy_apis_invoke_the_os_entropy_host_op`, `with_the_host_op_unreachable_both_entropy_apis_throw_and_write_nothing`, `random_uuid_sets_the_version_and_variant_bits_and_is_well_formed`, nonce tests. |
| CRYPTO-03 | S | SHA-1/256/384/512 use canonical Rust digest implementation; names case insensitive, not trimmed; string or `{name}` at JS boundary. Promise<ArrayBuffer> result; view offsets included. SHA-256('abc') = `ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad`. MD5 rejects NotSupportedError with supported set. | `crypto/digest.rs::digest_bytes`; published-vector tests and `digest_matches_the_published_vectors_through_the_js_boundary`. |
| CRYPTO-04 | S/L | timingSafeEqual compares whole equal-length byte spans, unequal lengths throw. Remaining subtle methods exist but reject NotSupportedError; DigestStream constructor throws; CryptoKey is undefined. No new key APIs. | JS crypto shim and named tests in `js_crypto_tests.rs`. |
| DIAG-01 | S | Capture log/info/warn/error/debug, preserving existing passthrough and level prefix. Drain joins with newline and clears; evaluation and failed-dispatch logs remain available. Limits: 1,000 lines, 262,144 UTF-16 units aggregate, 8,192 units per line plus truncation annotation. | `globals_shim.js` capture functions, `drain_console_logs`; `embedded_v8_console_logs.rs` three tests. |
| DIAG-02 | S | Exception message, originating module URL and usable line/column survive. Existing source mapper must reproject a deliberate generated throw to a fixed authored location. Wrapper/bootstrap frames cannot be interpreted as inner-source map coordinates. | `format_js_error`, `format_event_loop_error`, `zfb-render/src/sourcemap.rs`, adapter error handling; smoke `surfaces_v8_stack_with_parseable_frame`; new emitted-map proof. |
| LIFE-01 | S | Repeated finite boot/evaluate/dispatch/drop and recovery using a fresh host have independent globals/module state and clean resource release. Copy response bytes out of V8 backing storage. Error paths do not reply with invented success. | Host ownership, adapter catch/error paths, smoke `isolate_drops_cleanly_on_panic`; new native repeated-cycle/failure proof. |
| LIFE-02 | L/A | No new CPU-bound interruption, hard heap limit, hostile multi-tenant sandbox or guaranteed catastrophic-failure recovery is promised. Unresolved promises need explicit observed behavior; watchdog termination is not clean shutdown. Baseline adapter's bounded join may detach after 10 seconds and is not evidence of isolate teardown. | `ThreadedV8Host::drop`, config evaluator blocking receive/join, adapter catastrophic-recursion test comments; report actual limitations. |
| LIMIT-01 | L | No ReadableStream; response.body is null; blob/formData fail. WebSocket, EventSource and Worker web APIs absent. The term Worker elsewhere means a default.fetch module shape, not global Worker threads. No inspector, npm loader, full Node or full browser platform. | `web_polyfills.js` and #4235 scope. |

## Evidence boundaries and unresolved baseline details

| ID | Required evidence |
| --- | --- |
| EVID-01 | Same frozen modules, maps, copied Wasm, alias files and virtual source bytes for both independent runner processes. Producers finish before hashing. Record actual producer SHA, tools, features and hashes; no invented provenance. |
| EVID-02 | Real current ZFB/esbuild/MDX/CSS/SDK output: owned JSX/MDX/content, paths/dynamic route, CSS module, binary request/header case, consumed Wasm, mapped throw and plugin hooks. Literal/golden expected values supplement baseline agreement. |
| EVID-03 | One bounded Chromium representative uses candidate HTML plus current emitted client assets; assert SSR marker/initial content and a real hydrated interaction. No broad browser/platform matrix. |
| EVID-04 | Exact commands/SHA, selected nonzero test counts, graph receipt, failures/retries and fresh/reused/not-run status. Default/no-V8/native/feature checks belong to manager's serialized heavy lane. Baseline CI is not prototype verification. |
| EVID-05 | Linux x86_64 native execution required. Darwin arm64/x64, Linux arm64 and Windows x64 remain untested unless actually executed; no platform fleet in phase 1. Actual candidate CLI/factory/reload/rollback adoption remains phase 2. |

Named discrepancies/follow-ups (do not silently repair product semantics here):

- **A-01 — raw versus outer config:** raw arrays/Dates are accepted; Node's
  fallback rejects arrays; typed outer Rust behavior depends on deserialization,
  not a plain-record rule. Add explicit baseline sequence/value probes before
  asserting exact outer acceptance. The candidate reproduces raw behavior.
- **A-02 — Deno module conveniences:** explicit dynamic-import namespace identity
  and `import.meta` fields are not pinned by a dedicated existing assertion.
  Execute the baseline and preserve what generated inputs rely on; do not add
  filesystem/package authority to implement `import.meta.resolve`.
- **A-03 — cancellation drain:** baseline source labels its drain as defence in
  depth, not a demonstrated regression: the current shared dispatch shim's
  awaits already drove cancellation in the recorded test. Preserve the temporal
  requirement without claiming that removing the drain was previously red.
- **A-04 — stale comments:** config “plain object” and “Symbol throws” wording
  is broader than actual guards/JSON semantics; mode is deliberately retained
  after dispatch; URL and queueMicrotask comments must not be mistaken for
  built-in V8 web APIs; the render host has fetch/crypto extensions despite an
  older “no extra extensions” comment. Actual code and assertions win.
- **A-05 — liveness:** finite promises/native work can be proved; permanently
  unresolved promises, CPU loops and catastrophic native failure are separate
  limits. A runner killed to recover is a failed/limited row, never cleanup proof.

This contract does not certify implementation completion. Every row needs an
honest result in the delivery findings: pass, fail, existing unsupported,
deferred or not run. Keep #4235–#4240 open, the sole task PR draft, and unrelated
#4234/#4120 untouched. No default switch, release, package-version change,
deployment, preview upload or later migration phase is authorized.
