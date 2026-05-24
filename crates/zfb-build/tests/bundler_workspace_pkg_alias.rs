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

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;

fn locate_esbuild() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Some(p);
        }
    }
    let here = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(workspace) = here.parent().and_then(|p| p.parent()) {
        let slot = workspace.join("crates/zfb/binaries/esbuild/esbuild");
        if slot.exists() {
            return Some(slot);
        }
    }
    if let Ok(out) = Command::new("which").arg("esbuild").output() {
        if out.status.success() {
            let p = PathBuf::from(String::from_utf8_lossy(&out.stdout).trim());
            if p.exists() {
                return Some(p);
            }
        }
    }
    None
}

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
