//! The prepared `--tsconfig` JSX options govern dependency TSX too, as under
//! esbuild's override (#4407).
use super::run_prepared;
use serde_json::Value;
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

const OWNED_SOURCE: &str = "@takazudo/zfb/zudo-react";

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "zfb-native-jsx-tsconfig-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        let root = fs::canonicalize(path).unwrap();
        // The shape `write_synthetic_tsconfig` emits for the SSR pass.
        fs::write(
            root.join("tsconfig.json"),
            format!(
                r#"{{"compilerOptions":{{"baseUrl":".","paths":{{}},"jsx":"react-jsx","jsxImportSource":"{OWNED_SOURCE}"}}}}"#
            ),
        )
        .unwrap();
        Self(root)
    }
    fn write(&self, rel: &str, body: &str) {
        let path = self.0.join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, body).unwrap();
    }
    /// The argv `zfb_build::bundler::run_esbuild` prepares: `--tsconfig` only,
    /// no `--jsx` / `--jsx-import-source`.
    fn ssr_command(&self) -> Command {
        let mut cmd = Command::new("unused-prepared-esbuild");
        cmd.current_dir(&self.0).args([
            "entry.mjs",
            "--bundle",
            "--format=esm",
            "--platform=neutral",
            "--external:@takazudo/zfb/*",
            "--external:preact/*",
            "--tsconfig=tsconfig.json",
            "--outfile=out/bundle.mjs",
            "--metafile=out/meta.json",
        ]);
        cmd
    }
    fn bundle(&self) -> String {
        fs::read_to_string(self.0.join("out/bundle.mjs")).unwrap()
    }
    fn warnings(&self) -> Vec<String> {
        let meta: Value =
            serde_json::from_str(&fs::read_to_string(self.0.join("out/meta.json")).unwrap())
                .unwrap();
        meta["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|w| w.as_str().unwrap().to_string())
            .collect()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn dependency_tsx_uses_the_prepared_tsconfig_jsx_import_source() {
    let fixture = Fixture::new();
    fixture.write(
        "entry.mjs",
        "export { default as Page } from 'pkg/routes-src/page.tsx';\n\
         export { default as List } from 'pkg/routes-src/list.tsx';\n\
         export { default as Local } from './local.tsx';\n",
    );
    fixture.write("node_modules/pkg/package.json", r#"{"name":"pkg"}"#);
    fixture.write(
        "node_modules/pkg/routes-src/page.tsx",
        "/** @jsxRuntime automatic */\nexport default function Page() { return <main>hi</main>; }\n",
    );
    fixture.write(
        "node_modules/pkg/routes-src/list.tsx",
        "export default function List() { return <><li>a</li><li>b</li></>; }\n",
    );
    fixture.write(
        "local.tsx",
        "export default function Local() { return <p>local</p>; }\n",
    );
    run_prepared(&fixture.ssr_command()).unwrap();
    let bundle = fixture.bundle();
    assert!(
        bundle.contains(&format!("\"{OWNED_SOURCE}/jsx-runtime\"")),
        "{bundle}"
    );
    assert!(!bundle.contains("\"react/jsx-runtime\""), "{bundle}");
    assert!(bundle.contains("Fragment"), "{bundle}");
    let warnings = fixture.warnings();
    assert!(warnings.is_empty(), "{warnings:#?}");
}

#[test]
fn a_file_pragma_still_overrides_the_prepared_import_source() {
    let fixture = Fixture::new();
    fixture.write("entry.mjs", "export { default } from 'pkg/view.tsx';\n");
    fixture.write("node_modules/pkg/package.json", r#"{"name":"pkg"}"#);
    fixture.write(
        "node_modules/pkg/view.tsx",
        "/** @jsxImportSource preact */\nexport default function View() { return <p>v</p>; }\n",
    );
    run_prepared(&fixture.ssr_command()).unwrap();
    let bundle = fixture.bundle();
    assert!(bundle.contains("preact/jsx-runtime"), "{bundle}");
    assert!(!bundle.contains(OWNED_SOURCE), "{bundle}");
}
