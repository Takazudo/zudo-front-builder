use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use zfb_types::normalize_path_lexical;

/// First-party TypeScript path/baseUrl mappings available to terminal
/// `?raw` target resolution.
///
/// Targets are stored in the same logical absolute shape as
/// `zfb_build::bundler::BundlerInput::tsconfig_paths`: under-project mappings
/// point at the real project root, not a preprocessing shadow.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawImportAliasContext {
    paths: BTreeMap<String, Vec<String>>,
    base_url: Option<PathBuf>,
}

impl RawImportAliasContext {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn from_paths(paths: &BTreeMap<String, Vec<String>>) -> Self {
        Self::from_paths_and_base_url(paths, None)
    }

    pub fn from_paths_and_base_url(
        paths: &BTreeMap<String, Vec<String>>,
        base_url: Option<PathBuf>,
    ) -> Self {
        Self {
            paths: paths.clone(),
            base_url: base_url.map(|path| normalize_path_lexical(&path)),
        }
    }

    pub fn from_tsconfig_paths(paths: &crate::TsConfigPaths) -> Self {
        let mut map = BTreeMap::new();
        for alias in &paths.aliases {
            let targets = alias
                .targets
                .iter()
                .map(|target| crate::resolve_tsconfig_path_target(&paths.base_dir, target))
                .collect::<Vec<_>>();
            if !targets.is_empty() {
                map.insert(alias.pattern.clone(), targets);
            }
        }
        Self::from_paths_and_base_url(&map, paths.base_url.clone())
    }

    pub fn from_project_root(project_root: &Path) -> Self {
        crate::read_tsconfig_paths(project_root)
            .as_ref()
            .map(Self::from_tsconfig_paths)
            .unwrap_or_default()
    }

    pub fn from_paths_and_project_base_url(
        paths: &BTreeMap<String, Vec<String>>,
        project_root: &Path,
    ) -> Self {
        let base_url = crate::read_tsconfig_paths(project_root).and_then(|paths| paths.base_url);
        Self::from_paths_and_base_url(paths, base_url)
    }
}

pub fn is_relative_specifier(specifier: &str) -> bool {
    specifier.starts_with("./") || specifier.starts_with("../")
}

fn unsupported_target_error(importer: &Path, specifier_without_query: &str) -> anyhow::Error {
    anyhow!(
        "zfb bundler: unsupported ?raw target {specifier_without_query:?} in {}. \
         Only project-local relative targets in the form \
         `import text from \"./file.ext?raw\"` or exact first-party \
         tsconfig-alias/baseUrl targets are supported.",
        importer.display()
    )
}

enum PathAliasResolution {
    NoMatch,
    Matched(Option<PathBuf>),
}

fn match_tsconfig_pattern(pattern: &str, specifier: &str) -> Option<Option<String>> {
    match pattern.matches('*').count() {
        0 => (pattern == specifier).then_some(None),
        1 => {
            let star = pattern.find('*')?;
            let prefix = &pattern[..star];
            let suffix = &pattern[star + 1..];
            if specifier.len() < prefix.len() + suffix.len()
                || !specifier.starts_with(prefix)
                || !specifier.ends_with(suffix)
            {
                return None;
            }
            Some(Some(
                specifier[prefix.len()..specifier.len().saturating_sub(suffix.len())].to_string(),
            ))
        }
        _ => None,
    }
}

fn tsconfig_pattern_specificity(pattern: &str) -> usize {
    pattern
        .find('*')
        .map(|star| star.max(pattern.len().saturating_sub(star + 1)))
        .unwrap_or(pattern.len())
}

fn substitute_tsconfig_target(target: &str, capture: Option<&str>) -> String {
    match (target.find('*'), capture) {
        (Some(star), Some(capture)) => {
            let mut out = String::with_capacity(target.len() + capture.len());
            out.push_str(&target[..star]);
            out.push_str(capture);
            out.push_str(&target[star + 1..]);
            out
        }
        _ => target.to_string(),
    }
}

