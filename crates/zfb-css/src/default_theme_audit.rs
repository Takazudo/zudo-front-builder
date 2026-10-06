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
        let relative = path.strip_prefix(project_root).unwrap_or(&path);
        let source_id = relative.to_string_lossy().replace('\\', "/");
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
