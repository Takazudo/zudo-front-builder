//! Regression for #3139 (epic #3135, source #3133): the `node_modules`
//! dependency-staging closure scans each canonical package directory once.
//!
//! A pnpm-private package that three importers each depend on is reached along
//! three distinct logical paths (`node_modules/<importer>/node_modules/shared`),
//! and all three resolve to one directory in the pnpm store. The closure keeps
//! one logical visit per path — so each alias stays nested under its importer
//! (#1646) — but walks and import-parses the physical directory only once.
//!
//! Uses `mock_subprocess_output`: staging runs before esbuild, so no esbuild
//! binary is needed.

#![cfg(unix)]

use std::collections::BTreeMap;
use std::fs;
use std::os::unix::fs::symlink;
use std::path::Path;

use zfb_build::{bundle, BundleMode, BundlerInput, NodeModulesStagingStats};
use zfb_render::adapters::Framework;

const IMPORTERS: [&str; 3] = ["dep-a", "dep-b", "dep-c"];

fn write_package(dir: &Path, name: &str, index_body: &str) {
    fs::create_dir_all(dir).unwrap();
    fs::write(
        dir.join("package.json"),
        format!(r#"{{"name":"{name}","type":"module","exports":"./index.js"}}"#),
    )
    .unwrap();
    fs::write(dir.join("index.js"), index_body).unwrap();
}

fn write_fixture(root: &Path) {
    for dir in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(root.join(dir)).unwrap();
    }
    fs::write(
        root.join("layouts/default.tsx"),
        "export default function L({ children }) { return children; }\n",
    )
    .unwrap();
    fs::write(
        root.join("components/unused.tsx"),
        "export default function Unused() { return null; }\n",
    )
    .unwrap();

    let store = root.join("node_modules/.pnpm");
    let shared = store.join("shared@1.0.0/node_modules/shared");
    write_package(
        &shared,
        "shared",
        "import './extra.js'; export const marker = 'SHARED';\n",
    );
    fs::write(shared.join("extra.js"), "export const extra = 1;\n").unwrap();

    let mut page_imports = String::new();
    for importer in IMPORTERS {
        let physical = store.join(format!("{importer}@1.0.0/node_modules/{importer}"));
        write_package(
            &physical,
            importer,
            "import { marker } from 'shared'; export default marker;\n",
        );
        // pnpm links each importer's private dependency beside its store dir.
        symlink(
            &shared,
            store.join(format!("{importer}@1.0.0/node_modules/shared")),
        )
        .unwrap();
        symlink(&physical, root.join("node_modules").join(importer)).unwrap();
        let ident = importer.replace('-', "_");
        page_imports.push_str(&format!("import {ident} from '{importer}';\n"));
    }
    fs::write(
        root.join("pages/index.tsx"),
        format!(
            "{page_imports}export default function Page() {{ return <div>{{dep_a}}{{dep_b}}{{dep_c}}</div>; }}\n"
        ),
    )
    .unwrap();
}

fn staging_stats(root: &Path, bundle_exclude: Vec<String>) -> NodeModulesStagingStats {
    let input = BundlerInput {
        external: vec!["preact".into(), "@takazudo/zfb-runtime".into()],
        mock_subprocess_output: Some("export default {};\n".to_string()),
        node_modules_dir: Some(root.join("node_modules")),
        // Any non-empty `paths` map makes esbuild resolve through canonical
        // package dirs, which is what follows a store dir's private siblings.
        tsconfig_paths: BTreeMap::from([(
            "@/unused".to_string(),
            vec![root
                .join("components/unused.tsx")
                .to_string_lossy()
                .into_owned()],
        )]),
        bundle_exclude,
        ..BundlerInput::for_project(
            root.to_path_buf(),
            Framework::Preact,
            BundleMode::Production,
            root.join("dist"),
            None,
        )
    };
    bundle(input)
        .expect("mock bundle must succeed")
        .node_modules_staging_stats
}

#[test]
fn pnpm_private_dependency_reached_along_three_logical_paths_is_scanned_once() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let root = tmp.path().canonicalize().unwrap();
    write_fixture(&root);

    // A non-empty exclude stages bare dependencies immediately, so the closure
    // follows each importer into its pnpm-private `shared` link.
    let stats = staging_stats(&root, vec!["does-not-exist/**".to_string()]);

    // Logical visits: the three importers plus one `shared` alias under each.
    // Physical scans: the three importers plus ONE scan of the shared store dir.
    assert_eq!(
        stats,
        NodeModulesStagingStats {
            physical_scans: IMPORTERS.len() + 1,
            logical_visits: IMPORTERS.len() * 2,
            workspace_staging_activated: false,
        },
        "the shared pnpm store dir must be scanned exactly once across its three logical aliases",
    );
}
