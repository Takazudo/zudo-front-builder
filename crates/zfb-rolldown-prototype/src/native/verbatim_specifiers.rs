//! Windows verbatim import specifiers. ZFB's shadow writers emit canonical
//! absolute imports (for example the islands entry importing each component),
//! so on Windows a module can import `\\?\C:\…`, which the pinned resolver
//! reports as `UNRESOLVED_IMPORT` (#4336 ZFB-L2-006, run 39409734870). Resolve
//! the ordinary spelling instead, under the same fail-closed rules as argv.
use super::windows_path;
use anyhow::{Context, Result};
use rolldown::plugin::{
    HookResolveIdArgs, HookResolveIdOutput, HookUsage, Plugin, PluginContext,
    PluginContextResolveOptions,
};
use std::{borrow::Cow, path::PathBuf};

#[derive(Debug)]
pub(super) struct VerbatimSpecifiers;

impl Plugin for VerbatimSpecifiers {
    fn name(&self) -> Cow<'static, str> {
        "zfb-prepared-verbatim-specifiers".into()
    }

    fn register_hook_usage(&self) -> HookUsage {
        HookUsage::ResolveId
    }

    async fn resolve_id(
        &self,
        ctx: &PluginContext,
        args: &HookResolveIdArgs<'_>,
    ) -> Result<Option<HookResolveIdOutput>> {
        let Some(ordinary) = ordinary_specifier(args.specifier, cfg!(windows))? else {
            return Ok(None);
        };
        let resolved = ctx
            .resolve(
                &ordinary,
                args.importer,
                Some(PluginContextResolveOptions {
                    import_kind: args.kind,
                    is_entry: args.is_entry,
                    custom: args.custom.clone(),
                    ..Default::default()
                }),
            )
            .await?;
        // Fail closed: no later resolver can handle the verbatim spelling.
        let resolved = resolved.with_context(|| {
            format!(
                "resolve {ordinary} (rewritten from {}) imported by {}",
                args.specifier,
                args.importer.unwrap_or("<entry>")
            )
        })?;
        Ok(Some(HookResolveIdOutput::from_resolved_id(resolved)))
    }
}

/// A tsconfig `paths` target in the ordinary spelling, so the ordered target
/// fallback re-resolves it as an ordinary path and can still try the next one.
pub(super) fn ordinary_target(target: PathBuf) -> Result<PathBuf> {
    let Some(text) = target.to_str() else {
        return Ok(target);
    };
    Ok(ordinary_specifier(text, cfg!(windows))?.map_or(target, PathBuf::from))
}

/// The ordinary spelling Rolldown should resolve, or `None` to leave the
/// specifier to the other resolvers. Elsewhere `\\?\` is an ordinary name.
fn ordinary_specifier(specifier: &str, windows: bool) -> Result<Option<String>> {
    if !windows {
        return Ok(None);
    }
    // ZFB's resource contract permits exactly one terminal `?url` suffix.
    let (path, suffix) = specifier
        .strip_suffix("?url")
        .map_or((specifier, ""), |path| (path, "?url"));
    if !suffix.is_empty() && path.ends_with('\\') {
        anyhow::bail!(
            "Rolldown prototype cannot resolve a ?url resource on a directory: {specifier}"
        );
    }
    Ok(match windows_path::for_rolldown(path)? {
        Cow::Borrowed(_) => None,
        Cow::Owned(ordinary) => Some(ordinary + suffix),
    })
}

#[cfg(test)]
mod tests {
    use super::ordinary_specifier;

    #[test]
    fn windows_islands_entry_import_resolves_its_ordinary_spelling() {
        let component = r"\\?\C:\Users\runneradmin\AppData\Local\Temp\site\components\counter.tsx";
        assert_eq!(
            ordinary_specifier(component, true).unwrap().as_deref(),
            Some(r"C:\Users\runneradmin\AppData\Local\Temp\site\components\counter.tsx")
        );
        assert_eq!(
            ordinary_specifier(r"\\?\UNC\host\share\site\counter.tsx", true)
                .unwrap()
                .as_deref(),
            Some(r"\\host\share\site\counter.tsx")
        );
    }

    #[test]
    fn the_terminal_url_suffix_survives_the_rewrite() {
        assert_eq!(
            ordinary_specifier(r"\\?\C:\site\logo.svg?url", true)
                .unwrap()
                .as_deref(),
            Some(r"C:\site\logo.svg?url")
        );
        assert!(ordinary_specifier(r"\\?\C:\site\logo.svg?raw", true).is_err());
        assert!(ordinary_specifier(r"\\?\C:\site\?url", true).is_err());
    }

    #[test]
    fn ordinary_specifiers_and_other_hosts_are_left_to_the_resolver() {
        for specifier in ["./counter.tsx", "preact", r"C:\site\counter.tsx", "#lib/x"] {
            assert_eq!(ordinary_specifier(specifier, true).unwrap(), None);
        }
        assert_eq!(
            ordinary_specifier(r"\\?\C:\site\counter.tsx", false).unwrap(),
            None
        );
    }

    #[test]
    fn verbatim_specifiers_that_cannot_be_rewritten_fail_closed() {
        let deep = format!(r"\\?\C:\{}counter.tsx", "segment\\".repeat(40));
        let error = format!("{:#}", ordinary_specifier(&deep, true).unwrap_err());
        assert!(error.contains("longer than MAX_PATH"), "{error}");
        assert!(ordinary_specifier(r"\\?\C:\site\NUL\counter.tsx", true).is_err());
    }
}
