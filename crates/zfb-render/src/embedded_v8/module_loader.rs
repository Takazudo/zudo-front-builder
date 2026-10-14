//! Deno module-loader adapter over the canonical ZFB source/authorization service.
use deno_core::error::ModuleLoaderError;
use deno_core::{
    ModuleLoadOptions, ModuleLoadReferrer, ModuleLoadResponse, ModuleLoader, ModuleSource,
    ModuleSourceCode, ModuleSpecifier, ModuleType, ResolutionKind,
};
use std::path::PathBuf;
pub use zfb_js_host::modules::{AliasHook, PluginRegistryHooks, VirtualModuleHook};

#[derive(Default)]
pub struct BundleModuleLoader(zfb_js_host::modules::BundleModuleLoader);
impl BundleModuleLoader {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn with_main(self, specifier: &str, source: impl Into<String>) -> Self {
        Self(self.0.with_main(specifier, source))
    }
    pub fn with_plugin_hooks(self, hooks: PluginRegistryHooks) -> Self {
        Self(self.0.with_plugin_hooks(hooks))
    }
    pub fn with_bundle_asset_root(self, root: impl Into<PathBuf>) -> Self {
        Self(self.0.with_bundle_asset_root(root))
    }
    pub fn register_module(&self, specifier: &str, source: impl Into<String>) {
        self.0.register_module(specifier, source)
    }
    #[cfg(test)]
    fn resolve_bundle_wasm_asset(
        &self,
        specifier: &ModuleSpecifier,
    ) -> Result<Option<PathBuf>, ModuleLoaderError> {
        self.0
            .resolve_bundle_wasm_asset(specifier)
            .map_err(ModuleLoaderError::generic)
    }
}
impl ModuleLoader for BundleModuleLoader {
    fn resolve(
        &self,
        specifier: &str,
        referrer: &str,
        _kind: ResolutionKind,
    ) -> Result<ModuleSpecifier, ModuleLoaderError> {
        self.0
            .resolve(specifier, referrer)
            .map_err(ModuleLoaderError::generic)
    }
    fn load(
        &self,
        specifier: &ModuleSpecifier,
        _referrer: Option<&ModuleLoadReferrer>,
        _options: ModuleLoadOptions,
    ) -> ModuleLoadResponse {
        ModuleLoadResponse::Sync(
            self.0
                .load(specifier)
                .map(|module| {
                    ModuleSource::new_with_redirect(
                        ModuleType::JavaScript,
                        ModuleSourceCode::String(module.source.into()),
                        &module.specified_url,
                        &module.found_url,
                        None,
                    )
                })
                .map_err(ModuleLoaderError::generic),
        )
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use tempfile::TempDir;

    use super::BundleModuleLoader;
    use crate::{EmbeddedV8RenderHost, HttpRequestLike, RenderHost};

    #[test]
    fn shared_resolution_retains_the_baseline_url_and_error_contract() {
        use deno_core::ModuleLoader;
        for (specifier, referrer) in [
            ("./child.js", "file:///zfb/main.mjs"),
            ("../child.js", "file:///zfb/nested/main.mjs"),
            ("bare", "file:///zfb/main.mjs"),
            ("bare", ""),
            ("./child.js", "invalid base"),
            ("https://[invalid", "file:///zfb/main.mjs"),
        ] {
            let actual = BundleModuleLoader::new().resolve(
                specifier,
                referrer,
                deno_core::ResolutionKind::Import,
            );
            let expected = deno_core::resolve_import(specifier, referrer);
            match (actual, expected) {
                (Ok(actual), Ok(expected)) => assert_eq!(actual, expected),
                (Err(actual), Err(expected)) => assert_eq!(actual.to_string(), format!("embedded V8 host: failed to resolve `{specifier}` from `{referrer}`: {expected}")),
                other => panic!("resolution behavior differs: {other:?}"),
            }
        }
    }

    // A valid, empty Wasm module: magic, version, and no sections.
    const EMPTY_WASM_MODULE: &[u8] = b"\0asm\x01\0\0\0";

    fn write_wasm_asset(dir: &TempDir, name: &str) -> std::path::PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, EMPTY_WASM_MODULE).expect("write Wasm fixture");
        path
    }

    fn bundle_importing_wasm(specifier: &str) -> String {
        format!(
            r#"
            import wasm from "{specifier}";
            export default {{
              fetch() {{
                return new Response(
                  wasm instanceof WebAssembly.Module ? "module" : "not-module",
                  {{ status: 200 }},
                );
              }},
            }};
            "#
        )
    }

    fn host_for_asset_root(asset_root: &Path) -> EmbeddedV8RenderHost {
        let loader = BundleModuleLoader::new().with_bundle_asset_root(asset_root);
        EmbeddedV8RenderHost::with_loader(loader).expect("host boot")
    }

    async fn load_failure(asset_root: &Path, source: &str) -> String {
        let mut host = host_for_asset_root(asset_root);
        host.execute_module("bundle.mjs", source)
            .await
            .expect_err("untrusted Wasm import must fail")
            .to_string()
    }

    #[tokio::test]
    async fn bundle_wasm_import_exports_a_webassembly_module() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_wasm_asset(&dir, "tiny.wasm");
        let mut host = host_for_asset_root(dir.path());

        host.execute_module("bundle.mjs", &bundle_importing_wasm("./tiny.wasm"))
            .await
            .expect("bundle should load its copied Wasm asset");
        let response = host
            .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
            .await
            .expect("dispatch");

        assert_eq!(response.status, 200);
        assert_eq!(response.body_utf8(), Some("module"));
    }

    #[tokio::test]
    async fn bundle_wasm_import_percent_decodes_asset_filename() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_wasm_asset(&dir, "tiny module.wasm");
        let mut host = host_for_asset_root(dir.path());

        host.execute_module("bundle.mjs", &bundle_importing_wasm("./tiny%20module.wasm"))
            .await
            .expect("URL-escaped asset filename should load");
        let response = host
            .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
            .await
            .expect("dispatch");

        assert_eq!(response.body_utf8(), Some("module"));
    }

    #[tokio::test]
    async fn in_memory_wasm_module_takes_precedence_over_bundle_assets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let loader = BundleModuleLoader::new().with_bundle_asset_root(dir.path());
        loader.register_module("file:///zfb/in-memory.wasm", r#"export default "memory";"#);
        let mut host = EmbeddedV8RenderHost::with_loader(loader).expect("host boot");
        let bundle = r#"
            import wasm from "./in-memory.wasm";
            export default {
              fetch() {
                return new Response(wasm, { status: 200 });
              },
            };
            "#;

        host.execute_module("bundle.mjs", bundle)
            .await
            .expect("in-memory Wasm module should load before a disk lookup");
        let response = host
            .dispatch_fetch(HttpRequestLike::get("http://zfb.local/"))
            .await
            .expect("dispatch");

        assert_eq!(response.body_utf8(), Some("memory"));
    }

    #[tokio::test]
    async fn bundle_wasm_import_rejects_encoded_traversal_and_absolute_file_urls() {
        let fixture = tempfile::tempdir().expect("tempdir");
        let asset_root = fixture.path().join("assets");
        std::fs::create_dir(&asset_root).expect("asset root");
        let outside = write_wasm_asset(&fixture, "outside.wasm");

        let traversal_error =
            load_failure(&asset_root, &bundle_importing_wasm("./..%2Foutside.wasm")).await;
        assert!(
            traversal_error.contains("escapes the bundle asset root"),
            "percent-decoded traversal must hit the containment guard: {traversal_error}"
        );

        let absolute_specifier = deno_core::ModuleSpecifier::from_file_path(&outside)
            .expect("absolute file URL")
            .to_string();
        let absolute_error =
            load_failure(&asset_root, &bundle_importing_wasm(&absolute_specifier)).await;
        assert!(
            absolute_error.contains("no in-memory source"),
            "absolute file URL must retain the no-arbitrary-file-read failure: {absolute_error}"
        );

        let hosted_file_url =
            deno_core::ModuleSpecifier::parse("file://asset-host/zfb/outside.wasm")
                .expect("hosted file URL");
        let loader = BundleModuleLoader::new().with_bundle_asset_root(&asset_root);
        assert!(
            loader
                .resolve_bundle_wasm_asset(&hosted_file_url)
                .expect("hosted file URL is not malformed")
                .is_none(),
            "only the exact synthetic file:///zfb/ namespace may load copied Wasm assets"
        );
    }

    #[tokio::test]
    async fn bundle_wasm_import_missing_asset_names_expected_disk_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let expected = dir.path().join("missing.wasm");

        let error = load_failure(dir.path(), &bundle_importing_wasm("./missing.wasm")).await;
        assert!(
            error.contains(&expected.display().to_string()),
            "missing asset error must name the expected disk path `{}`: {error}",
            expected.display()
        );
    }
}
