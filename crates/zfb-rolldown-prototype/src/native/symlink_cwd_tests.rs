//! A project reached through a symlinked directory (macOS `$TMPDIR` sits under
//! the `/var` -> `/private/var` link) bundles exactly as through its physical
//! path, while every user-visible path keeps the caller's spelling (#4479).
//! Rolldown spells module ids relative to its cwd inside the hashed chunk
//! bytes, so a logical engine cwd put the random temp dir name into chunk names.
use super::{prepared::Request, run_prepared};
use serde_json::Value;
use std::{
    fs,
    os::unix::fs::symlink,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicUsize, Ordering},
};

struct Project {
    root: PathBuf,
    real: PathBuf,
    link: PathBuf,
}
impl Project {
    fn new(role: &str, files: &[(&str, &str)]) -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let root = std::env::temp_dir().join(format!(
            "zfb-native-symlink-{role}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir(&root).unwrap();
        // Only the fixture's own parent is canonicalized; the subject is `link`.
        let root = fs::canonicalize(root).unwrap();
        let real = root.join("real");
        let link = root.join("link");
        fs::create_dir(&real).unwrap();
        symlink(&real, &link).unwrap();
        for (path, source) in files {
            fs::write(real.join(path), source).unwrap();
        }
        Self { root, real, link }
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const SPLIT: &[(&str, &str)] = &[
    (
        "entry.js",
        "export const load = () => import(\"./heavy.js\").then((m) => m.HEAVY);\n",
    ),
    ("heavy.js", "export const HEAVY = \"zfb_symlink_heavy\";\n"),
];

struct Built {
    /// Emitted JS file name -> bytes.
    js: Vec<(String, String)>,
    meta: Value,
}

fn split_args<'a>(outdir: &'a str, extra: &[&'a str]) -> Vec<&'a str> {
    let mut args = vec![
        "entry.js",
        "--bundle",
        "--format=esm",
        "--platform=browser",
        "--splitting=true",
        outdir,
        "--entry-names=islands",
        "--chunk-names=islands-chunk-[hash]",
        "--metafile=out/meta.json",
    ];
    args.extend_from_slice(extra);
    args
}

fn build(cwd: &Path, args: &[&str]) -> anyhow::Result<Built> {
    let _ = fs::remove_dir_all(cwd.join("out"));
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(cwd).args(args);
    run_prepared(&cmd)?;
    let mut js: Vec<_> = fs::read_dir(cwd.join("out"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "js"))
        .map(|path| {
            let name = path.file_name().unwrap().to_string_lossy().into_owned();
            (name, fs::read_to_string(&path).unwrap())
        })
        .collect();
    js.sort();
    let meta = serde_json::from_slice(&fs::read(cwd.join("out/meta.json")).unwrap()).unwrap();
    Ok(Built { js, meta })
}

fn keys(value: &Value) -> Vec<String> {
    value.as_object().unwrap().keys().cloned().collect()
}

fn chunk_names(built: &Built) -> Vec<&str> {
    built
        .js
        .iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| name.starts_with("islands-chunk-"))
        .collect()
}

/// The metafile names outputs under `outdir` exactly as spelled, and keys
/// inputs and the entry point relative to the working directory.
fn assert_caller_spelling(built: &Built, outdir: &Path) {
    let outputs = keys(&built.meta["outputs"]);
    assert!(!outputs.is_empty());
    for key in &outputs {
        assert!(
            Path::new(key).starts_with(outdir),
            "output {key} is not under {}",
            outdir.display()
        );
    }
    let entry = outdir.join("islands.js").to_string_lossy().into_owned();
    assert_eq!(
        built.meta["outputs"][&entry]["entryPoint"], "entry.js",
        "{:#}",
        built.meta
    );
    assert_eq!(
        keys(&built.meta["inputs"]),
        ["entry.js", "heavy.js"],
        "{:#}",
        built.meta
    );
    for output in built.meta["outputs"].as_object().unwrap().values() {
        for input in keys(&output["inputs"]) {
            assert!(!input.starts_with('/') && !input.contains(".."), "{input}");
        }
    }
}

#[test]
fn a_symlinked_cwd_emits_the_chunks_its_physical_path_does() {
    for extra in [&[][..], &["--preserve-symlinks"][..]] {
        let project = Project::new("split", SPLIT);
        let args = split_args("--outdir=out", extra);
        let via_link = build(&project.link, &args).unwrap();
        let via_real = build(&project.real, &args).unwrap();
        assert_eq!(
            chunk_names(&via_link).len(),
            1,
            "{extra:?}: {:?}",
            via_link.js
        );
        assert_eq!(via_link.js, via_real.js, "{extra:?}");
        for (_, source) in &via_link.js {
            assert!(!source.contains("real/"), "{extra:?}: {source}");
        }
        assert_caller_spelling(&via_link, &project.link.join("out"));
        assert_caller_spelling(&via_real, &project.real.join("out"));
        assert!(project
            .link
            .join("out")
            .join(chunk_names(&via_link)[0])
            .is_file());
    }
}

#[test]
fn relative_and_absolute_output_arguments_keep_the_callers_spelling() {
    let project = Project::new("outdir", SPLIT);
    let outdir = project.link.join("out");
    let absolute = format!("--outdir={}", outdir.display());
    let relative = build(&project.link, &split_args("--outdir=out", &[])).unwrap();
    let explicit = build(&project.link, &split_args(&absolute, &[])).unwrap();
    assert_eq!(relative.js, explicit.js);
    assert_caller_spelling(&relative, &outdir);
    assert_caller_spelling(&explicit, &outdir);

    for outfile in [
        "--outfile=out/bundle.mjs".to_string(),
        format!("--outfile={}", outdir.join("bundle.mjs").display()),
    ] {
        let mut cmd = Command::new("unused-prepared-esbuild");
        cmd.current_dir(&project.link).args([
            "heavy.js",
            "--bundle",
            "--format=esm",
            &outfile,
            "--metafile=out/meta.json",
        ]);
        run_prepared(&cmd).unwrap();
        let meta: Value =
            serde_json::from_slice(&fs::read(outdir.join("meta.json")).unwrap()).unwrap();
        let bundle = outdir.join("bundle.mjs").to_string_lossy().into_owned();
        assert_eq!(
            keys(&meta["outputs"]),
            std::slice::from_ref(&bundle),
            "{outfile}"
        );
        assert_eq!(meta["outputs"][&bundle]["entryPoint"], "heavy.js");
        assert_eq!(keys(&meta["inputs"]), ["heavy.js"], "{outfile}");
    }
}

#[test]
fn the_engine_gets_the_physical_cwd_and_the_caller_keeps_its_own() {
    let project = Project::new("decode", SPLIT);
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(&project.link).args([
        "entry.js",
        "--bundle",
        "--outdir=out",
        "--metafile=out/meta.json",
    ]);
    let request = Request::decode(&cmd).unwrap();
    assert_eq!(request.cwd, project.link);
    assert_eq!(request.physical_cwd, project.real);
    assert_eq!(request.outdir, Some(project.link.join("out")));
    assert_eq!(request.metafile, Some(project.link.join("out/meta.json")));
    let options = &request.options;
    assert_eq!(options.cwd.as_deref(), Some(project.real.as_path()));
    let physical = |rel: &str| project.real.join(rel).to_string_lossy().into_owned();
    assert_eq!(options.dir.as_deref(), Some(physical("out").as_str()));
    assert_eq!(
        options.input.as_ref().unwrap()[0].import,
        physical("entry.js")
    );
}

/// Only the cwd prefix is made physical: a symlinked directory inside the
/// project (how shadow staging lays out a subtree) keeps its spelling, so the
/// entry's relative imports resolve beside the staged files, not the originals.
#[test]
fn a_symlinked_subdirectory_inside_the_cwd_keeps_its_spelling() {
    let project = Project::new("nested", &[]);
    let staged = project.root.join("staged-src");
    fs::create_dir(&staged).unwrap();
    fs::write(staged.join("entry.js"), "export const A = 1;\n").unwrap();
    symlink(&staged, project.real.join("src")).unwrap();
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(&project.link)
        .args(["src/entry.js", "--bundle", "--outdir=out"]);
    let request = Request::decode(&cmd).unwrap();
    assert_eq!(
        request.options.input.as_ref().unwrap()[0].import,
        project.real.join("src/entry.js").to_string_lossy()
    );
}

/// #4429 reads a warning's importer against the process cwd, which here is
/// the crate directory, not the symlinked project.
#[test]
fn a_handled_import_through_a_symlinked_cwd_stays_external() {
    let process_cwd = std::env::current_dir().unwrap();
    let handled = Project::new(
        "handled",
        &[(
            "entry.js",
            "export const load = () => import(\"not-installed-optional-peer\").catch(() => null);\n",
        )],
    );
    assert!(!process_cwd.starts_with(&handled.link) && !process_cwd.starts_with(&handled.real));
    let args = [
        "entry.js",
        "--bundle",
        "--format=esm",
        "--platform=neutral",
        "--outfile=out/bundle.mjs",
        "--metafile=out/meta.json",
    ];
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(&handled.link).args(args);
    run_prepared(&cmd).unwrap();
    let output = fs::read_to_string(handled.link.join("out/bundle.mjs")).unwrap();
    assert!(
        output.contains("import(\"not-installed-optional-peer\")"),
        "{output}"
    );

    let unhandled = Project::new(
        "unhandled",
        &[(
            "entry.js",
            "export const load = () => import(\"not-installed-optional-peer\");\n",
        )],
    );
    let mut cmd = Command::new("unused-prepared-esbuild");
    cmd.current_dir(&unhandled.link).args(args);
    let error = format!("{:#}", run_prepared(&cmd).unwrap_err());
    assert!(
        error.contains("Could not resolve \"not-installed-optional-peer\"\n\n    entry.js:1:"),
        "{error}"
    );
}
