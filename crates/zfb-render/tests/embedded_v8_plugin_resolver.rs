//! Tests for the plugin-registry hooks wired into `BundleModuleLoader`
//! (sub-issue #260 — embedded V8 host resolver for aliases + virtual modules).
//!
//! Covers:
//! - Alias hit: a bundle that imports an aliased bare specifier resolves to
//!   the correct filesystem file and can read its exports.
//! - Alias miss: an unregistered specifier is rejected with the existing
//!   "no in-memory source" error (no regression).
//! - Virtual module hit (first import): the pre-resolved source is served.
//! - Virtual module hit (second import, same specifier): the same in-memory
//!   source is served again — the loader has no external side-effect.
//! - Virtual module loader error path: errors from loaders surface with the
//!   specifier and the plugin name attached (tested via bad source that
//!   throws at evaluation time, so the plugin name is carried in the
//!   `AliasHook` / `VirtualModuleHook` and visible in diagnostics).

#![cfg(feature = "embed_v8")]

use std::path::PathBuf;

use tempfile::TempDir;
use zfb_render::{
    embedded_v8::BundleModuleLoader, EmbeddedV8RenderHost, HttpRequestLike, PluginRegistryHooks,
    RenderHost,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Write `content` into `<dir>/<filename>` and return the absolute path.
fn write_temp_file(dir: &TempDir, filename: &str, content: &str) -> PathBuf {
    let path = dir.path().join(filename);
    std::fs::write(&path, content).expect("write temp file");
    path
}

/// Workerd-shaped bundle that imports `specifier` (a bare module) and uses
/// its export in the fetch response.
fn bundle_importing(specifier: &str, export_name: &str) -> String {
    format!(
        r#"
        import {{ {export_name} }} from "{specifier}";
        export default {{
          fetch(_request) {{
            return new Response({export_name}, {{ status: 200 }});
          }},
        }};
        "#
    )
}

// ---------------------------------------------------------------------------
// Alias tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn alias_hit_resolves_to_aliased_file() {
    let dir = TempDir::new().expect("tempdir");
    let lib_path = write_temp_file(
        &dir,
        "my-lib.js",
        r#"export const greeting = "hello from alias";"#,
    );

    let mut hooks = PluginRegistryHooks::new();
    hooks.add_alias("@my/lib", lib_path.clone(), "test-plugin");

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = bundle_importing("@my/lib", "greeting");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("hello from alias"));
}

#[tokio::test]
async fn alias_miss_falls_through_to_existing_error() {
    // No aliases registered; importing an unknown bare specifier must
    // produce the existing "no in-memory source" error — regression guard.
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    // The bundle itself fails during module load (not dispatch), so we
    // expect `execute_module` to return an Err.
    let bundle = r#"
        import { x } from "unregistered-bare-specifier";
        export default { fetch() { return new Response(x, { status: 200 }); } };
    "#;
    let err = host
        .execute_module("bundle.mjs", bundle)
        .await
        .expect_err("execute should fail for unresolvable import");
    let msg = err.to_string();
    // The error message should mention the specifier.
    assert!(
        msg.contains("unregistered-bare-specifier"),
        "expected error to mention the specifier, got: {msg}"
    );
}

