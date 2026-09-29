use std::fs;
use std::path::Path;
use zudo_wind::{expand_file_set, PositiveRoot, SourceId, SourcePlan};

fn put(root: &Path, path: &str, bytes: &str) {
    let target = root.join(path);
    fs::create_dir_all(target.parent().unwrap()).unwrap();
    fs::write(target, bytes).unwrap();
}
fn root(base: &Path, label: &str, path: &str, required: bool) -> PositiveRoot {
    PositiveRoot {
        label: label.into(),
        declaring_dir: base.into(),
        path: path.into(),
        required,
        exclusions: Default::default(),
    }
}

#[test]
fn root_exclusions_do_not_hide_separately_declared_roots() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    put(base, "package/src/own.tsx", "");
    put(base, "package/projects/nested/src/app.tsx", "");
    put(base, "package/mirror/src/copy.tsx", "");
    put(base, "package/ignored/global.tsx", "");

    let mut package = root(base, "a-package", "package", true);
    package
        .exclusions
        .extend(["package/projects/nested".into(), "package/mirror".into()]);
    let mut plan = SourcePlan {
        roots: vec![
            package,
            root(base, "b-project", "package/projects/nested", true),
            root(base, "c-mirror", "package/mirror", true),
        ],
        ..Default::default()
    };
    plan.exclusions.insert(base.join("package/ignored"));

    let first = expand_file_set(&plan);
    assert_eq!(first, expand_file_set(&plan));
    assert!(first.diagnostics.is_empty());
    let ids: Vec<_> = first.files.iter().map(|file| file.id.render()).collect();
    assert_eq!(
        ids,
        [
            "a-package:src/own.tsx",
            "b-project:src/app.tsx",
            "c-mirror:src/copy.tsx",
        ]
    );
}

#[test]
fn walk_respects_root_boundaries_and_is_deterministic() {
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    put(base, ".gitignore", "ignored-parent/\n");
    put(base, "src/.gitignore", "blocked/\n");
    put(base, "src/keep/a.tsx", "");
    put(base, "src/keep/b.md", "");
    put(base, "src/keep/.gitignore", "nested/\n");
    put(base, "src/keep/nested/no.tsx", "");
    put(base, "src/blocked/x.tsx", "");
    put(base, "src/node_modules/x.tsx", "");
    put(base, "src/dist/x.tsx", "");
    put(base, "src/.zfb-build/x.tsx", "");
    put(base, "src/.secret/x.tsx", "");
    put(base, "src/excluded/x.tsx", "");
    put(base, ".hidden-root/x.tsx", "");
    put(base, "ignored-parent/declared/x.tsx", "");
    put(base, "package/node_modules/pkg/dist/x.tsx", "");
    put(base, "src/skip.html", "");
    let mut plan = SourcePlan {
        roots: vec![
            root(base, "src", "src", true),
            root(base, "hidden", ".hidden-root", true),
            root(base, "ignored", "ignored-parent/declared", true),
            root(base, "missing-conventional", "absent", false),
            root(base, "missing-required", "missing", true),
        ],
        ..Default::default()
    };
    plan.package_sources.insert(
        "pkg".into(),
        root(base, "pkg", "package/node_modules/pkg/dist", true),
    );
    plan.exclusions.insert(base.join("src/excluded"));
    let first = expand_file_set(&plan);
    assert_eq!(first, expand_file_set(&plan));
    let ids: Vec<_> = first.files.iter().map(|entry| entry.id.render()).collect();
    assert_eq!(
        ids,
        [
            "hidden:x.tsx",
            "ignored:x.tsx",
            "pkg:x.tsx",
            "src:keep/a.tsx",
            "src:keep/b.md"
        ]
    );
    assert_eq!(first.diagnostics.len(), 1);
    assert_eq!(first.diagnostics[0].root_label, "missing-required");
    assert!(ids
        .iter()
        .all(|id| !id.contains(&base.display().to_string())));
}

#[test]
fn relative_id_normalizes_and_rejects_escape() {
    let id = SourceId::new("app", "src/a/../b/file.tsx").unwrap();
    assert_eq!(id.render(), "app:src/b/file.tsx");
    assert!(SourceId::new("app", "../outside.tsx").is_err());
    assert!(SourceId::new("app", "/absolute.tsx").is_err());
    assert_eq!(
        SourceId::new("app", r"src\a\b.tsx").unwrap().render(),
        "app:src/a/b.tsx"
    );
    assert!(SourceId::new("app", r"C:\absolute.tsx").is_err());
}

#[cfg(unix)]
#[test]
fn symlink_directory_is_not_followed_but_explicit_symlink_root_is() {
    use std::os::unix::fs::symlink;
    let tmp = tempfile::tempdir().unwrap();
    let base = tmp.path();
    put(base, "src/real/a.tsx", "");
    symlink(base.join("src/real"), base.join("src/link")).unwrap();
    let mut plan = SourcePlan::default();
    plan.roots.push(root(base, "src", "src", true));
    let result = expand_file_set(&plan);
    assert_eq!(result.files.len(), 1);
    plan.roots = vec![root(base, "link", "src/link", true)];
    let result = expand_file_set(&plan);
    assert_eq!(result.files[0].id.render(), "link:a.tsx");
}

#[cfg(target_os = "linux")]
#[test]
fn non_utf8_filename_is_diagnosed_and_skipped() {
    use std::os::unix::ffi::OsStringExt;
    let tmp = tempfile::tempdir().unwrap();
    let filename = std::ffi::OsString::from_vec(b"bad\xff.tsx".to_vec());
    fs::write(tmp.path().join(filename), "").unwrap();
    let mut plan = SourcePlan::default();
    plan.roots.push(root(tmp.path(), "root", ".", true));
    let result = expand_file_set(&plan);
    assert!(result.files.is_empty());
    assert_eq!(result.diagnostics.len(), 1);
    assert!(result.diagnostics[0].message.contains("UTF-8"));
}

#[test]
fn explicit_file_is_indexed_through_shared_extractor() {
    let tmp = tempfile::tempdir().unwrap();
    put(tmp.path(), "entry.html", "<div class=\"p-1\"></div>");
    let mut plan = SourcePlan::default();
    plan.extensions.insert("html".into());
    plan.roots
        .push(root(tmp.path(), "entry", "entry.html", true));
    let files = expand_file_set(&plan);
    assert_eq!(files.files.len(), 1);
    let mut index = zudo_wind::CandidateIndex::default();
    let result = index.index_file(&files.files[0]).unwrap();
    assert!(result
        .candidates
        .iter()
        .any(|candidate| candidate.text == "p-1"));
    assert!(index.live_set().contains("p-1"));
}
