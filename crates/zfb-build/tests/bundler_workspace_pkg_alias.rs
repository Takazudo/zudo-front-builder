//! Regression test for issue #443 / sub-issue #450.
//!
//! `@/*` tsconfig path aliases must resolve for imports that originate
//! INSIDE a workspace / `node_modules` package, not just for imports
//! that originate inside the host project's own source tree. The fix
//! for this lives in the bundler — the synthetic tsconfig the bundler
//! writes (and esbuild reads via `--tsconfig=`) must apply the user's
//! `compilerOptions.paths` to importers everywhere esbuild walks, not
//! only to importers under the shadow root.
//!
//! The fixture mirrors the user's report on #443:
//!
//! ```text
//! project-root/
//!   tsconfig.json              — paths: { "@/*": ["src/*"] }
//!   src/config/settings.ts
//!   pages/index.tsx            — imports the workspace package's Header
//!   node_modules/@scope/foo/
//!     package.json
//!     src/header.tsx           — imports "@/config/settings"
//! ```
//!
//! The bug shape (pre-fix) is an esbuild "Could not resolve
//! \"@/config/settings\"" error pointing at `src/header.tsx`.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::PathBuf;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Mirror the behaviour of `zfb/src/commands/build.rs::read_tsconfig_paths`:
/// resolve each `compilerOptions.paths` target to an absolute path against
/// the project root (preserving a trailing `/*`). The bundler feeds the
/// resulting map into `BundlerInput::tsconfig_paths`.
fn read_tsconfig_paths_absolute(
    project_root: &std::path::Path,
    paths: &[(&str, &str)],
) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for (key, target) in paths {
        let (prefix, suffix) = match target.rsplit_once("/*") {
            Some((p, "")) => (p, "/*"),
            _ => (*target, ""),
        };
        let abs = project_root.join(prefix);
        let mut s = abs.to_string_lossy().into_owned();
        s.push_str(suffix);
        out.insert(key.to_string(), vec![s]);
    }
    out
}

#[test]
fn workspace_package_resolves_at_alias_via_synthetic_tsconfig() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_workspace_pkg_alias] no esbuild binary available; \
             set ZFB_ESBUILD_BIN, place the binary at \
             crates/zfb/binaries/esbuild/esbuild, or install esbuild on PATH \
             to enable this test. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();

    // Project source layout (host app).
    for d in ["pages", "src/config", "components", "layouts", "content"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    // The alias target — `src/config/settings.ts` reachable via `@/config/settings`.
    fs::write(
        root.join("src/config/settings.ts"),
        "export const siteName = \"zfb-alias-fixture\";\n",
    )
    .unwrap();

    // The workspace package physically lives in a sibling `packages/`
    // directory, OUTSIDE the host project's `node_modules`. This is
    // the pnpm-workspace layout shape from the #443 reproduction: the
    // consumer (`zudo-doc`) has its workspace member at
    // `packages/zudo-doc-v2` and pnpm links it into `node_modules/`
    // with a symlink whose canonical target is the `packages/` path.
    //
    // Crucially the canonical target is NOT inside any `node_modules/`
    // directory — so when esbuild canonicalises (no --preserve-symlinks)
    // and walks upward from `<tmp>/packages/foo/src/header.tsx` looking
    // for a tsconfig, the walk goes through `<tmp>/packages/foo`,
    // `<tmp>/packages`, `<tmp>`, … and never crosses the project root.
    // With --preserve-symlinks ON it walks upward from
    // `<root>/node_modules/@scope/foo/src/header.tsx` instead, which
    // DOES cross the project root.
    let pkg_real_root = tmp.path().join("packages/foo");
    fs::create_dir_all(pkg_real_root.join("src")).unwrap();
    fs::write(
        pkg_real_root.join("package.json"),
        r#"{ "name": "@scope/foo", "version": "0.0.0", "source": "src/header.tsx" }"#,
    )
    .unwrap();
    fs::write(
        pkg_real_root.join("src/header.tsx"),
        r#"
            import { siteName } from "@/config/settings";
            export function Header() {
              return "Header for " + siteName;
            }
        "#,
    )
    .unwrap();

    // Symlink the workspace package into the host project's
    // `node_modules` tree the way pnpm does for workspace members.
    fs::create_dir_all(root.join("node_modules/@scope")).unwrap();
    let pkg_link = root.join("node_modules/@scope/foo");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&pkg_real_root, &pkg_link)
        .expect("symlink workspace package into node_modules");
    #[cfg(not(unix))]
    {
        eprintln!("[bundler_workspace_pkg_alias] non-unix platform: skipping");
        return;
    }

    // Host page imports the workspace package so its module reaches
    // esbuild during bundling.
    fs::write(
        root.join("pages/index.tsx"),
        r#"
            import { Header } from "@scope/foo/src/header";
            export default function Home() {
              return Header();
            }
        "#,
    )
    .unwrap();

    // Build the same `tsconfig_paths` shape `read_tsconfig_paths`
    // produces from a real `tsconfig.json` — absolutised targets.
    let paths = read_tsconfig_paths_absolute(&root, &[("@/*", "src/*")]);

    let mut input = BundlerInput::for_project(
        root.clone(),
        Framework::Preact,
        BundleMode::Production,
        root.join(".zfb-build"),
        None,
    );
    input.tsconfig_paths = paths;
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild);
    // Mirror production wiring: `commands/build.rs::run` sets
    // `node_modules_dir = Some(<project_root>/node_modules)` whenever
    // the project has a node_modules directory. This is the toggle
    // that — post-f2f739e — turns on `--preserve-symlinks` and changes
    // how esbuild walks importers reached through node_modules.
    input.node_modules_dir = Some(root.join("node_modules"));

    let out = bundle(input).expect(
        "bundle must succeed: @/* path alias must resolve when the importer \
         lives inside a workspace / node_modules package (regression vs #443)",
    );

    // Confirm the resolved module ended up in the output bundle.
    assert!(out.bundle_path.exists(), "bundle.mjs should exist");
    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");
    assert!(
        body.contains("zfb-alias-fixture"),
        "the alias target's exported value must appear in the bundle — \
         this confirms `@/config/settings` resolved to `src/config/settings.ts` \
         when imported from inside `node_modules/@scope/foo/src/header.tsx`"
    );
    assert!(
        body.contains("Header for"),
        "the workspace package's header.tsx body must appear in the bundle"
    );

    drop(tmp);
}

