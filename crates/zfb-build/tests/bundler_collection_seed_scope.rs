//! Regression for #3142 (epic #3135, source #3133): a content collection seeds
//! the `node_modules` staging closure only with the files its include/exclude
//! filter keeps — the same predicate `materialise_collection` applies when it
//! copies the collection into the shadow.
//!
//! A `.tsx` the `include` glob drops is never copied into the shadow, so
//! esbuild can never see it; before #3142 it still seeded the closure, reached
//! a workspace package, and flipped the whole build into workspace staging
//! (plus the post-flip drain of every deferred live dependency). MDX-level
//! imports are dropped by zfb's MDX compiler (components arrive through the
//! `components` prop), so an `.mdx` never seeds anything either.
//!
//! The fixture is the #3133 shape at unit scale: a pnpm workspace whose site
//! (`apps/site`) declares an out-of-root collection at
//! `../../packages/ui/src/components`, a real site `node_modules`, a non-empty
//! tsconfig `paths` map (the gate that makes the closure resolve through
//! canonical package dirs), and one physical pnpm-private `leftpad-priv`
//! reached along three logical paths.
//!
//! Uses `mock_subprocess_output`: staging runs before esbuild, so no esbuild
//! binary is needed.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use zfb_build::{bundle, BundleMode, BundlerInput, ContentCollectionSpec, NodeModulesStagingStats};
use zfb_render::adapters::Framework;

const COLLECTION_ROOT: &str = "../../packages/ui/src/components";

fn write(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
}

