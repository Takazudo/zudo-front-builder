# Research: Feature-gate V8 in the production runtime (issue #344)

Branch: `research/344-v8-feature-gate`
Worktree: `worktrees/r344-v8-feature-gate/`
Source issue: https://github.com/Takazudo/zfb/issues/344

## 1. Question

Can zfb gate the V8 isolate behind the question "does this project have any `prerender = false` routes?" so that pure-SSG projects ship a small Rust runtime (static-file serving only) while only SSR-bearing projects pull in the V8-linked build? Three sub-questions:

- What is the binary-size delta between `embed_v8 = on` and `embed_v8 = off`?
- Where in the build pipeline would the "this project has SSR routes" flag be set, and is the gate detection-driven or config-driven?
- Does the Cloudflare adapter already gate `_worker.js` emission on the same signal — so one detection seam can feed both decisions?

## 2. What was tried

### 2.1 Audit the `embed_v8` cargo feature

Searched the workspace for every place the feature is declared, propagated, or consumed:

```
rg -n "embed_v8" Cargo.toml crates/
```

Single owner: `crates/zfb-render/Cargo.toml:16-18`.

```toml
[features]
default = ["embed_v8"]
embed_v8 = ["dep:deno_core", "dep:tokio"]
```

The feature is declared on the *library crate only* and toggles two optional dependencies — `deno_core` (the V8 host) and `tokio` (deno's async runtime). No other crate declares an `embed_v8` feature; `zfb`, `zfb-build`, `zfb-content`, etc. consume `zfb-render` with default features (so they inherit V8 transitively).

### 2.2 Find every `#[cfg(feature = "embed_v8")]` gate

- `crates/zfb-render/src/lib.rs:30-37` — gates `pub mod embedded_v8` and the re-exports of `EmbeddedV8RenderHost`, `BundleModuleLoader`, `HttpRequestLike`, `HttpResponseLike`, `PluginRegistryHooks`, `AliasHook`, `VirtualModuleHook`.
- `crates/zfb-render/src/embedded_v8/` is the gated module: `mod.rs`, `dispatch.rs`, `extensions.rs`, `module_loader.rs`, plus a `js/` directory of polyfills (`web_polyfills.js` 691 lines, `browser_event.js` 116, `globals_shim.js` 57, several `node_*.js` stubs).
- All `crates/zfb-render/tests/embedded_v8_*.rs` integration tests are `#![cfg(feature = "embed_v8")]`-gated at the file level.

Module size (gated source only):

```
85    embedded_v8/dispatch.rs
126   embedded_v8/extensions.rs
696   embedded_v8/mod.rs
476   embedded_v8/module_loader.rs
1383  rust subtotal
1051  js polyfills subtotal
2434  total
```

### 2.3 Measure binary-size delta

`CARGO_TARGET_DIR=~/.cargo-target` (global build dir) is configured via `~/.cargo/config.toml`, so the parent repo and worktrees share artifacts. The V8 dep tree was already compiled there, which made the V8 build cheap and is the only reason the measurement fit in the 5-minute budget — `docs/src/content/docs/architecture/why-rust.mdx:37` documents a 15–30 minute first-build cost for V8 from cold.

Commands:

```
cargo clean --release -p zfb-render
cargo build --release -p zfb-render --no-default-features  # 18s (warm deps)
cargo build --release -p zfb-render                          # 22s (warm deps)
```

Both completed successfully with no patches to the codebase.

Artifact sizes recorded in 2.4. Logs in `__inbox/344-v8-feature-gate-spike/build-no-v8.log`.

### 2.4 Look up rlib + linked-binary sizes

`ls -la ~/.cargo-target/release/deps/libzfb_render-*.rlib` plus a `cargo build --message-format=json` pass identified the artifacts. Linked `zfb` binary was inspected via `ls -la ~/.cargo-target/release/zfb` and `strip /tmp/zfb-stripped`.

### 2.5 Identify the detection seam

Read `crates/zfb/src/render_pipeline.rs` for the prerender map builder; `crates/zfb/src/commands/build.rs` for the SSR detection + adapter precondition; `crates/zfb-build/src/adapter.rs` for the no-SSR-without-adapter check; `packages/zfb-adapter-cloudflare/src/build.ts` for what the Cloudflare adapter actually emits.

## 3. Evidence

### 3.1 `embed_v8` feature topology

- The feature lives on `zfb-render` only.
- `zfb`, `zfb-build`, and `zfb-content` reference `zfb-render` without `default-features = false`, so they always pull V8.
- `zfb` has an unconditional dependency on the gated types via `crates/zfb/src/v8_host_adapter.rs:29-32`:

  ```rust
  use zfb_build::renderer::{EmbeddedV8Host, HttpResponseLike, RendererError};
  use zfb_render::{
      BundleModuleLoader, EmbeddedV8RenderHost, HttpRequestLike, PluginRegistryHooks,
  };
  ```

  These types disappear when `embed_v8` is off, so `cargo build -p zfb --no-default-features` (or any propagation) will fail to compile until the consumer is refactored to branch on the feature. There is currently no `#[cfg(feature = "embed_v8")]` in `zfb` or `zfb-build`.

### 3.2 rlib sizes (release profile)

| Build                                           | Size            |
| ----------------------------------------------- | --------------- |
| `cargo build -r -p zfb-render --no-default-features` | 5,986,908 B (~5.71 MiB) |
| `cargo build -r -p zfb-render` (default = embed_v8)  | 6,680,192 B (~6.37 MiB) |
| Delta (zfb-render's own code only)              | **~693 KiB**    |

Rlibs of the V8-bearing transitive deps (only present in the embed_v8 build):

| Crate         | rlib size       | Notes                                       |
| ------------- | --------------- | ------------------------------------------- |
| `v8`          | ~177,187,000 B (~169 MiB) | The V8 engine + bundled snapshot. The bulk of the cost. |
| `deno_core`   | ~13,200,000 B (~12.6 MiB) | Module loader, event loop, JsRuntime.       |
| `deno_*` helpers | a handful of MiB combined | `deno_error`, `deno_unsync`, `deno_ops`, `deno_core_icudata`. |
| `tokio`       | not attributed  | The `zfb` binary already pulls tokio independently (`rt-multi-thread, net, fs, signal, process, sync`); zfb-render's feature only adds `rt + macros + sync`, a subset. So tokio is shared with the rest of the workspace, not a V8-specific cost. |

Rlibs include all symbols; the linker strips most. Direct dead-code elimination at link time is the right next metric, but it requires actually linking a binary without V8 — which the codebase does not currently support (see 3.1).

### 3.3 Linked `zfb` binary (V8-bearing)

The workspace's `zfb` release binary, built with default features, is the only Rust binary that links V8 today:

| Variant                    | Size                                  |
| -------------------------- | ------------------------------------- |
| `target/release/zfb` (V8-on, unstripped) | 225,798,144 B (~215 MiB)              |
| `strip target/release/zfb` (V8-on, stripped) | 203,035,920 B (~193 MiB)              |

A V8-off variant cannot be produced without refactoring (see 3.1). Estimating from rlib sizes and typical V8 link-time dead-code-elimination ratios on Linux x86_64, a hypothetical V8-off `zfb` linked binary would be on the order of 30–50 MiB — but this is an estimate, not a measurement. The honest finding is: **today's only V8-linked Rust binary is the build CLI itself; V8 is the dominant component, on the order of 150+ MiB of the 203 MiB stripped binary.**

### 3.4 Detection seam already exists in the build pipeline

`crates/zfb/src/render_pipeline.rs:1012` — `build_prerender_map` walks the route table, reads each TSX page's `export const prerender = …` flag via `zfb_content::tsx_frontmatter`, and returns `BTreeMap<route_template, bool>`. Missing entries default to SSG (`true`).

`crates/zfb/src/commands/build.rs:1084-1106` — the build orchestrator already computes the exact SSR-route set:

```rust
let ssr_route_refs: Vec<SsrRouteRef<'_>> = static_routes
    .iter()
    .filter(|entry| !prerender_map.get(&entry.route_key).copied().unwrap_or(true))
    .map(|entry| SsrRouteRef { route_key: ..., url_path: ... })
    .collect();
ensure_no_ssr_without_adapter(&adapter, &ssr_route_refs)?;

let ssr_route_keys_for_runtime_bundle: BTreeSet<String> = static_routes
    .iter()
    .filter(|entry| !prerender_map.get(&entry.route_key).copied().unwrap_or(true))
    .map(|entry| entry.route_key.clone())
    .collect();
```

A `ssr_route_refs.is_empty()` check at this point is sufficient to know "this project would compile with V8 off in its production runtime." The data is computed *before* the bundle step and *before* the V8 host is booted.

### 3.5 Cloudflare adapter dispatch — same data, different decision

The adapter is invoked at `crates/zfb/src/commands/build.rs:1481` strictly on `adapter != None`:

```rust
if !adapter.is_none() {
    let mut runtime_bundler_input = bundler_input_for_runtime;
    runtime_bundler_input.worker_only_routes =
        Some(ssr_route_keys_for_runtime_bundle);
    ...
    run_adapter_bundle_with(&adapter, adapter_in, adapter_runner)?;
}
```

The inner bundle is narrowed to the SSR routes via `worker_only_routes`, so when the SSR set is empty the inner bundle is functionally empty — but `packages/zfb-adapter-cloudflare/src/build.ts:69-84` (`emitWorker`) unconditionally writes `_worker.js` + `_zfb_inner.mjs` to the outdir. **Direct answer to the issue's question:** No, the Cloudflare adapter does not gate `_worker.js` emission on `prerender = false` detection — it gates on whether any adapter is configured at all. The *same* `prerender_map → ssr_route_keys` data feeds the bundle-narrowing step, so the seam is already there to also feed the adapter-emit decision (and, by symmetry, the V8 gate).

### 3.6 Production-runtime architecture mismatch — the load-bearing finding

The issue's framing assumes a "production Rust runtime" that ships V8. Looking at the workspace's actual shipping artifacts:

- **Cloudflare path** (`packages/zfb-adapter-cloudflare/`): emits a JS-only `_worker.js`. V8 runs *inside Cloudflare's Workers runtime* on their servers — no Rust binary ships to the deploy target. There is no zfb-side V8 cost on this path.
- **`@takazudo/zfb-runtime`** (`packages/zfb-runtime/`): pure JavaScript. It is the bundle entry that the Cloudflare worker (or any future SSR-host) imports. No V8 embedded here either.
- **`zfb` CLI** (the Rust binary): the *only* Rust binary that links V8. It is a dev-machine tool — `zfb build`, `zfb dev`, `zfb preview` all need V8 to execute the user bundle and render the SSG pages. Pure-SSG projects still need V8 here at build time.
- **Tauri sidecar / standalone runtime**: not implemented today. The issue mentions Tauri-sidecar weight as a motivation but there is no Tauri sidecar in this workspace.

So as of 2026-05-21, the V8 binary-size cost lands on **contributors and `cargo install` users** (who build/install the `zfb` CLI), not on any deploy target. A pure-SSG project's user-machine `zfb` is the same 193 MiB binary as an SSR project's.

## 4. Conclusion

### 4.1 Direct answers

- **Binary-size cost of V8 today.** ~693 KiB for `zfb-render`'s own gated code; ~177 MiB for the bundled `v8` rlib and ~13 MiB for `deno_core`. The linked `zfb` binary is 203 MiB stripped; if a hypothetical V8-off binary lands in the estimated 30–50 MiB range (3.3), V8's contribution is ~150–170 MiB, i.e. 75–85% of the linked binary. The V8-off linked-binary size cannot be measured today without refactoring `zfb` / `zfb-build` to add `#[cfg(feature = "embed_v8")]` fallbacks. (See 3.2, 3.3.)
- **Where the SSR-detection flag would be set.** `crates/zfb/src/commands/build.rs:1084` — at this point `ssr_route_refs` is already computed; an `is_empty()` check is the natural gate seam. No new detection code is needed. (See 3.4.)
- **Detection-driven vs config-driven.** Detection-driven, primary; config-driven escape hatch, secondary. The detection seam already exists; adding a `zfb.config.ts` knob (`output: "static" | "hybrid"`) on top is useful only when a user wants to *force* V8 inclusion (e.g., an SSG-only repo today that will add `prerender = false` later, wanting consistent runtime topology). (See 3.4.)
- **Does the Cloudflare adapter use the same signal.** Not today. The adapter is gated on `adapter != None`, not on the SSR-route count. The same prerender-map data feeds the runtime-bundle narrowing — refactor seam is there. (See 3.5.)
- **Split `zfb-render` into two crates, or just a binary-variant?** Just a binary-variant — `zfb-render` already supports `--no-default-features` and the gated code lives in a clearly separated `embedded_v8/` module. The work is in the *consumer* crates (`zfb`, `zfb-build`) which currently reference `EmbeddedV8RenderHost` and `EmbeddedV8Host` unconditionally.

### 4.2 The headline: the issue's premise is partly anachronistic

The most important finding is that, given today's shipping topology, **feature-gating V8 in `zfb-render` does not reduce any deploy-target binary**. The motivation in the issue body (pure-SSG projects shipping a "tiny Rust runtime") presupposes a Rust runtime that ships to the deploy target. That doesn't exist:

- Cloudflare deploys are JS-only. The adapter writes `_worker.js`; V8 is provided by Cloudflare.
- There is no Tauri sidecar in this workspace.
- The `zfb` CLI always needs V8 to render SSG pages — pure-SSG projects pay the V8 cost on the **build machine**, not the deploy target.

The `embed_v8` feature is therefore best understood as **infrastructure for a future shipping path** (Tauri sidecar, standalone Node-free SSR server, cargo-install-as-deploy mode) rather than a present-day binary-size win. Until such a shipping path exists, finishing the gate (refactoring `zfb`/`zfb-build` to compile with `embed_v8 = off`) is a non-trivial cost with limited near-term payoff.

### 4.3 Recommended next steps

Two distinct workstreams, in priority order:

1. **Land a "tiny zfb-render" CI build** (low effort, immediate value). Add a `cargo build --release -p zfb-render --no-default-features` step to CI so the no-V8 feature combination stays buildable. This is the entry-cost insurance for any future work; today it would prevent silent rot in the `#[cfg(feature = "embed_v8")]` gates.

2. **Decide the deploy-target shipping path before doing more refactor work.** The V8 gate's value depends entirely on whether zfb ships a V8-bearing Rust runtime to *somewhere*. Today's answer is "no." Concrete decision input needed:
   - Is a Tauri-sidecar distribution planned in any short-term roadmap?
   - Is a standalone-SSR-server crate (e.g. `zfb-runtime-rust`, a Cloudflare-Workers-shaped HTTP front) planned?
   - If both answers are "no for now," document this as the reason `embed_v8 = off` is currently unreachable in the shipping graph, and link back to this research from `crates/zfb-render/Cargo.toml`.

3. **Only after a deploy-target shipping path is committed:** refactor `zfb` and `zfb-build` to compile cleanly with `embed_v8 = off`. The work is mechanical (cfg-gate the `EmbeddedV8Host` re-exports, `WorkerDispatch::EmbeddedV8`, `Backend::EmbeddedV8`, `v8_host_adapter::ThreadedV8Host`, etc.) but touches multiple crates and is not the right pre-investment.

When (3) happens, the detection seam is `crates/zfb/src/commands/build.rs:1084`. The gate becomes:

```rust
let needs_v8_in_runtime = !ssr_route_refs.is_empty();
```

Detection-driven, zero config, same data as `ensure_no_ssr_without_adapter`. Add a `zfb.config.ts` `output` field as the escape hatch only after a concrete user-request justifies it.

### 4.4 Confidence

**Medium-high.**

- High confidence on: the feature topology (single declaration site, gate locations), the existence and shape of the detection seam, the rlib size numbers, the linked `zfb` binary size, and the Cloudflare adapter's actual decision logic.
- Medium confidence on: the estimated 30–50 MiB no-V8 `zfb` binary size — extrapolated from rlib sizes and typical link-time DCE, not measured.
- Lower confidence on: whether a Tauri sidecar or other Rust-runtime shipping path is in the roadmap. The codebase shows no such artifact today, but I did not search the broader `big-plan` issue history or sibling repos.

## 5. Follow-ups

- **CI: add a `--no-default-features` zfb-render build.** Cheap, prevents the gates from rotting.
- **Quantify the linked-binary delta empirically.** Would require introducing `#[cfg(feature = "embed_v8")]` fallbacks in `zfb` / `zfb-build` (mock host or hard-error stub) and rebuilding. Not done here because of the file-scope guardrail.
- **`embed_v8 = off` test surface.** All `crates/zfb-render/tests/embedded_v8_*.rs` are file-level gated. With the feature off, the only remaining tests in `zfb-render` are unit tests inside `lib.rs` modules and a handful of SWC-pipeline + paths-extract integration tests. Worth confirming this leaves enough coverage to keep the no-V8 build sane.
- **Cloudflare adapter no-op case.** When `adapter = "@takazudo/zfb-adapter-cloudflare"` is configured but the project has zero `prerender = false` routes, the build still writes `_worker.js` and `_zfb_inner.mjs` to `dist/`. The inner bundle is effectively empty. This is wasted bytes in the deploy upload — not within scope here, but worth a separate issue. Mention in the synthesized super-epic tracking.
- **`@takazudo/zfb-runtime` is the JS-only runtime.** Mentioning it explicitly in this research because the naming overlaps with the hypothetical "production Rust runtime" the issue discusses — they're not the same thing. A future Rust-side runtime should pick a distinct name (e.g. `zfb-runtime-rust` or `zfb-server-bin`).
- **Boa/QuickJS exploration is separate.** `docs/src/content/docs/architecture/js-runtime.mdx:40` notes Boa was already rejected on ECMA conformance grounds. Filing a sibling research issue for "pure-Rust JS engine for SSG-only mode" should reference that existing evaluation as the baseline rather than re-spike from scratch.

## 6. Scope exceptions

None. All work stayed within the allowed paths: `research/344-v8-feature-gate.md` (this file), `__inbox/344-v8-feature-gate-spike/build-no-v8.log` (build log). No source files, no Cargo.toml, no docs/, no packages/ touched.