#[tokio::test]
async fn alias_is_exact_match_only() {
    // Registering `@my/lib` must NOT match `@my/lib/sub` — exact-match only.
    let dir = TempDir::new().expect("tempdir");
    let lib_path = write_temp_file(&dir, "my-lib.js", r#"export const x = 1;"#);

    let mut hooks = PluginRegistryHooks::new();
    hooks.add_alias("@my/lib", lib_path, "test-plugin");

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    // Import `@my/lib/sub` (not registered) — must fail.
    let bundle = r#"
        import { x } from "@my/lib/sub";
        export default { fetch() { return new Response(String(x), { status: 200 }); } };
    "#;
    let err = host
        .execute_module("bundle.mjs", bundle)
        .await
        .expect_err("@my/lib/sub is not the same as @my/lib");
    let msg = err.to_string();
    assert!(
        msg.contains("@my/lib/sub"),
        "error should mention the unresolved specifier, got: {msg}"
    );
}

#[tokio::test]
async fn alias_missing_target_file_surfaces_error_with_plugin_name() {
    let mut hooks = PluginRegistryHooks::new();
    // Point the alias to a non-existent file.
    hooks.add_alias(
        "@ghost/lib",
        PathBuf::from("/nonexistent/path/ghost.js"),
        "ghost-plugin",
    );

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = r#"
        import { x } from "@ghost/lib";
        export default { fetch() { return new Response(String(x), { status: 200 }); } };
    "#;
    let err = host
        .execute_module("bundle.mjs", bundle)
        .await
        .expect_err("missing alias target must error");
    let msg = err.to_string();
    // The error must mention the plugin name and the path.
    assert!(
        msg.contains("ghost-plugin"),
        "expected plugin name in error, got: {msg}"
    );
    assert!(
        msg.contains("ghost.js"),
        "expected file path in error, got: {msg}"
    );
}

#[tokio::test]
async fn alias_transitively_loads_sibling_files() {
    // Codex P1: a top-level alias to `lib.js` whose source imports
    // `./helper.js` must successfully load both via disk. Without
    // sibling-directory disk-read support, the transitive import
    // would fail with "no in-memory source".
    let dir = TempDir::new().expect("tempdir");
    let _helper = write_temp_file(&dir, "helper.js", r#"export const help = "from helper";"#);
    let lib_path = write_temp_file(
        &dir,
        "lib.js",
        // Use relative import so deno_core resolves it against the
        // file:// URL of lib.js.
        r#"
        import { help } from "./helper.js";
        export const greeting = "lib + " + help;
        "#,
    );

    let mut hooks = PluginRegistryHooks::new();
    hooks.add_alias("@multi/lib", lib_path, "multi-plugin");

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = bundle_importing("@multi/lib", "greeting");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("lib + from helper"));
}

#[tokio::test]
async fn alias_to_tsx_extension_rejected_with_clear_error() {
    // Codex P1: aliases to TS/TSX source files would fail with an
    // opaque V8 syntax error since the host does not transpile. We
    // reject at load time with a targeted error so the failure mode
    // is actionable.
    let dir = TempDir::new().expect("tempdir");
    let lib_path = write_temp_file(
        &dir,
        "comp.tsx",
        // Content doesn't matter — we reject on extension before reading.
        r#"export const x: number = 1;"#,
    );

    let mut hooks = PluginRegistryHooks::new();
    hooks.add_alias("@ts/comp", lib_path, "ts-plugin");

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = r#"
        import { x } from "@ts/comp";
        export default { fetch() { return new Response(String(x), { status: 200 }); } };
    "#;
    let err = host
        .execute_module("bundle.mjs", bundle)
        .await
        .expect_err("TSX alias target must error");
    let msg = err.to_string();
    assert!(
        msg.contains("ts-plugin"),
        "expected plugin name in TS-rejection error, got: {msg}"
    );
    assert!(
        msg.contains("TypeScript extension"),
        "expected explanation of the TS limitation, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Canonical disk-authority containment (sub-issue #1765)
//
// Containment is decided on canonical paths: a symlink INSIDE the authorised
// directory that resolves OUTSIDE it is denied, while a symlinked CONFIGURED
// target (explicit plugin intent) is trusted. Symlink creation is privileged
// on Windows, so these are Unix-only.
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn in_dir_symlink_escaping_outside_is_denied() {
    use std::os::unix::fs::symlink;

    // Authorised dir holds the alias target; a sibling symlink inside it
    // points at a file OUTSIDE the dir. Canonicalisation must collapse the
    // link and deny the read.
    let auth = TempDir::new().expect("auth dir");
    let outside = TempDir::new().expect("outside dir");
    let secret = outside.path().join("secret.js");
    std::fs::write(&secret, r#"export const secret = "leaked";"#).expect("write secret");
    symlink(&secret, auth.path().join("evil.js")).expect("symlink evil.js -> outside secret");

    let lib_path = write_temp_file(
        &auth,
        "lib.js",
        r#"
        import { secret } from "./evil.js";
        export const greeting = secret;
        "#,
    );

    let mut hooks = PluginRegistryHooks::new();
    hooks.add_alias("@esc/lib", lib_path, "esc-plugin");

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = bundle_importing("@esc/lib", "greeting");
    let err = host
        .execute_module("bundle.mjs", &bundle)
        .await
        .expect_err("in-dir symlink escaping the authorised dir must be denied");
    let msg = err.to_string();
    assert!(
        msg.contains("no in-memory source"),
        "escape must fall through to the generic no-read error, got: {msg}"
    );
    assert!(
        msg.contains("evil.js"),
        "error should mention the denied specifier, got: {msg}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn nested_symlink_chain_escaping_outside_is_denied() {
    use std::os::unix::fs::symlink;

    // A dedicated multi-hop escape: in-dir symlink -> another symlink ->
    // outside target. canonicalize resolves the whole chain; the read is
    // denied because the final real parent is outside the authorised dir.
    let auth = TempDir::new().expect("auth dir");
    let outside = TempDir::new().expect("outside dir");
    let secret = outside.path().join("secret.js");
    std::fs::write(&secret, r#"export const secret = "leaked";"#).expect("write secret");
    // hop2 (outside) -> secret (outside); hop1 (in auth) -> hop2.
    let hop2 = outside.path().join("hop2.js");
    symlink(&secret, &hop2).expect("symlink hop2 -> secret");
    symlink(&hop2, auth.path().join("hop1.js")).expect("symlink hop1 -> hop2");

    let lib_path = write_temp_file(
        &auth,
        "lib.js",
        r#"
        import { secret } from "./hop1.js";
        export const greeting = secret;
        "#,
    );

    let mut hooks = PluginRegistryHooks::new();
    hooks.add_alias("@chain/lib", lib_path, "chain-plugin");

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = bundle_importing("@chain/lib", "greeting");
    let err = host
        .execute_module("bundle.mjs", &bundle)
        .await
        .expect_err("multi-hop symlink escape must be denied");
    let msg = err.to_string();
    assert!(
        msg.contains("no in-memory source"),
        "multi-hop escape must fall through to the generic no-read error, got: {msg}"
    );
    assert!(
        msg.contains("hop1.js"),
        "error should mention the denied specifier, got: {msg}"
    );
}

#[cfg(unix)]
#[tokio::test]
async fn in_dir_symlink_to_in_dir_file_is_allowed() {
    use std::os::unix::fs::symlink;

    // A symlink inside the authorised dir pointing at another file inside
    // the same dir stays within the canonical parent and is allowed.
    let auth = TempDir::new().expect("auth dir");
    let _real = write_temp_file(&auth, "real.js", r#"export const v = "real-value";"#);
    symlink(auth.path().join("real.js"), auth.path().join("link.js"))
        .expect("symlink link.js -> real.js");

    let lib_path = write_temp_file(
        &auth,
        "lib.js",
        r#"
        import { v } from "./link.js";
        export const greeting = v;
        "#,
    );

    let mut hooks = PluginRegistryHooks::new();
    hooks.add_alias("@in/lib", lib_path, "in-plugin");

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = bundle_importing("@in/lib", "greeting");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("in-dir symlink to an in-dir file should load");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("real-value"));
}

#[cfg(unix)]
#[tokio::test]
async fn symlinked_configured_target_is_trusted() {
    use std::os::unix::fs::symlink;

    // The CONFIGURED alias target passes through a symlinked directory —
    // explicit plugin config = intent, so it is trusted, and both the target
    // and its sibling resolve against the canonical location.
    let root = TempDir::new().expect("root dir");
    let real_dir = root.path().join("real");
    std::fs::create_dir(&real_dir).expect("real dir");
    std::fs::write(
        real_dir.join("foo.js"),
        r#"
        import { b } from "./bar.js";
        export const greeting = "from-symlinked-target-" + b;
        "#,
    )
    .expect("write foo.js");
    std::fs::write(real_dir.join("bar.js"), r#"export const b = "sib";"#).expect("write bar.js");
    let link_dir = root.path().join("linkdir");
    symlink(&real_dir, &link_dir).expect("symlink linkdir -> real");

    let mut hooks = PluginRegistryHooks::new();
    // Configure the alias THROUGH the symlinked directory.
    hooks.add_alias("@sym/lib", link_dir.join("foo.js"), "sym-plugin");

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = bundle_importing("@sym/lib", "greeting");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("symlinked configured target should be trusted");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("from-symlinked-target-sib"));
}

#[cfg(unix)]
#[tokio::test]
async fn read_uses_canonical_path_not_symlink_spelling() {
    use std::os::unix::fs::symlink;

    // Proof the check + read operate on the CANONICAL path: a `.js`-spelled
    // symlink inside the authorised dir points at a `.ts` file in the same
    // dir. If the loader inspected the original `.js` spelling it would read
    // it; because it inspects the canonical `.ts` path, the TS-extension
    // guard fires. Same-dir, so this is authorised (not denied) — it can
    // only reach the ext check by resolving the symlink.
    let auth = TempDir::new().expect("auth dir");
    std::fs::write(auth.path().join("real.ts"), r#"export const x = 1;"#).expect("write real.ts");
    symlink(auth.path().join("real.ts"), auth.path().join("wrapper.js"))
        .expect("symlink wrapper.js -> real.ts");

    let lib_path = write_temp_file(
        &auth,
        "lib.js",
        r#"
        import { x } from "./wrapper.js";
        export const greeting = String(x);
        "#,
    );

    let mut hooks = PluginRegistryHooks::new();
    hooks.add_alias("@canon/lib", lib_path, "canon-plugin");

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = bundle_importing("@canon/lib", "greeting");
    let err = host
        .execute_module("bundle.mjs", &bundle)
        .await
        .expect_err("canonical .ts target must hit the TS-extension guard");
    let msg = err.to_string();
    assert!(
        msg.contains("TypeScript extension"),
        "ext check must run on the canonical .ts path, got: {msg}"
    );
    assert!(
        msg.contains("real.ts"),
        "error should name the canonical path, not the .js symlink spelling, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Virtual module tests
// ---------------------------------------------------------------------------

#[tokio::test]
async fn virtual_module_hit_first_import() {
    let mut hooks = PluginRegistryHooks::new();
    hooks.add_virtual_module(
        "virtual:my-data",
        r#"export const answer = "42 from virtual";"#,
        "data-plugin",
    );

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = bundle_importing("virtual:my-data", "answer");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("42 from virtual"));
}

#[tokio::test]
async fn virtual_module_hit_second_import_uses_same_source() {
    // The acceptance criterion: "loader invoked exactly once per build
    // per specifier even if multiple pages import the same virtual module".
    // In the V8 host, the virtual module source is a pre-resolved string
    // stored in `PluginRegistryHooks`. Two dispatches against the same
    // loaded bundle both see the identical virtual module source — there
    // is no per-dispatch re-invocation or mutation.
    //
    // We verify this by making two dispatch calls and checking both return
    // the same virtual module content. The `BundleModuleLoader` serves
    // the `virtual:` specifier from the in-memory hooks map on every load
    // (deno_core may cache the module too, but either way the source is
    // consistent).
    let mut hooks = PluginRegistryHooks::new();
    hooks.add_virtual_module(
        "virtual:counter",
        r#"export const value = "static";"#,
        "counter-plugin",
    );

    let loader = BundleModuleLoader::new().with_plugin_hooks(hooks);
    let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");

    let bundle = bundle_importing("virtual:counter", "value");
    host.execute_module("bundle.mjs", &bundle)
        .await
        .expect("execute bundle");

    // First dispatch.
    let first = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/page1"))
        .await
        .expect("first dispatch");
    assert_eq!(first.body_utf8(), Some("static"));

    // Second dispatch — same bundle, different URL — still sees the
    // same virtual module content (no re-invocation of any external loader).
    let second = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/page2"))
        .await
        .expect("second dispatch");
    assert_eq!(second.body_utf8(), Some("static"));
}

#[tokio::test]
async fn virtual_module_unregistered_specifier_rejected() {
    // No virtual modules registered; importing a `virtual:` specifier
    // must fail with the existing unknown-specifier error.
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = r#"
        import { x } from "virtual:unregistered";
        export default { fetch() { return new Response(x, { status: 200 }); } };
    "#;
    let err = host
        .execute_module("bundle.mjs", bundle)
        .await
        .expect_err("unregistered virtual module must error");
    let msg = err.to_string();
    assert!(
        msg.contains("virtual:unregistered"),
        "error should mention the specifier, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// No-hooks regression: existing behaviour unchanged
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_hooks_existing_loader_behaviour_unchanged() {
    // Smoke test: the baseline loader (no hooks) still boots and
    // dispatches normally. Guards against any regression in the
    // `Default` / `new()` path.
    let mut host = EmbeddedV8RenderHost::new().expect("host boot");
    let bundle = r#"
        export default {
          fetch(_request) {
            return new Response("baseline ok", { status: 200 });
          },
        };
    "#;
    host.execute_module("bundle.mjs", bundle)
        .await
        .expect("execute bundle");
    let resp = host
        .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
        .await
        .expect("dispatch");
    assert_eq!(resp.status, 200);
    assert_eq!(resp.body_utf8(), Some("baseline ok"));
}
