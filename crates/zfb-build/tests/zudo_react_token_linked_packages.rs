#![cfg(unix)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use zfb_build::bundler::{
    zudo_react_build_token_with_aliases, zudo_react_build_token_with_inputs_and_output,
    LinkedPackageIgnoreSnapshot,
};

struct Fixture {
    root: tempfile::TempDir,
    app: PathBuf,
    widget: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        Self::from_root(tempfile::tempdir().unwrap())
    }

    fn from_root(root: tempfile::TempDir) -> Self {
        let app = root.path().join("app");
        let widget = root.path().join("widget");
        fs::create_dir_all(app.join("node_modules")).unwrap();
        fs::create_dir_all(widget.join("dist")).unwrap();
        fs::create_dir_all(widget.join("src")).unwrap();
        fs::write(
            root.path().join("pnpm-workspace.yaml"),
            "packages:\n  - app\n",
        )
        .unwrap();
        fs::write(
            root.path().join(".gitignore"),
            "node_modules/\n.zfb-build/\ndist/\ntest-results/\n",
        )
        .unwrap();
        fs::write(
            widget.join("package.json"),
            r#"{"name":"widget","exports":"./dist/index.js"}"#,
        )
        .unwrap();
        fs::write(widget.join("dist/index.js"), "export const value = 1;\n").unwrap();
        fs::write(widget.join("src/x.ts"), "export const x = 1;\n").unwrap();
        Self { root, app, widget }
    }

    fn token(&self, entry: &str) -> String {
        let aliases = if entry == "alias" {
            vec![(
                "widget".into(),
                self.widget.join("dist/index.js").display().to_string(),
            )]
        } else {
            vec![]
        };
        if entry == "symlink" {
            let link = self.app.join("node_modules/widget");
            if !link.exists() {
                symlink(&self.widget, link).unwrap();
            }
        }
        if entry == "tsconfig" {
            fs::write(self.app.join("tsconfig.json"), serde_json::json!({"compilerOptions":{"baseUrl":".","paths":{"widget":["../widget/dist/index.js"]}}}).to_string()).unwrap();
        }
        zudo_react_build_token_with_aliases(&self.app, &aliases).unwrap()
    }
}

fn write(path: &Path, value: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, value).unwrap();
}

#[test]
fn linked_package_snapshot_classifies_only_ignored_undeclared_paths() {
    let f = Fixture::new();
    write(
        &f.root.path().join(".gitignore"),
        "node_modules/\ndist/\ntest-results/\nprivate/\n",
    );
    write(&f.widget.join(".gitignore"), "!test-results/negated.json\n");
    write(&f.widget.join("test-results/.gitignore"), "*.log\n");
    write(&f.widget.join("private/kept.js"), "export {};");
    let root = fs::canonicalize(&f.widget).unwrap();
    let project = fs::canonicalize(&f.app).unwrap();
    let mut snapshot = LinkedPackageIgnoreSnapshot::capture(&root, &project).unwrap();
    assert_eq!(snapshot.package_root(), root);
    for rule in [
        fs::canonicalize(f.root.path()).unwrap().join(".gitignore"),
        root.join(".gitignore"),
        root.join("test-results/.gitignore"),
        root.join("package.json"),
    ] {
        assert!(snapshot.rule_file_paths().contains(&rule));
        assert!(!snapshot.ignored_and_undeclared(&rule, false));
    }
    assert!(snapshot.ignored_and_undeclared(&root.join("test-results/x.json"), false));
    assert!(snapshot.ignored_and_undeclared(&root.join("private/other.json"), false));
    assert!(!snapshot.ignored_and_undeclared(&root.join("dist/index.js"), false));
    assert!(!snapshot.ignored_and_undeclared(&root.join("package.json"), false));
    assert!(!snapshot.ignored_and_undeclared(&root.join("test-results/negated.json"), false));
    assert!(!snapshot.ignored_and_undeclared(&f.app.join("outside.json"), false));

    let member = root.join("private/kept.js");
    snapshot.extend_closure_members([member.clone()]);
    assert!(snapshot.closure_members().contains(&member));
    assert!(!snapshot.ignored_and_undeclared(&member, false));
    assert!(!snapshot.ignored_and_undeclared(&root.join("private"), true));
}

