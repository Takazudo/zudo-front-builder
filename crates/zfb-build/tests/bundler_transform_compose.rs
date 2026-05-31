//! Sub-issue #677 — in-shadow transforms compose with tsconfig paths.
//!
//! In **branch 4** of `run_esbuild` (`node_modules_dir = Some` +
//! `node_modules_preserve_symlinks = false` + non-empty `tsconfig_paths`)
//! esbuild runs WITHOUT `--preserve-symlinks` (deliberate, to keep the
//! #443/#450 workspace path-alias resolution working). In that config the
//! in-shadow source transforms — `import.meta.glob` expansion and the
//! `.module.css` → JS rewrite — used to be invisible to esbuild because:
//!
//!  - **Relative-reach (hole A):** a symlinked source importer is
//!    canonicalised back to the real tree, so its relative `./registry` /
//!    `./x.module.css` resolves to the untransformed original. Fixed by
//!    `copy_mode` (Part 1) — source files are materialised as real copies
//!    so the importer stays physically inside the shadow.
//!  - **Alias-reach (hole B):** `read_tsconfig_paths` absolutises alias
//!    targets to the REAL root, so `@lib/foo` resolved entirely outside the
//!    shadow. Fixed by the shadow-first dual-target rebase (Part 2).
//!
//! These tests reproduce the real zzmod config (project node_modules +
//! non-empty `tsconfig_paths`) and assert the transform is now visible both
//! via relative imports and via `@alias` imports.

use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use zfb_build::{bundle, BundleMode, BundlerInput};
use zfb_render::adapters::Framework;
use zfb_test_utils::locate_esbuild;

/// Absolutise each `compilerOptions.paths` target against the project root
/// (preserving a trailing `/*`) — the exact shape `read_tsconfig_paths`
/// produces and the bundler's `tsconfig_paths` field expects.
fn tsconfig_paths_absolute(
    project_root: &Path,
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

/// A real (non-symlinked) `node_modules` directory so the build hits
/// **branch 4** — `node_modules_dir = Some` is the toggle that turns OFF
/// `--preserve-symlinks` when `tsconfig_paths` is non-empty. An empty
/// directory is enough: this fixture's imports are all relative / aliased
/// to in-project source, with framework packages marked `external`.
fn make_real_node_modules(root: &Path) -> PathBuf {
    let nm = root.join("node_modules");
    fs::create_dir_all(&nm).unwrap();
    nm
}

fn branch4_input(
    root: &Path,
    esbuild: &Path,
    tsconfig_paths: BTreeMap<String, Vec<String>>,
    class_maps: HashMap<PathBuf, HashMap<String, String>>,
) -> BundlerInput {
    main_fields: Vec::new(),
    let mut input = BundlerInput::for_project(
        root.to_path_buf(),
        Framework::Preact,
        BundleMode::Production,
        root.join("dist"),
        None,
    );
    input.external = vec![
        "preact".into(),
        "preact-render-to-string".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild.to_path_buf());
    // The two conditions that, with non-empty tsconfig_paths, select branch 4
    // (no --preserve-symlinks → copy_mode on).
    input.node_modules_dir = Some(make_real_node_modules(root));
    input.node_modules_preserve_symlinks = false;
    input.tsconfig_paths = tsconfig_paths;
    input.css_module_class_maps = class_maps;
    input
}

/// Relative-reach + glob: a page imports a barrel via `./` that uses
/// `import.meta.glob`. With copy_mode the importer + barrel are real copies
/// in the shadow, so esbuild reads the Rust-expanded barrel.
#[test]
fn branch4_relative_glob_transform_is_visible() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_transform_compose] no esbuild binary; set ZFB_ESBUILD_BIN. Skipping.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for d in ["pages", "components", "layouts", "content"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    fs::write(
        root.join("components/_gallery.tsx"),
        "const stories = import.meta.glob('./*.stories.tsx', { eager: true });\n\
         export const galleryKeys = Object.keys(stories).sort().join(\",\");\n",
    )
    .unwrap();
    fs::write(
        root.join("components/good.stories.tsx"),
        "export const GoodStory = () => \"ok\";\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "import { galleryKeys } from \"../components/_gallery\";\n\
         export default function Home() { return galleryKeys; }\n",
    )
    .unwrap();

    // Non-empty tsconfig_paths (an unrelated alias) is enough to put us in
    // branch 4; the page reaches the transform purely via the relative import.
    let paths = tsconfig_paths_absolute(root, &[("@unused/*", "src/*")]);
    let out = bundle(branch4_input(root, &esbuild, paths, HashMap::new()))
        .expect("branch4 relative-glob build must succeed");
    let body = fs::read_to_string(&out.bundle_path).unwrap();

    assert!(
        !body.contains("import.meta.glob("),
        "relative-reach: import.meta.glob( must be expanded Rust-side and absent.\n{body}"
    );
    assert!(
        body.contains("good.stories.tsx"),
        "relative-reach: expanded glob key (good.stories.tsx) must appear.\n{body}"
    );
}