fn path_alias_candidate(
    aliases: &RawImportAliasContext,
    specifier_without_query: &str,
) -> PathAliasResolution {
    let mut matches = aliases
        .paths
        .iter()
        .filter_map(|(pattern, targets)| {
            match_tsconfig_pattern(pattern, specifier_without_query).map(|capture| {
                (
                    pattern.as_str(),
                    targets,
                    capture,
                    tsconfig_pattern_specificity(pattern),
                )
            })
        })
        .collect::<Vec<_>>();
    if matches.is_empty() {
        return PathAliasResolution::NoMatch;
    }
    matches.sort_by_key(|(_, _, _, specificity)| std::cmp::Reverse(*specificity));

    let mut first_candidate = None;
    for (_, targets, capture, _) in matches {
        for target in targets {
            let substituted = substitute_tsconfig_target(target, capture.as_deref());
            let candidate = normalize_path_lexical(Path::new(&substituted));
            first_candidate.get_or_insert_with(|| candidate.clone());
            if candidate.is_file() {
                return PathAliasResolution::Matched(Some(candidate));
            }
        }
    }
    PathAliasResolution::Matched(first_candidate)
}

fn base_url_candidate(
    aliases: &RawImportAliasContext,
    specifier_without_query: &str,
) -> Option<PathBuf> {
    let base_url = aliases.base_url.as_ref()?;
    Some(normalize_path_lexical(
        &base_url.join(specifier_without_query),
    ))
}

pub fn validate_raw_candidate(
    importer: &Path,
    specifier_without_query: &str,
    project_root: &Path,
    candidate: PathBuf,
) -> Result<PathBuf> {
    let lexical_root = normalize_path_lexical(project_root);
    if !candidate.starts_with(&lexical_root) {
        bail!(
            "zfb bundler: raw import target {specifier_without_query:?} from {} escapes the \
             logical project root {} (resolved lexical path {}). Raw targets must remain \
             project-local.",
            importer.display(),
            lexical_root.display(),
            candidate.display()
        );
    }
    if !candidate.is_file() {
        bail!(
            "zfb bundler: raw import target {specifier_without_query:?} from {} does not \
             resolve to an existing file (looked for {}).",
            importer.display(),
            candidate.display()
        );
    }
    let canonical_root = project_root.canonicalize().with_context(|| {
        format!(
            "canonicalize logical project root {} for ?raw containment",
            project_root.display()
        )
    })?;
    let canonical_target = candidate.canonicalize().with_context(|| {
        format!(
            "canonicalize raw import target {} before reading it",
            candidate.display()
        )
    })?;
    if !canonical_target.starts_with(&canonical_root) {
        bail!(
            "zfb bundler: raw import target {} from {} escapes the project root {} through \
             a symlink (canonical target {}). Raw targets must remain project-local.",
            candidate.display(),
            importer.display(),
            canonical_root.display(),
            canonical_target.display()
        );
    }
    // Preserve the lexical project-root spelling for `bundle.exclude`
    // relativisation and invalidation aliases (notably macOS `/var` ->
    // `/private/var` temp roots). Consumers register both this logical path
    // and its current canonical target.
    Ok(candidate)
}

/// Resolve a raw target without routing the stripped path through the normal
/// JavaScript module resolver.
///
/// Relative spellings (`./` or `../`) are unchanged. Non-relative spellings
/// may resolve only through first-party TypeScript `paths`/`baseUrl`
/// mappings. An exact file must exist; no JS extension probing or `index.*`
/// resolution is performed because the edge is terminal and accepts any
/// extension.
pub fn resolve_raw_target_with_aliases(
    importer: &Path,
    specifier_without_query: &str,
    project_root: &Path,
    aliases: &RawImportAliasContext,
) -> Result<PathBuf> {
    if is_relative_specifier(specifier_without_query) {
        let importer_dir = importer.parent().unwrap_or_else(|| Path::new("."));
        let candidate = normalize_path_lexical(&importer_dir.join(specifier_without_query));
        return validate_raw_candidate(importer, specifier_without_query, project_root, candidate);
    }
    if Path::new(specifier_without_query).is_absolute() {
        return Err(unsupported_target_error(importer, specifier_without_query));
    }

    match path_alias_candidate(aliases, specifier_without_query) {
        PathAliasResolution::Matched(Some(candidate)) => {
            validate_raw_candidate(importer, specifier_without_query, project_root, candidate)
        }
        PathAliasResolution::Matched(None) => {
            Err(unsupported_target_error(importer, specifier_without_query))
        }
        PathAliasResolution::NoMatch => {
            if let Some(candidate) = base_url_candidate(aliases, specifier_without_query) {
                validate_raw_candidate(importer, specifier_without_query, project_root, candidate)
            } else {
                Err(unsupported_target_error(importer, specifier_without_query))
            }
        }
    }
}

