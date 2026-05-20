# #347 — Emit `routes.json` at build end + expose `ctx.routes[].prerender`

Research + implementation branch: `research/347-routes-json-manifest`
Tracking: #358
Source issue: #347

## 1. Question — restate

Issue #347 scopes two related gaps together because they share one schema decision:

- **Gap 1.** zfb's plugin API exposes the postBuild route manifest in-memory as `ctx.routes`, but a non-plugin consumer (a script wired into `pnpm build`) cannot read the same data without authoring a zfb plugin. The fix is to emit a `routes.json` to disk at build end so any consumer script (sitemap, OGP indexer, search shard builder) can read it without learning the plugin API.
- **Gap 2.** `ZfbRouteEntry` had no `prerender: boolean` field, so the canonical sitemap example in `docs/src/content/docs/concepts/plugins.mdx` filters only on `extension === "html"` and silently over-includes SSR routes. The fix is to add `prerender` to every entry and update the sitemap example to also filter `r.prerender !== false`.

Both gaps depend on the same on-disk + in-memory schema, so splitting them risks the on-disk file shipping without `prerender` and then breaking compat to add it.

## 2. What was tried

Findings doc is shipped alongside an implementation. Concrete actions:

1. Audited `ZfbRouteEntry` in TypeScript (`packages/zfb/src/plugins.ts:40`) and `PostBuildRouteEntry` in Rust (`crates/zfb-build/src/plugin_runner.rs:124`). Confirmed both already carry a `prerender: bool` field — that part of Gap 2 was landed earlier on `main` by commit `f053186 feat(plugins): expose prerender on postBuild route manifest` (a sibling of #262). The gap that remained was the **docs side**: the sitemap example in `concepts/plugins.mdx` was still filtering on extension only.
2. Read the postBuild manifest construction site at `crates/zfb/src/commands/build.rs::build_post_build_manifest` (around line 1577) to confirm the in-memory and on-disk surfaces can be the same byte representation — the function sorts by `url` for byte-stable output (#262 AC), and the per-entry `prerender` boolean is already resolved from `build_prerender_map` for static, statically-expanded, and runtime-expanded routes.
3. Confirmed `zfb_build::atomic_write_string` is already exported (`crates/zfb-build/src/atomic.rs:83`, re-exported at `crates/zfb-build/src/lib.rs:68`) and is the standard write primitive used elsewhere in the build for sitemap-/manifest-class outputs.
4. Implemented on-disk emission at `crates/zfb/src/commands/build.rs::emit_routes_manifest_file` — a small helper that takes the same `PostBuildRouteManifest` already built for the postBuild context, serialises it with `serde_json::to_string_pretty`, appends a trailing `\n`, and atomically writes to `<outDir>/__zfb/routes.json`.
5. Wired the helper into the postBuild stage of `crates/zfb/src/commands/build.rs::run` so the emit runs **before** the postBuild plugin loop fires (so plugins can re-read the file if they want, and so a postBuild plugin failure doesn't suppress the emit). Default-on; opt out via `emitRoutesManifest: false`.
6. Added a `Config::emit_routes_manifest: Option<bool>` field to `crates/zfb/src/config.rs` + matching `emitRoutesManifest?: boolean` field to `packages/zfb/src/config.ts`. Both default to `None` / `undefined`, which the emit call interprets as `true` (`config.emit_routes_manifest.unwrap_or(true)`).
7. Updated `docs/src/content/docs/concepts/plugins.mdx`:
   - Added the `prerender: boolean` line to the `ZfbRouteEntry` reference table (the prior session was likely raced against the f053186 docs commit — present in code, missing in docs).
   - Added a one-paragraph callout below the table describing SSG vs SSR semantics and the `r.prerender !== false` filter pattern.
   - Added an "On-disk access — `dist/__zfb/routes.json`" subsection documenting the on-disk file, the shape, the `emitRoutesManifest: false` opt-out, and the "two access shapes, one source of truth" framing.
   - Updated the worked-example sitemap plugin to filter `r.extension === "html" && r.prerender !== false`.
8. Added three new unit tests in `crates/zfb/src/commands/build.rs::tests` covering schema shape, byte-stability across runs, and SSG-vs-SSR pinning.
9. Ran `cargo check -p zfb --tests`, `cargo test -p zfb --lib emit_routes_manifest`, `cargo test -p zfb-build --lib`, and `pnpm typecheck` in `packages/zfb/`. All pass.

## 3. Evidence — schema decision + implementation snippets + test results

### 3.1 Frozen schema (on-disk = in-memory)

**Decision: one shape, two access surfaces.** The on-disk `routes.json` is the exact serialised form of the in-memory `ctx.routes` (`zfb_build::PostBuildRouteManifest`). No extra wrapping, no envelope version field, no build-id metadata — adding any of those would make the on-disk file diverge from the in-memory one and force two type contracts to evolve in lockstep.

```jsonc
{
  "routes": [
    {
      "url": "/",                               // string — emitted URL path
      "output": "index.html",                   // string — path under outDir
      "extension": "html",                      // string — file extension
      "source": "pages/index.tsx",              // string — project-root-relative source module
      "prerender": true                         // bool — true = SSG; false = SSR (no on-disk artifact)
      // "params" omitted for static routes
    },
    {
      "url": "/api/me",
      "output": "api/me/index.html",
      "extension": "html",
      "source": "pages/api/me.tsx",
      "prerender": false
    },
    {
      "url": "/blog/hello/",
      "output": "blog/hello/index.html",
      "extension": "html",
      "source": "pages/blog/[slug].tsx",
      "prerender": true,
      "params": { "slug": "hello" }             // scalar string OR string[] (catchall)
    }
  ]
}
```

Field-by-field rationale:

- `url`, `output`, `extension`, `source`, `params` — already locked by #262 for the in-memory shape. Reusing them is non-negotiable because the two surfaces share the same producer code path (`build_post_build_manifest`).
- `prerender: boolean` — added because the existing sitemap example silently over-includes SSR routes. A boolean is enough; we do not need an enum (`"ssg" | "ssr" | "isr"`) yet because zfb only has the two modes today. If a third mode lands later, the field is a strict superset (the JSON bool widens to a string union without re-arranging the rest).
- No top-level `version` field. We accept the v0.x breakage policy stated in the issue body. If a future major version reshapes the schema we add a `schemaVersion` then. Adding it pre-emptively would invite plugins to write defensive version checks against a field that has only ever had one value.

### 3.2 Feature flag default

**Decision: default-on.** Reasoning:

- The file is small (~tens of KB even on a 1000-page site), single-write, deterministic, and lives in a clearly-internal `__zfb/` directory. The cost of always-on is near zero.
- The dominant use case the issue documents — sitemap generation, OGP indexing, search shard builders — is "I want the file to exist by default so my CI script can read it without ceremony." Default-off would push every consumer to first edit `zfb.config.ts`, which defeats the "do not learn the plugin API" motivation.
- For projects that strip everything except shipped assets out of `dist/` before deploy, `emitRoutesManifest: false` is a one-line opt-out.

The implementation interprets `Option<bool>::None` as `true`:

```rust
if config.emit_routes_manifest.unwrap_or(true) {
    emit_routes_manifest_file(&outdir, &route_manifest)
        .context("failed to emit dist/__zfb/routes.json")?;
}
```

This is the standard zfb pattern for "default-on with explicit override," matching how `trailing_slash` and `site` are handled elsewhere in the same `Config` struct.

### 3.3 Directory choice — `__zfb/`, not `.zfb/`

The issue body offered `dist/.zfb/routes.json` or `dist/__zfb/routes.json`. Picked `__zfb/` because:

- The `__zfb/` prefix matches the common "framework-private" directory convention (Next.js `_next/`, Astro `_astro/`, Nuxt `_nuxt/`) — visible in directory listings but lexicographically grouped together, and obviously non-content.
- It pairs with the existing JS-side `globalThis.__zfb` runtime namespace, so the source-of-truth for "this is internal zfb metadata" reads the same on disk and in memory.
- `.zfb/` would be hidden on POSIX `ls` and is a footgun for static hosts that ignore dot-prefixed paths during deploy (Netlify, S3 with default ACLs).
- No existing zfb precedent constrains the choice — searched the repo for `.zfb/`, `__zfb/`, and adjacent patterns and found no production usage (only `globalThis.__zfb` for the runtime namespace, which reinforces the choice).

### 3.4 Implementation snippet — emit helper

```rust
const ROUTES_MANIFEST_REL_PATH: &str = "__zfb/routes.json";

fn emit_routes_manifest_file(
    outdir: &Path,
    manifest: &zfb_build::PostBuildRouteManifest,
) -> Result<()> {
    let dest = outdir.join(ROUTES_MANIFEST_REL_PATH);
    let mut json = serde_json::to_string_pretty(manifest)
        .context("serialise postBuild route manifest to JSON")?;
    json.push('\n');
    zfb_build::atomic_write_string(&dest, &json)
        .with_context(|| format!("failed to write {}", dest.display()))?;
    Ok(())
}
```

Wired into the build at `commands/build.rs::run`, immediately **before** the postBuild plugin loop fires — so the file lands even if a postBuild plugin later errors, and so plugins themselves can re-read the file if they want a single I/O path.

### 3.5 Tests

Three new unit tests in `crates/zfb/src/commands/build.rs::tests`:

- `emit_routes_manifest_writes_documented_schema` — pins the field set, asserts `prerender` is a JSON boolean (not stringified), asserts `params` is omitted for static routes and present for dynamic ones, and pins the on-disk path to exactly `<outdir>/__zfb/routes.json`.
- `emit_routes_manifest_is_byte_stable_across_runs` — emits the same manifest twice to two different outdirs and asserts byte-equal output. Mirrors the #262 in-memory byte-stability guarantee for the on-disk surface and pins the trailing-newline contract.
- `emit_routes_manifest_preserves_ssg_and_ssr_entries` — pins that a mixed manifest (one SSG, one SSR, one dynamic-with-params) round-trips with `prerender` set correctly on each.

Run results:

```
$ cargo test -p zfb --lib emit_routes_manifest
running 3 tests
...
test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 240 filtered out

$ cargo test -p zfb-build --lib
running 199 tests
test result: ok. 199 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out

$ pnpm --filter @takazudo/zfb typecheck
(no output, tsc --noEmit exits 0)
```

The 199-test zfb-build pass is the important one — it includes `post_build_route_manifest_preserves_prerender_field` and `post_build_route_manifest_includes_prerender_field`, which are the in-memory counterparts of my new on-disk tests and would have surfaced any drift between the two surfaces.

### 3.6 Docs change

The plugins.mdx update has three parts:

1. The `ZfbRouteEntry` interface gains the `prerender: boolean` line so the reference table matches the TS / Rust types.
2. A short paragraph immediately below the table explains the SSG / SSR semantics and the `r.prerender !== false` filter pattern so the next reader doesn't repeat the original sitemap-over-include bug.
3. A new "On-disk access — `dist/__zfb/routes.json`" subsection documents the on-disk file with an inline JSON example, the `emitRoutesManifest: false` opt-out, and the "two access shapes, one source of truth" framing.
4. The worked-example sitemap plugin's filter is updated from `r.extension === "html"` to `r.extension === "html" && r.prerender !== false`, with a one-sentence comment explaining what each half of the predicate covers.

## 4. Conclusion

**Recommended schema:** one shape, two access surfaces. On-disk `routes.json` is the serialised form of `ctx.routes` byte-for-byte. Fields: `url`, `output`, `extension`, `source`, `prerender`, optional `params`. No envelope, no schema version, no build metadata. Sorted by `url`. Pretty-printed JSON with trailing newline.

**Recommended feature-flag default:** default-on (`emitRoutesManifest: undefined` ⇒ `true`). Opt out with `emitRoutesManifest: false` in `zfb.config.ts`. Cost of always-on is near zero; the issue's motivating use cases all assume the file exists by default.

**Recommended directory:** `<outDir>/__zfb/routes.json`. The `__zfb/` prefix matches `_next/` / `_astro/` precedent, pairs with the existing `globalThis.__zfb` runtime namespace, and avoids the dot-prefix footgun on static hosts.

**Confidence: high.**

- The in-memory side of this contract is already locked by #262 and battle-tested — I am only choosing to write it down byte-for-byte.
- The `prerender` field already exists across TS and Rust (landed on `main` by `f053186` before this branch started); my changes only carry it through to docs + the new on-disk surface.
- All three new tests pass; the existing 199-test zfb-build suite passes; TypeScript `tsc --noEmit` passes.
- Default-on is reversible (`emitRoutesManifest: false`); default-off would be effectively irreversible because consumers would write CI scripts assuming the file might not exist.

## 5. Follow-ups

- **Tracking issue note.** Once #358 (the docs-philosophy tracking issue this research feeds into) lands, link the "On-disk access" subsection from the cookbook recipes catalog as the prerequisite data substrate.
- **Schema version field — deferred, not declined.** When the on-disk surface acquires a second consumer outside the zfb workspace (a published third-party tool that reads it from CI), we will want a `schemaVersion: 1` top-level field so that tool can refuse incompatible files. Track in a follow-up issue if/when that consumer materialises.
- **Integration test.** A black-box integration test that runs `zfb build` against a fixture project with one static + one SSR + one dynamic route and asserts the on-disk JSON would lock the surface end-to-end. Not added here because the unit tests already cover the producer; flag for a future hardening pass.
- **Plugin example update.** The `plugins.mdx` worked example could be augmented with a parallel "from a `package.json` script" example showing the same sitemap generation from a `pnpm build && node scripts/sitemap.mjs` setup reading `dist/__zfb/routes.json`. Deferred to the cookbook recipes epic.
- **Default-on telemetry.** Worth confirming after a real-world build that the file size is in the expected single-digit-KB-per-100-routes range. Not a blocker because the file is just `serde_json::to_string_pretty(manifest)`; size grows linearly with route count.

## 6. Scope exceptions

Every file touched on this branch. Listed for code reviewers (per the codex note flagging this branch as implementation-heavy):

| File | Why it changed |
| --- | --- |
| `crates/zfb/src/commands/build.rs` | Added `emit_routes_manifest_file` helper, wired it into `run`, added three unit tests (`emit_routes_manifest_writes_documented_schema`, `…_is_byte_stable_across_runs`, `…_preserves_ssg_and_ssr_entries`), added the `ROUTES_MANIFEST_REL_PATH` constant. No changes to existing functions. |
| `crates/zfb/src/config.rs` | Added `emit_routes_manifest: Option<bool>` to `Config` with doc comments + `Default` impl. Additive only. |
| `packages/zfb/src/config.ts` | Added `emitRoutesManifest?: boolean` to `ZfbConfig` with TSDoc mirroring the Rust comment. Additive only. |
| `docs/src/content/docs/concepts/plugins.mdx` | Added the `prerender: boolean` line to the `ZfbRouteEntry` reference table, added the "On-disk access — `dist/__zfb/routes.json`" subsection, and updated the sitemap worked-example filter. |
| `research/347-routes-json-manifest.md` | This file. |

**No** changes to:

- `crates/zfb-build/` — the in-memory `PostBuildRouteEntry` already had `prerender: bool` and `PostBuildRouteManifest` is `Serialize`; nothing in the producer crate needed editing.
- `crates/zfb-types/`, `crates/zfb-render/`, `crates/zfb-router/`, etc. — no route-type changes propagated.
- `packages/zfb/src/plugins.ts` — `ZfbRouteEntry` already exposed `prerender: boolean` from the `f053186` commit on `main`.
- `packages/zfb-runtime/` — runtime package re-exports the type; the re-export picks up the existing field without a re-export edit.
- Any built `dist/` artefact in `packages/*` — `packages/zfb` consumes its TS sources directly (`"main": "./src/index.ts"`); there is no separate built output to regenerate.

## 7. Compatibility Notes

Reviewer focus per the codex flag was higher-bar tests + compatibility notes. Summary:

### What's strictly additive

- `Config::emit_routes_manifest` / `ZfbConfig.emitRoutesManifest` are new optional fields. Existing `zfb.config.ts` files load unchanged and get default-on behaviour.
- The new on-disk file `dist/__zfb/routes.json` is a new artefact at a new path. It does not collide with any existing build output (verified by searching the workspace for `__zfb/` references; the only matches are the `globalThis.__zfb` runtime namespace and the new code in this branch).
- The new tests are additive; no existing tests changed.

### What's technically a minor breaking change (v0.x acceptable per issue body)

- The `prerender: boolean` field on `ZfbRouteEntry` / `PostBuildRouteEntry` is a breaking change for any plugin that destructures the entry exhaustively, e.g. `const { url, output, extension, source, params } = entry; if (Object.keys(entry).length !== 5) throw …`. The issue body explicitly accepts this for v0.x. This branch did not introduce that break — it was landed earlier by `f053186` — but the docs change in this branch is the **first** place the field is publicly documented, so it is reasonable to call the docs landing the breakage point.
- Searched the worktree for plugin examples / fixtures that destructure `ZfbRouteEntry`: only the documentation worked example, which this branch updates to be `prerender`-aware. No other in-tree consumer breaks.

### What does NOT break

- Consumers that read `ctx.routes.routes` and project to a subset of fields (the common case) are unaffected.
- Consumers that ignore unknown fields (the conventional TypeScript / JSON pattern) are unaffected.
- Consumers that already filter by `r.extension === "html"` keep working; they would over-include SSR routes the same way they did before, but the on-disk file existing changes nothing about that behaviour. They opt in to the stricter filter on their next docs read.
- Static-host deploys: `__zfb/` is shipped under `dist/` so it ends up in production deploys by default. This is intentional — see "Open questions" in the issue body — and consistent with how Next.js ships `_next/_buildManifest.json` in `.next/`. Hosts that want to strip the file set `emitRoutesManifest: false`.

### Deprecation / addition policy callout

The issue body asks whether docs need a deprecation/addition policy callout. Recommendation: not in this branch. The plugins.mdx page already documents the v0.x stability posture; adding a sentence specifically about `ZfbRouteEntry` additions would either echo what's there or set a precedent (per-type stability notes) we are not ready to commit to across the rest of the API. If a future addition feels riskier, document it then.