/// Relative-reach + CSS-module: a page imports `./x.module.css` and reads a
/// class. With copy_mode the page is a real copy so esbuild resolves the
/// relative `.module.css` to the rewritten JS shim in the shadow.
#[test]
fn branch4_relative_css_module_transform_is_visible() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_transform_compose] no esbuild binary; set ZFB_ESBUILD_BIN. Skipping.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for d in ["pages", "components", "layouts", "content"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    let module_css = root.join("pages/hero.module.css");
    fs::write(&module_css, ".foo { color: red; }\n").unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "import styles from \"./hero.module.css\";\n\
         export default function Page() { return styles.foo; }\n",
    )
    .unwrap();

    let mut names: HashMap<String, String> = HashMap::new();
    names.insert("foo".into(), "rel123_foo".into());
    let mut class_maps: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    class_maps.insert(module_css, names);

    let paths = tsconfig_paths_absolute(root, &[("@unused/*", "src/*")]);
    let out = bundle(branch4_input(root, &esbuild, paths, class_maps))
        .expect("branch4 relative-css build must succeed");
    let body = fs::read_to_string(&out.bundle_path).unwrap();

    assert!(
        body.contains("rel123_foo"),
        "relative-reach: scoped CSS class (rel123_foo) must appear in the bundle.\n{body}"
    );
}

/// Alias-reach + glob: a page imports a glob barrel that lives in an aliased
/// dir (`@lib/* → src/lib/*`) via `@lib/...`. The shadow-first dual-target
/// rebase + copy_mode make the Rust-expanded barrel visible.
#[test]
fn branch4_alias_glob_transform_is_visible() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_transform_compose] no esbuild binary; set ZFB_ESBUILD_BIN. Skipping.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for d in ["pages", "components", "layouts", "content", "src/lib"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    fs::write(
        root.join("src/lib/gallery.tsx"),
        "const stories = import.meta.glob('./*.story.tsx', { eager: true });\n\
         export const aliasGalleryKeys = Object.keys(stories).sort().join(\",\");\n",
    )
    .unwrap();
    fs::write(
        root.join("src/lib/alpha.story.tsx"),
        "export const Alpha = () => \"alpha\";\n",
    )
    .unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "import { aliasGalleryKeys } from \"@lib/gallery\";\n\
         export default function Home() { return aliasGalleryKeys; }\n",
    )
    .unwrap();

    let paths = tsconfig_paths_absolute(root, &[("@lib/*", "src/lib/*")]);
    let out = bundle(branch4_input(root, &esbuild, paths, HashMap::new()))
        .expect("branch4 alias-glob build must succeed");
    let body = fs::read_to_string(&out.bundle_path).unwrap();

    assert!(
        !body.contains("import.meta.glob("),
        "alias-reach: import.meta.glob( must be expanded and absent.\n{body}"
    );
    assert!(
        body.contains("alpha.story.tsx"),
        "alias-reach: expanded glob key (alpha.story.tsx) must appear via @lib/* alias.\n{body}"
    );
}

/// Alias-reach + CSS-module: a page imports a `.module.css` from an aliased
/// dir (`@lib/* → src/lib/*`) via `@lib/...`. The dual-target rebase must
/// route the aliased import to the shadow-rewritten JS shim.
#[test]
fn branch4_alias_css_module_transform_is_visible() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_transform_compose] no esbuild binary; set ZFB_ESBUILD_BIN. Skipping.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for d in ["pages", "components", "layouts", "content", "src/lib"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    let module_css = root.join("src/lib/card.module.css");
    fs::write(&module_css, ".title { font-weight: bold; }\n").unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "import styles from \"@lib/card.module.css\";\n\
         export default function Page() { return styles.title; }\n",
    )
    .unwrap();

    let mut names: HashMap<String, String> = HashMap::new();
    names.insert("title".into(), "alias987_title".into());
    let mut class_maps: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    class_maps.insert(module_css, names);

    let paths = tsconfig_paths_absolute(root, &[("@lib/*", "src/lib/*")]);
    let out = bundle(branch4_input(root, &esbuild, paths, class_maps))
        .expect("branch4 alias-css build must succeed");
    let body = fs::read_to_string(&out.bundle_path).unwrap();

    assert!(
        body.contains("alias987_title"),
        "alias-reach: scoped CSS class (alias987_title) must appear via @lib/* alias.\n{body}"
    );
}

/// Whole-root `@/* → src/*` variant: the alias maps to the project root's
/// `src/` dir (prefix == `<root>/src`). `rebase` must emit the shadow-first
/// dual-target for it just like a nested dir, so a transform reached via
/// `@/...` is visible.
#[test]
fn branch4_whole_root_alias_css_module_transform_is_visible() {
    let Some(esbuild) = locate_esbuild() else {
        eprintln!("[bundler_transform_compose] no esbuild binary; set ZFB_ESBUILD_BIN. Skipping.");
        return;
    };
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    for d in ["pages", "components", "layouts", "content", "src/ui"] {
        fs::create_dir_all(root.join(d)).unwrap();
    }

    let module_css = root.join("src/ui/badge.module.css");
    fs::write(&module_css, ".pill { border-radius: 9999px; }\n").unwrap();
    fs::write(
        root.join("pages/index.tsx"),
        "import styles from \"@/ui/badge.module.css\";\n\
         export default function Page() { return styles.pill; }\n",
    )
    .unwrap();

    let mut names: HashMap<String, String> = HashMap::new();
    names.insert("pill".into(), "root456_pill".into());
    let mut class_maps: HashMap<PathBuf, HashMap<String, String>> = HashMap::new();
    class_maps.insert(module_css, names);

    let paths = tsconfig_paths_absolute(root, &[("@/*", "src/*")]);
    let out = bundle(branch4_input(root, &esbuild, paths, class_maps))
        .expect("branch4 whole-root alias-css build must succeed");
    let body = fs::read_to_string(&out.bundle_path).unwrap();

    assert!(
        body.contains("root456_pill"),
        "whole-root @/*: scoped CSS class (root456_pill) must appear via @/ alias.\n{body}"
    );
}
