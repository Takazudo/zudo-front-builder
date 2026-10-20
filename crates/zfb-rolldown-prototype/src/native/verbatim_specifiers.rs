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
use rolldown_resolver::ResolveError;
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
        // A miss stays a miss: TrailingPaths re-resolves tsconfig targets in
        // order and needs NotFound to try the next one, and no later resolver
        // can resolve the verbatim spelling it falls back to.
        if let Err(ResolveError::NotFound(_)) = resolved {
            return Ok(None);
        }
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

/// A tsconfig target in the ordinary spelling, so the trailing-directory
/// probes see real components and ids without `\\?\`. A target that cannot be
/// rewritten is kept as is and fails closed if it is ever resolved.
pub(super) fn ordinary_target(target: PathBuf, windows: bool) -> PathBuf {
    let Some(text) = target.to_str() else {
        return target;
    };
    if !windows || !text.starts_with(r"\\?\") {
        return target;
    }
    match windows_path::for_rolldown(&text.replace('/', "\\")) {
        Ok(Cow::Owned(ordinary)) => PathBuf::from(ordinary),
        _ => target,
    }
}

/// The ordinary spelling Rolldown should resolve, or `None` to leave the
/// specifier to the other resolvers. Elsewhere `\\?\` is an ordinary name.
fn ordinary_specifier(specifier: &str, windows: bool) -> Result<Option<String>> {
    if !windows || !specifier.starts_with(r"\\?\") {
        return Ok(None);
    }
    // A tsconfig wildcard substitutes the import's subpath with `/`, which
    // separates components once the prefix is gone; inside the verbatim
    // spelling it would be a literal name character instead.
    let specifier = specifier.replace('/', "\\");
    // ZFB's resource contract permits exactly one terminal `?url` suffix.
    let (path, suffix) = specifier
        .strip_suffix("?url")
        .map_or((specifier.as_str(), ""), |path| (path, "?url"));
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
    use super::{ordinary_specifier, ordinary_target};
    use std::path::PathBuf;

    #[test]
    fn trailing_tsconfig_targets_keep_their_separator_in_the_ordinary_spelling() {
        let target = |value: &str| ordinary_target(PathBuf::from(value), true);
        assert_eq!(
            target(r"\\?\C:\shadow\choice\"),
            PathBuf::from(r"C:\shadow\choice\")
        );
        // ZFB re-appends the original separator, which may be `/`.
        assert_eq!(
            target(r"\\?\C:\shadow\choice/"),
            PathBuf::from(r"C:\shadow\choice\")
        );
        assert_eq!(
            target(r"\\?\C:\shadow\src\a/b"),
            PathBuf::from(r"C:\shadow\src\a\b")
        );
        // Unrewritable or ordinary targets, and other hosts, are untouched.
        for kept in [r"\\?\C:\shadow\NUL\", r"C:\shadow\choice\", "/tmp/choice/"] {
            assert_eq!(target(kept), PathBuf::from(kept));
        }
        assert_eq!(
            ordinary_target(PathBuf::from(r"\\?\C:\shadow\choice\"), false),
            PathBuf::from(r"\\?\C:\shadow\choice\")
        );
    }

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
    fn a_tsconfig_wildcard_subpath_separates_components() {
        assert_eq!(
            ordinary_specifier(r"\\?\C:\shadow\src\components/button\", true)
                .unwrap()
                .as_deref(),
            Some(r"C:\shadow\src\components\button\")
        );
        assert!(ordinary_specifier(r"\\?\C:\shadow\src\../x", true).is_err());
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
        assert!(ordinary_specifier(r"\\?\C:\site\assets/?url", true).is_err());
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