/// Resolve a raw target using the nearest root TypeScript mappings, preserving
/// backward compatibility for callers that do not thread an explicit context.
pub fn resolve_raw_target(
    importer: &Path,
    specifier_without_query: &str,
    project_root: &Path,
) -> Result<PathBuf> {
    let aliases = RawImportAliasContext::from_project_root(project_root);
    resolve_raw_target_with_aliases(importer, specifier_without_query, project_root, &aliases)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alias_context(
        paths: impl IntoIterator<Item = (&'static str, Vec<String>)>,
        base_url: Option<PathBuf>,
    ) -> RawImportAliasContext {
        RawImportAliasContext::from_paths_and_base_url(
            &paths
                .into_iter()
                .map(|(pattern, targets)| (pattern.to_string(), targets))
                .collect(),
            base_url,
        )
    }

    #[test]
    fn base_url_only_raw_import_resolves_exact_file() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir_all(root.join("src/data")).unwrap();
        let importer = root.join("entry.ts");
        let target = root.join("src/data/payload.txt");
        std::fs::write(&importer, "placeholder").unwrap();
        std::fs::write(&target, "base-url raw").unwrap();
        let aliases = alias_context(std::iter::empty(), Some(root.join("src")));

        let resolved =
            resolve_raw_target_with_aliases(&importer, "data/payload.txt", root, &aliases).unwrap();

        assert_eq!(resolved, target);
    }

    #[test]
    fn paths_mapping_wins_over_base_url_raw_target() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir_all(root.join("aliased/theme")).unwrap();
        std::fs::create_dir_all(root.join("base/theme")).unwrap();
        let importer = root.join("entry.ts");
        let aliased = root.join("aliased/theme/palette.txt");
        let base = root.join("base/theme/palette.txt");
        std::fs::write(&importer, "placeholder").unwrap();
        std::fs::write(&aliased, "paths").unwrap();
        std::fs::write(&base, "base").unwrap();
        let aliases = alias_context(
            [(
                "theme/*",
                vec![root.join("aliased/theme/*").to_string_lossy().into_owned()],
            )],
            Some(root.join("base")),
        );

        let resolved =
            resolve_raw_target_with_aliases(&importer, "theme/palette.txt", root, &aliases)
                .unwrap();

        assert_eq!(resolved, aliased);
    }

    #[test]
    fn raw_alias_multi_target_uses_first_existing_exact_file() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir_all(root.join("second/icons")).unwrap();
        std::fs::create_dir_all(root.join("third/icons")).unwrap();
        let importer = root.join("entry.ts");
        let second = root.join("second/icons/logo.svg");
        let third = root.join("third/icons/logo.svg");
        std::fs::write(&importer, "placeholder").unwrap();
        std::fs::write(&second, "second").unwrap();
        std::fs::write(&third, "third").unwrap();
        let aliases = alias_context(
            [(
                "@/*",
                vec![
                    root.join("missing/*").to_string_lossy().into_owned(),
                    root.join("second/*").to_string_lossy().into_owned(),
                    root.join("third/*").to_string_lossy().into_owned(),
                ],
            )],
            None,
        );

        let resolved =
            resolve_raw_target_with_aliases(&importer, "@/icons/logo.svg", root, &aliases).unwrap();

        assert_eq!(resolved, second);
    }

    #[test]
    fn raw_alias_does_not_probe_extensions_or_index_files() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        std::fs::create_dir_all(root.join("src/payload")).unwrap();
        let importer = root.join("entry.ts");
        std::fs::write(&importer, "placeholder").unwrap();
        std::fs::write(root.join("src/payload.txt"), "extension").unwrap();
        std::fs::write(root.join("src/payload/index.txt"), "index").unwrap();
        let aliases = alias_context(
            [(
                "@/*",
                vec![root.join("src/*").to_string_lossy().into_owned()],
            )],
            None,
        );

        let error = resolve_raw_target_with_aliases(&importer, "@/payload", root, &aliases)
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("does not resolve to an existing file"),
            "{error}"
        );
        let expected = root.join("src/payload");
        assert!(
            error.contains(expected.to_string_lossy().as_ref()),
            "{error}"
        );
    }

    #[test]
    fn missing_raw_alias_reports_expanded_exact_path() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("entry.ts");
        std::fs::write(&importer, "placeholder").unwrap();
        let missing = root.join("src/missing.txt");
        let aliases = alias_context(
            [(
                "@/*",
                vec![root.join("src/*").to_string_lossy().into_owned()],
            )],
            None,
        );

        let error = resolve_raw_target_with_aliases(&importer, "@/missing.txt", root, &aliases)
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("does not resolve to an existing file"),
            "{error}"
        );
        assert!(
            error.contains(missing.to_string_lossy().as_ref()),
            "{error}"
        );
    }

    #[test]
    fn unmapped_bare_raw_specifier_mentions_tsconfig_alias_support() {
        let project = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("entry.ts");
        std::fs::write(&importer, "placeholder").unwrap();
        let aliases = RawImportAliasContext::empty();

        let error = resolve_raw_target_with_aliases(&importer, "pkg/payload.txt", root, &aliases)
            .unwrap_err()
            .to_string();

        assert!(error.contains("unsupported ?raw target"), "{error}");
        assert!(error.contains("tsconfig-alias"), "{error}");
    }

    #[test]
    fn raw_alias_lexical_escape_is_rejected_before_read() {
        let outer = tempfile::tempdir().unwrap();
        let root = outer.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        let importer = root.join("entry.ts");
        let secret = outer.path().join("secret.txt");
        std::fs::write(&importer, "placeholder").unwrap();
        std::fs::write(&secret, "secret").unwrap();
        let aliases = alias_context(
            [("secret", vec![secret.to_string_lossy().into_owned()])],
            None,
        );

        let error = resolve_raw_target_with_aliases(&importer, "secret", &root, &aliases)
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("escapes the logical project root"),
            "{error}"
        );
    }

    #[cfg(unix)]
    #[test]
    fn raw_alias_symlink_escape_is_rejected_before_read() {
        let project = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let root = project.path();
        let importer = root.join("entry.ts");
        let outside_target = outside.path().join("secret.txt");
        std::fs::write(&importer, "placeholder").unwrap();
        std::fs::write(&outside_target, "secret").unwrap();
        std::os::unix::fs::symlink(&outside_target, root.join("secret.txt")).unwrap();
        let aliases = alias_context(
            [(
                "secret",
                vec![root.join("secret.txt").to_string_lossy().into_owned()],
            )],
            None,
        );

        let error = resolve_raw_target_with_aliases(&importer, "secret", root, &aliases)
            .unwrap_err()
            .to_string();

        assert!(error.contains("escapes the project root"), "{error}");
        assert!(error.contains("through a symlink"), "{error}");
    }

    #[test]
    fn overlapping_tsconfig_pattern_does_not_panic_or_false_match() {
        assert_eq!(match_tsconfig_pattern("a*a", "a"), None);
        assert_eq!(
            match_tsconfig_pattern("a*a", "aba"),
            Some(Some("b".to_string()))
        );
    }
}
