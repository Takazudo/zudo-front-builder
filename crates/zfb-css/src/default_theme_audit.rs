//! Audit-only reporting for authored Tailwind default theme references.

use std::path::{Path, PathBuf};

use crate::{
    undeclared_default_theme_var_references, AuditOutcome, AuditReport, DiagnosticView, OriginView,
};

/// Append ZW016 rows for the entry stylesheet and its resolved import closure.
pub fn append_default_theme_var_diagnostics(
    report: &mut AuditReport,
    stylesheets: &[(PathBuf, String)],
    project_root: &Path,
    strict: bool,
) {
    if report.outcome != AuditOutcome::Complete {
        return;
    }

    for (path, reference) in undeclared_default_theme_var_references(stylesheets) {
        let source_id = project_relative_path(&path, project_root)
            .to_string_lossy()
            .replace('\\', "/");
        report.diagnostics.push(DiagnosticView {
            severity: if strict { "warning" } else { "auditInfo" }.to_owned(),
            code: "ZW016".to_owned(),
            candidate: Some(reference.name.clone()),
            origin: Some(OriginView {
                kind: "source".to_owned(),
                source_id: Some(source_id.clone()),
                path: Some(source_id),
                line: Some(reference.line),
                byte_column: Some(reference.column),
                byte_offset: Some(reference.offset),
                ..OriginView::default()
            }),
            message: format!(
                "references Tailwind default theme variable {}, which no scanned stylesheet declares; declare it in authored CSS, point at the emitted --zw-… token variable, or add a var() fallback",
                reference.name
            ),
            suggestion: None,
            rejection_id: None,
        });
    }
    zudo_wind::finalize_audit_report(report);
}

fn project_relative_path(path: &Path, project_root: &Path) -> PathBuf {
    if let Ok(relative) = path.strip_prefix(project_root) {
        return relative.to_path_buf();
    }
    if path.is_absolute() != project_root.is_absolute() {
        return path.to_path_buf();
    }
    let path_parts: Vec<_> = path.components().collect();
    let root_parts: Vec<_> = project_root.components().collect();
    let common = path_parts
        .iter()
        .zip(&root_parts)
        .take_while(|(path, root)| path == root)
        .count();
    let mut relative = PathBuf::new();
    for _ in common..root_parts.len() {
        relative.push("..");
    }
    for part in &path_parts[common..] {
        relative.push(part.as_os_str());
    }
    relative
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imported_stylesheet_outside_project_root_has_relative_origin() {
        let root = Path::new("/workspace/site");
        let stylesheet = Path::new("/workspace/design-system/tokens.css");
        assert_eq!(
            project_relative_path(stylesheet, root),
            PathBuf::from("../design-system/tokens.css")
        );
        assert_eq!(
            project_relative_path(Path::new("/workspace/site/src/main.css"), root),
            PathBuf::from("src/main.css")
        );
    }
}