#[test]
fn ignored_artifacts_hidden_files_and_real_sources_across_entry_points() {
    for entry in ["symlink", "alias", "tsconfig"] {
        let f = Fixture::new();
        let initial = f.token(entry);
        // Captured before the snapshot refactor in CI run 37684324151 at
        // eba1694472cc9930e16eaebabe4eabab9cc94e59. Relative tsconfig
        // paths keep these byte identities independent of random temp roots.
        let expected_initial = match entry {
            "symlink" => "9d34c7b6d4efcacd",
            "alias" => "aac02ca5ce06de10",
            "tsconfig" => "93c1dc2ddf8abc2a",
            _ => unreachable!(),
        };
        assert_eq!(initial, expected_initial, "{entry}: captured baseline");
        for (name, value) in [
            ("probe.json", "{}"),
            ("error-context.md", "oops"),
            ("report.html", "bad"),
        ] {
            write(&f.widget.join("test-results").join(name), value);
            assert_eq!(initial, f.token(entry), "{entry}: {name}");
        }
        write(&f.widget.join(".wrangler/state.json"), "{}");
        assert_eq!(initial, f.token(entry), "{entry}: hidden");
        write(&f.widget.join("dist/index.js"), "export const value = 2;\n");
        let after_dist = f.token(entry);
        let expected_dist = match entry {
            "symlink" => "c358b9b876122c71",
            "alias" => "abdf94ce0346be21",
            "tsconfig" => "fed4815a903e7915",
            _ => unreachable!(),
        };
        assert_eq!(after_dist, expected_dist, "{entry}: declared dist baseline");
        assert_ne!(initial, after_dist, "{entry}: declared dist");
        write(&f.widget.join("src/x.ts"), "export const x = 2;\n");
        let after_source = f.token(entry);
        let expected_source = match entry {
            "symlink" => "00569e67b0f2d49d",
            "alias" => "24675e179a046bea",
            "tsconfig" => "9a90f4b06e66435b",
            _ => unreachable!(),
        };
        assert_eq!(after_source, expected_source, "{entry}: source baseline");
        assert_ne!(after_dist, after_source, "{entry}: source");
    }
}

#[test]
fn hidden_files_are_pruned_without_any_gitignore() {
    let f = Fixture::new();
    fs::remove_file(f.root.path().join(".gitignore")).unwrap();
    let first = f.token("symlink");
    assert_eq!(first, "9d34c7b6d4efcacd");
    write(&f.widget.join(".wrangler/state.json"), "1");
    assert_eq!(first, f.token("symlink"));
}

#[test]
fn package_ignore_adds_rules_and_negation_reincludes_one_file() {
    let f = Fixture::new();
    let initial = f.token("symlink");
    assert_eq!(initial, "9d34c7b6d4efcacd");
    write(
        &f.widget.join(".gitignore"),
        "*.log\n!test-results/keep.json\n",
    );
    let with_rules = f.token("symlink");
    assert_eq!(with_rules, "9d34c7b6d4efcacd");
    write(&f.widget.join("test-results/no.json"), "1");
    assert_eq!(with_rules, f.token("symlink"));
    write(&f.widget.join("test-results/keep.json"), "1");
    let negated = f.token("symlink");
    assert_eq!(negated, "cba1047455add88e");
    assert_ne!(with_rules, negated);
    assert_eq!(initial, with_rules);
}

