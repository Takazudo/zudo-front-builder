//! Windows verbatim import specifiers. ZFB's shadow writers emit canonical
//! absolute imports (for example the islands entry importing each component),
//! so on Windows a module can import `\\?\C:\…`, which the pinned resolver
//! reports as `UNRESOLVED_IMPORT` (#4336 ZFB-L2-006, run 39409734870). Resolve
//! the ordinary spelling instead, under the same fail-closed rules as argv.
use super::windows_path;
use anyhow::Result;
use rolldown::plugin::{
    HookResolveIdArgs, HookResolveIdOutput, HookUsage, Plugin, PluginContext,
    PluginContextResolveOptions,
};
use std::borrow::Cow;

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
        Ok(Some(HookResolveIdOutput::from_resolved_id(resolved?)))
    }
}

/// The ordinary spelling Rolldown should resolve, or `None` to leave the
/// specifier to the other resolvers. Elsewhere `\\?\` is an ordinary name.
fn ordinary_specifier(specifier: &str, windows: bool) -> Result<Option<String>> {
    if !windows {
        return Ok(None);
    }
    Ok(match windows_path::for_rolldown(specifier)? {
        Cow::Borrowed(_) => None,
        Cow::Owned(ordinary) => Some(ordinary),
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