fn write_package(dir: &Path, name: &str, deps: &[&str], index_body: &str) {
    let deps = deps
        .iter()
        .map(|dep| format!(r#""{dep}":"workspace:*""#))
        .collect::<Vec<_>>()
        .join(",");
    write(
        &dir.join("package.json"),
        &format!(
            r#"{{"name":"{name}","version":"1.0.0","type":"module","main":"./index.js","dependencies":{{{deps}}}}}"#
        ),
    );
    write(&dir.join("index.js"), index_body);
}

/// Lay down the workspace and return `(workspace_root, site_root)`.
///
/// - `packages/ui/src/components/button/{button.mdx,button.tsx}` and
///   `orphan/orphan.tsx`: both `.tsx` files import the workspace package
///   `shared-utils` (and `button.tsx` also a ui-only and a site-installed
///   dependency).
/// - `shared-utils` → `shared-icons` (workspace links) → the one physical
///   `leftpad-priv` in `<ws>/node_modules/.pnpm`.
/// - `apps/site/node_modules/{shared-utils,site-dep}`: what `pnpm install`
///   gives the site; `site-dep` is an ordinary (non-workspace) package.
fn write_fixture(ws: &Path) -> PathBuf {
    write(
        &ws.join("pnpm-workspace.yaml"),
        "packages:\n  - \"apps/*\"\n  - \"packages/*\"\n",
    );

    let store = ws.join("node_modules/.pnpm");
    let leftpad = store.join("leftpad-priv@1.0.0/node_modules/leftpad-priv");
    write_package(&leftpad, "leftpad-priv", &[], "export default 'LEFTPAD';\n");
    let site_dep = store.join("site-dep@1.0.0/node_modules/site-dep");
    write_package(&site_dep, "site-dep", &[], "export default 'SITE_DEP';\n");
    let ui_only = store.join("ui-only-dep@1.0.0/node_modules/ui-only-dep");
    write_package(&ui_only, "ui-only-dep", &[], "export default 'UI_ONLY';\n");

    let packages = ws.join("packages");
    write_package(
        &packages.join("shared-icons"),
        "shared-icons",
        &[],
        "import 'leftpad-priv';\nexport function icon() { return 'icon'; }\n",
    );
    write_package(
        &packages.join("shared-utils"),
        "shared-utils",
        &["shared-icons"],
        "import 'leftpad-priv';\nimport { icon } from 'shared-icons';\n\
         export function helper() { return icon(); }\n",
    );
    write(
        &packages.join("ui/package.json"),
        r#"{"name":"ui","version":"1.0.0","dependencies":{"shared-utils":"workspace:*"}}"#,
    );
    let components = packages.join("ui/src/components");
    write(
        &components.join("button/button.mdx"),
        "---\ntitle: Button\n---\n\nimport Button from \"./button.tsx\";\n\n<Button />\n",
    );
    write(
        &components.join("button/button.tsx"),
        "import 'leftpad-priv';\nimport 'ui-only-dep';\nimport 'site-dep';\n\
         import { helper } from 'shared-utils';\n\
         export default function Button() { return <button>{helper()}</button>; }\n",
    );
    write(
        &components.join("orphan/orphan.tsx"),
        "import { helper } from 'shared-utils';\n\
         export default function Orphan() { return <div>{helper()}</div>; }\n",
    );

    for (owner, dep, target) in [
        ("packages/ui", "leftpad-priv", leftpad.clone()),
        ("packages/ui", "ui-only-dep", ui_only),
        ("packages/ui", "shared-utils", packages.join("shared-utils")),
        ("packages/shared-utils", "leftpad-priv", leftpad.clone()),
        (
            "packages/shared-utils",
            "shared-icons",
            packages.join("shared-icons"),
        ),
        ("packages/shared-icons", "leftpad-priv", leftpad),
        ("apps/site", "shared-utils", packages.join("shared-utils")),
        ("apps/site", "site-dep", site_dep),
    ] {
        let node_modules = ws.join(owner).join("node_modules");
        fs::create_dir_all(&node_modules).unwrap();
        symlink(&target, node_modules.join(dep)).unwrap();
    }

    let site = ws.join("apps/site");
    write(
        &site.join("package.json"),
        r#"{"name":"site","private":true,"dependencies":{"shared-utils":"workspace:*","site-dep":"1.0.0"}}"#,
    );
    write(
        &site.join("pages/index.tsx"),
        "export default function Index() { return <div>home</div>; }\n",
    );
    write(
        &site.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    );
    write(
        &site.join("components/unused.tsx"),
        "export default function Unused() { return null; }\n",
    );
    fs::create_dir_all(site.join("content")).unwrap();
    site
}

fn collection(include: &[&str]) -> ContentCollectionSpec {
    ContentCollectionSpec {
        include: Some(include.iter().map(|glob| glob.to_string()).collect()),
        ..ContentCollectionSpec::new("componentDocs", COLLECTION_ROOT)
    }
}

fn staging_stats(
    site: &Path,
    collections: Vec<ContentCollectionSpec>,
    bundle_exclude: Vec<String>,
) -> NodeModulesStagingStats {
    let input = BundlerInput {
        external: vec!["preact".into(), "@takazudo/zfb-runtime".into()],
        mock_subprocess_output: Some("export default {};\n".to_string()),
        node_modules_dir: Some(site.join("node_modules")),
        // Any non-empty `paths` map makes the closure resolve through
        // canonical package dirs — the #3133 fixture's load-bearing gate.
        tsconfig_paths: BTreeMap::from([(
            "@/unused".to_string(),
            vec![site
                .join("components/unused.tsx")
                .to_string_lossy()
                .into_owned()],
        )]),
        content_collections: collections,
        bundle_exclude,
        ..BundlerInput::for_project(
            site.to_path_buf(),
            Framework::Preact,
            BundleMode::Production,
            site.join("dist"),
            None,
        )
    };
    bundle(input)
        .expect("mock bundle must succeed")
        .node_modules_staging_stats
}

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let tmp = tempfile::tempdir().expect("tempdir");
    let ws = tmp.path().canonicalize().unwrap();
    let site = write_fixture(&ws);
    (tmp, site)
}

const NOTHING_STAGED: NodeModulesStagingStats = NodeModulesStagingStats {
    physical_scans: 0,
    logical_visits: 0,
    workspace_staging_activated: false,
    cache_hits: 0,
    parsed_files: 0,
};

/// (a) The #3133 shape: `include: ["**/*.mdx"]` drops both `.tsx` files from
/// the shadow, so neither may seed — workspace staging never flips.
#[test]
fn non_included_tsx_siblings_never_seed_or_flip_workspace_staging() {
    let (_tmp, site) = fixture();
    let control = staging_stats(&site, Vec::new(), Vec::new());
    assert_eq!(control, NOTHING_STAGED, "control: the site imports nothing");

    let stats = staging_stats(&site, vec![collection(&["**/*.mdx"])], Vec::new());
    assert_eq!(
        stats, NOTHING_STAGED,
        "a collection whose include glob keeps only `.mdx` contributes no seeds: the dropped \
         `button.tsx` / `orphan.tsx` are never copied into the shadow, so their workspace imports \
         must not flip staging",
    );
}

/// (b) A `.tsx` the include glob DOES match is materialised and keeps seeding,
/// resolved from its logical shadow location `<site>/content/<name>/…` — so a
/// site-installed dependency is staged and a dependency installed only beside
/// the physical `packages/ui` source (which esbuild can never reach from the
/// shadow copy) is not.
#[test]
fn included_tsx_seeds_from_its_logical_site_location() {
    let (_tmp, site) = fixture();
    let include = ["**/*.{mdx,tsx}"];

    let stats = staging_stats(
        &site,
        vec![collection(&include)],
        vec!["does-not-exist/**".to_string()],
    );
    // Visits, every one under `<site>/node_modules`: `site-dep`,
    // `shared-utils`, and `shared-utils`' own `shared-icons` / `leftpad-priv`
    // aliases. `ui-only-dep` and `packages/ui`'s own `leftpad-priv` link are
    // NOT reachable from `<site>/content/…`, so a fifth visit (or a sixth)
    // means the seed resolved from its physical `packages/ui` location.
    assert_eq!(
        stats,
        NodeModulesStagingStats {
            physical_scans: 4,
            logical_visits: 4,
            workspace_staging_activated: true,
            cache_hits: 0,
            parsed_files: 4,
        },
        "an included `.tsx` must seed from `<site>/content/componentDocs/…`, staging the site's \
         `site-dep` / `shared-utils` and nothing from `packages/ui/node_modules`",
    );

    let stats = staging_stats(&site, vec![collection(&include)], Vec::new());
    assert!(
        stats.workspace_staging_activated,
        "under an empty bundle.exclude, an included `.tsx` importing a workspace package must \
         still flip workspace staging: {stats:?}",
    );
}

/// (b, logical root) The matched file's importer is its SHADOW location
/// `<site>/content/<name>/<rel>`, not a synthetic project entry: a relative
/// `../../../node_modules/site-dep` import from `button/deep.tsx` climbs to
/// `<shadow>/node_modules/site-dep` in the shadow, so that exact site-level
/// package must be staged. From the physical `packages/ui/src/components/…`
/// location (or from `<site>/entry`) the same specifier names no package.
#[test]
fn included_tsx_relative_node_modules_import_resolves_from_the_logical_collection_root() {
    let (_tmp, site) = fixture();
    write(
        &site.join(COLLECTION_ROOT).join("button/deep.tsx"),
        "import '../../../node_modules/site-dep/index.js';
         export default function Deep() { return null; }
",
    );
    let stats = staging_stats(
        &site,
        vec![collection(&["**/*.mdx", "button/deep.tsx"])],
        vec!["does-not-exist/**".to_string()],
    );
    assert_eq!(
        stats,
        NodeModulesStagingStats {
            physical_scans: 1,
            logical_visits: 1,
            workspace_staging_activated: false,
            cache_hits: 0,
            parsed_files: 1,
        },
        "`deep.tsx`'s relative node_modules import must resolve from \
         `<site>/content/componentDocs/button/`, staging `<site>/node_modules/site-dep`",
    );
}

/// (c) A non-content sibling (`.ts`) is materialised whatever the include
/// glob says, so it can be an esbuild input and must keep seeding — the seed
/// walk and `materialise_collection` share one predicate.
#[test]
fn materialised_non_content_sibling_still_seeds() {
    let (_tmp, site) = fixture();
    write(
        &site.join(COLLECTION_ROOT).join("button/helper.ts"),
        "import siteDep from 'site-dep';\nexport const helper = siteDep;\n",
    );
    let stats = staging_stats(
        &site,
        vec![collection(&["**/*.mdx"])],
        vec!["does-not-exist/**".to_string()],
    );
    assert_eq!(
        stats,
        NodeModulesStagingStats {
            physical_scans: 1,
            logical_visits: 1,
            workspace_staging_activated: false,
            cache_hits: 0,
            parsed_files: 1,
        },
        "`helper.ts` beside an included `.mdx` is copied into the shadow, so its bare `site-dep` \
         import must be staged",
    );
}

/// (d) MDX-level ESM is dropped by zfb's MDX compiler, so an `.mdx` import
/// stages nothing. A future change that preserves MDX imports must surface
/// here and seed them deliberately.
#[test]
fn mdx_imports_stage_nothing() {
    let (_tmp, site) = fixture();
    write(
        &site.join(COLLECTION_ROOT).join("button/usage.mdx"),
        "import siteDep from \"site-dep\";\nimport { helper } from \"shared-utils\";\n\n# Usage\n",
    );
    let stats = staging_stats(
        &site,
        vec![collection(&["**/*.mdx"])],
        vec!["does-not-exist/**".to_string()],
    );
    assert_eq!(
        stats, NOTHING_STAGED,
        "an `.mdx` file's own imports must not seed the staging closure",
    );
}
