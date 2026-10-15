//! Native-resolver package externalization candidate, isolated from production.
use anyhow::Result;
use rolldown::plugin::{
    HookResolveIdArgs, HookResolveIdOutput, HookUsage, Plugin, PluginContext,
    PluginContextResolveOptions,
};
use rolldown_resolver::ResolveError;
use std::{borrow::Cow, path::Path, sync::Arc};

#[derive(Debug)]
pub(super) struct PackageExternals;

fn is_bare(specifier: &str) -> bool {
    // A slash-rooted path is not fully absolute on Windows, but it is never
    // a package request. has_root also handles native drive and UNC roots.
    !Path::new(specifier).has_root()
        && specifier != "."
        && specifier != ".."
        && !specifier.starts_with("./")
        && !specifier.starts_with("../")
}

impl Plugin for PackageExternals {
    fn name(&self) -> Cow<'static, str> {
        "zfb-plugin-probe-package-externals".into()
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId
    }

    async fn resolve_id(
        &self,
        ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> Result<Option<HookResolveIdOutput>> {
        if args.is_entry || !is_bare(args.specifier) {
            return Ok(None);
        }
        // The SAME native resolver, with package-directory lookup disabled in
        // BundlerOptions, still owns tsconfig paths/baseUrl, package imports,
        // extension selection and import-vs-require conditions. Never classify a
        // resolved filename by whether it contains "node_modules": aliases may
        // intentionally target a source file there.
        let resolved = ctx
            .resolve(
                args.specifier,
                args.importer,
                Some(PluginContextResolveOptions {
                    import_kind: args.kind,
                    is_entry: args.is_entry,
                    skip_self: true,
                    custom: Arc::clone(&args.custom),
                }),
            )
            .await?;
        match resolved {
            Ok(resolved) => Ok(Some(HookResolveIdOutput::from_resolved_id(resolved))),
            // Only an ordinary unresolved package request is left to Node. A
            // malformed tsconfig, failed explicit alias or package-import error
            // must not be silently converted into an external module.
            Err(ResolveError::NotFound(missing))
                if missing == args.specifier && !args.specifier.starts_with('#') =>
            {
                Ok(Some(HookResolveIdOutput {
                    id: args.specifier.into(),
                    external: Some(true.into()),
                    ..Default::default()
                }))
            }
            Err(error) => Err(error.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::is_bare;

    #[test]
    fn package_and_mapping_requests_reach_the_native_resolver() {
        for specifier in [
            "pkg",
            "@scope/pkg/sub",
            ".hidden-package",
            "#local",
            "node:path",
        ] {
            assert!(is_bare(specifier), "{specifier}");
        }
    }

    #[test]
    fn relative_and_rooted_files_are_not_package_requests() {
        for specifier in [".", "..", "./pkg", "../pkg", "/tmp/pkg"] {
            assert!(!is_bare(specifier), "{specifier}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn windows_drive_and_unc_roots_are_not_package_requests() {
        for specifier in [r"C:\tmp\pkg", r"\tmp\pkg", r"\\server\share\pkg"] {
            assert!(!is_bare(specifier), "{specifier}");
        }
    }
}