#[test]
fn nested_gitignore_patterns_stay_scoped_to_their_directory() {
    let f = Fixture::new();
    write(&f.widget.join("sub/.gitignore"), "/x.json\n");
    let first = f.token("symlink");
    assert_eq!(first, "9d34c7b6d4efcacd");
    write(&f.widget.join("sub/x.json"), "1");
    assert_eq!(first, f.token("symlink"), "anchored nested rule");
    write(&f.widget.join("x.json"), "1");
    let outside = f.token("symlink");
    assert_eq!(outside, "bb29fa7ce215adf2");
    assert_ne!(first, outside, "nested rule must not ignore package root");

    write(&f.widget.join("sub/.gitignore"), "*.json\n");
    let second = f.token("symlink");
    assert_eq!(second, "bb29fa7ce215adf2");
    write(&f.widget.join("sub/deep/y.json"), "1");
    assert_eq!(second, f.token("symlink"), "nested wildcard rule");
    write(&f.widget.join("other.json"), "1");
    let root_json = f.token("symlink");
    assert_eq!(root_json, "a60f932ad0b675b2");
    assert_ne!(second, root_json, "nested wildcard must not escape sub");
}

#[test]
fn bare_imports_target_does_not_reinclude_an_ignored_root_file() {
    let f = Fixture::new();
    write(
        &f.root.path().join(".gitignore"),
        "node_modules/\ndependency.js\n",
    );
    write(
        &f.widget.join("package.json"),
        r##"{"name":"widget","exports":"./dist/index.js","imports":{"#dep":"dependency.js"}}"##,
    );
    write(
        &f.widget.join("dependency.js"),
        "export const dependency = 1;",
    );
    let first = f.token("symlink");
    assert_eq!(first, "66d2973acfbe6763");
    write(
        &f.widget.join("dependency.js"),
        "export const dependency = 2;",
    );
    assert_eq!(first, f.token("symlink"));
}

#[test]
fn git_file_bounds_ignores_and_rules_above_bound_do_not_apply() {
    let outside = tempfile::tempdir().unwrap();
    write(&outside.path().join(".gitignore"), "widget/src/\n");
    let f = Fixture::from_root(tempfile::tempdir_in(outside.path()).unwrap());
    write(&f.root.path().join(".git"), "gitdir: elsewhere\n");
    let initial = f.token("symlink");
    assert_eq!(initial, "9d34c7b6d4efcacd");
    assert_eq!(initial, f.token("symlink"));
    write(&f.widget.join("src/x.ts"), "export const x = 2;\n");
    let changed = f.token("symlink");
    assert_eq!(changed, "c548a33ae74e5445");
    assert_ne!(initial, changed);
}

#[test]
fn external_git_file_is_an_ignore_bound() {
    let f = Fixture::new();
    fs::remove_file(f.root.path().join("pnpm-workspace.yaml")).unwrap();
    write(&f.widget.join(".git"), "gitdir: elsewhere\n");
    write(&f.widget.join(".gitignore"), "test-results/\n");
    let first = f.token("symlink");
    assert_eq!(first, "9d34c7b6d4efcacd");
    write(&f.widget.join("test-results/probe.json"), "1");
    assert_eq!(first, f.token("symlink"));
    write(&f.widget.join("src/x.ts"), "export const x = 2;\n");
    let changed = f.token("symlink");
    assert_eq!(changed, "c548a33ae74e5445");
    assert_ne!(first, changed);
}

#[test]
fn declared_lib_and_exact_root_main_are_included() {
    let f = Fixture::new();
    write(&f.widget.join("lib/index.js"), "export const lib = 1");
    write(
        &f.root.path().join(".gitignore"),
        "node_modules/\nlib/\nindex.js\nhelper.js\n",
    );
    write(
        &f.widget.join("package.json"),
        r#"{"name":"widget","exports":"./lib/index.js","main":"./index.js"}"#,
    );
    write(&f.widget.join("index.js"), "import './helper.js';");
    write(&f.widget.join("helper.js"), "export const helper = 1;");
    let first = f.token("symlink");
    assert_eq!(first, "93f409b7e3167e14");
    write(&f.widget.join("lib/index.js"), "export const lib = 2");
    let second = f.token("symlink");
    assert_eq!(second, "9b9894fb73d9f564");
    assert_ne!(first, second);
    write(&f.widget.join("helper.js"), "export const helper = 2;");
    let changed_helper = f.token("symlink");
    assert_eq!(changed_helper, "51adb485ec612341");
    assert_ne!(second, changed_helper);
}