/// **Critical discriminator (sub #677).**
///
/// The #443 workspace-pkg shape, but the alias target is a **CSS-module**
/// instead of a plain `.ts` file: `packages/foo` is symlinked into
/// `node_modules/@scope/foo`, and `header.tsx` (which lives INSIDE
/// `node_modules`) imports `@/styles/theme.module.css` — an alias whose
/// target is a `.module.css`.
///
/// This single test proves BOTH halves of the fix at once:
///
/// 1. **No #443 regression** — the `@/*` alias STILL resolves for an
///    importer that lives inside a `node_modules` package (the primary
///    #443 gate). If the dual-target rebase or copy_mode had broken the
///    `--preserve-symlinks`-off resolution path, this would fail with an
///    unresolved-import error.
/// 2. **Transform now seen** — the alias resolves to the SHADOW copy of
///    the `.module.css`, which `rewrite_css_modules_in_shadow` rewrote to a
///    JS class-map shim, so the scoped class string appears in the bundle.
///    Before the fix the alias resolved entirely outside the shadow to the
///    raw `.module.css`, so the scoped class would be absent.
#[test]
fn workspace_pkg_alias_target_is_css_module_resolves_and_transforms() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!(
            "[bundler_workspace_pkg_alias] no esbuild binary available; \
             set ZFB_ESBUILD_BIN. Skipping."
        );
        return;
    };

    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().to_path_buf();

    for d in ["pages", "src/styles", "components", "layouts", "content"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    // The alias target IS a CSS-module — reachable via `@/styles/theme.module.css`.
    let module_css = root.join("src/styles/theme.module.css");
    fs::write(&module_css, ".brand { color: rebeccapurple; }\n").unwrap();

    // Workspace package physically outside node_modules (pnpm-workspace
    // shape) — its `header.tsx` imports the CSS-module via the `@/` alias.
    let pkg_real_root = tmp.path().join("packages/foo");
    fs::create_dir_all(pkg_real_root.join("src")).unwrap();
    fs::write(
        pkg_real_root.join("package.json"),
        r#"{ "name": "@scope/foo", "version": "0.0.0", "source": "src/header.tsx" }"#,
    )
    .unwrap();
    fs::write(
        pkg_real_root.join("src/header.tsx"),
        r#"
            import styles from "@/styles/theme.module.css";
            export function Header() {
              return "brand-class:" + styles.brand;
            }
        "#,
    )
    .unwrap();

    // Symlink the workspace package into node_modules the way pnpm does.
    fs::create_dir_all(root.join("node_modules/@scope")).unwrap();
    let pkg_link = root.join("node_modules/@scope/foo");
    #[cfg(unix)]
    std::os::unix::fs::symlink(&pkg_real_root, &pkg_link)
        .expect("symlink workspace package into node_modules");
    #[cfg(not(unix))]
    {
        eprintln!("[bundler_workspace_pkg_alias] non-unix platform: skipping");
        return;
    }

    fs::write(
        root.join("pages/index.tsx"),
        r#"
            import { Header } from "@scope/foo/src/header";
            export default function Home() {
              return Header();
            }
        "#,
    )
    .unwrap();

    let paths = read_tsconfig_paths_absolute(&root, &[("@/*", "src/*")]);

    let mut input = BundlerInput::for_project(
        root.clone(),
        Framework::Preact,
        BundleMode::Production,
        root.join(".zfb-build"),
        None,
    );
    input.tsconfig_paths = paths;
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild);
    input.node_modules_dir = Some(root.join("node_modules"));

    // The class map the CSS pipeline would produce, keyed on the ABSOLUTE
    // `.module.css` path (exactly as `zfb-css` emits it / the bundler keys it).
    let mut names: HashMap<String, String> = HashMap::new();
    names.insert("brand".into(), "wsdisc_brand".into());
    let mut class_maps: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    class_maps.insert(module_css, names);
    input.css_module_class_maps = class_maps;

    let out = bundle(input).expect(
        "bundle must succeed: @/* alias must resolve to the shadow CSS-module \
         when imported from inside node_modules/@scope/foo (no #443 regression)",
    );

    let body = fs::read_to_string(&out.bundle_path).expect("read bundle");

    // Half 2: the scoped class string proves the alias resolved to the
    // SHADOW-rewritten `.module.css` JS shim, not the raw original.
    assert!(
        body.contains("wsdisc_brand"),
        "discriminator: the scoped CSS class (wsdisc_brand) must appear — the \
         `@/styles/theme.module.css` alias must resolve to the shadow-rewritten \
         CSS-module shim even though the importer lives inside node_modules. \
         Its absence means the alias resolved outside the shadow (pre-#677 hole B) \
         OR #443 regressed."
    );
    // Half 1: the importer's own body must be present, confirming the
    // node_modules-package importer resolved at all (no #443 regression).
    assert!(
        body.contains("brand-class:"),
        "discriminator: the workspace package's header.tsx body must appear — \
         the node_modules importer must still resolve (no #443 regression)"
    );

    drop(tmp);
}