#[test]
fn repeated_alias_target_seeds_ignored_relative_closure() {
    let f = Fixture::new();
    write(
        &f.root.path().join(".gitignore"),
        "node_modules/\nprivate/\n",
    );
    write(&f.widget.join("private/target.js"), "import './dep.js';");
    write(&f.widget.join("private/dep.js"), "export const dep = 1;");
    write(&f.widget.join("private/unrelated.json"), "1");
    let aliases = vec![
        (
            "a".into(),
            f.widget.join("dist/index.js").display().to_string(),
        ),
        (
            "b".into(),
            f.widget.join("private/target.js").display().to_string(),
        ),
    ];
    let token = || {
        zudo_react_build_token_with_inputs_and_output(
            &f.app,
            &aliases,
            &[],
            &f.app.join("dist"),
            &Default::default(),
        )
        .unwrap()
    };
    let first = token();
    assert_eq!(first, "140f75f49c27ec15");
    write(&f.widget.join("private/unrelated.json"), "2");
    assert_eq!(first, token());
    write(&f.widget.join("private/dep.js"), "export const dep = 2;");
    let changed_dependency = token();
    assert_eq!(changed_dependency, "9677fc2fc2e15bc1");
    assert_ne!(first, changed_dependency);
}

#[test]
fn no_bound_falls_back_to_package_and_ignored_vendor_does_not_hide_package() {
    let f = Fixture::new();
    fs::remove_file(f.root.path().join("pnpm-workspace.yaml")).unwrap();
    let first = f.token("symlink");
    assert_eq!(first, "9d34c7b6d4efcacd");
    write(&f.widget.join("test-results/probe.json"), "1");
    let unbounded = f.token("symlink");
    assert_eq!(unbounded, "54cfee4a02c579c7");
    assert_ne!(first, unbounded);
    let f = Fixture::new();
    let vendor = f.root.path().join("vendor/widget");
    fs::create_dir_all(vendor.parent().unwrap()).unwrap();
    fs::rename(&f.widget, &vendor).unwrap();
    symlink(&vendor, f.app.join("node_modules/widget")).unwrap();
    write(&f.root.path().join(".gitignore"), "vendor/\n");
    let first = f.token("symlink");
    assert_eq!(first, "9d34c7b6d4efcacd");
    write(&vendor.join("src/x.ts"), "export const x = 3;");
    let changed_vendor = f.token("symlink");
    assert_eq!(changed_vendor, "68ec385dfa848846");
    assert_ne!(first, changed_vendor);
}

#[test]
fn ignored_nested_node_modules_are_not_discovered() {
    let f = Fixture::new();
    write(
        &f.root.path().join(".gitignore"),
        "node_modules/\nignored/\n",
    );
    let nested = f.root.path().join("nested");
    write(&nested.join("package.json"), r#"{"name":"nested"}"#);
    write(&nested.join("index.js"), "export const n = 1;");
    fs::create_dir_all(f.widget.join("ignored/node_modules")).unwrap();
    symlink(&nested, f.widget.join("ignored/node_modules/nested")).unwrap();
    let first = f.token("symlink");
    assert_eq!(first, "9d34c7b6d4efcacd");
    write(&nested.join("index.js"), "export const n = 2;");
    assert_eq!(first, f.token("symlink"));
    fs::create_dir_all(f.widget.join("visible/node_modules")).unwrap();
    symlink(&nested, f.widget.join("visible/node_modules/nested")).unwrap();
    let second = f.token("symlink");
    assert_eq!(second, "ece7986fd9686bf1");
    write(&nested.join("index.js"), "export const n = 3;");
    let changed_nested = f.token("symlink");
    assert_eq!(changed_nested, "6391d61134c50a83");
    assert_ne!(second, changed_nested);
}
