//! Windows regression for an out-of-root `link:` package (#4545, root cause #4544).
//!
//! `room\site` depends on `@acme/linked` through a junction that leaves the
//! project (`site\node_modules\@acme\linked` -> `room\packages-ws\linked`), the
//! linked package reaches its transitive dependency through a pnpm-isolated
//! junction, and tsconfig `paths` sends the build down the live-link route
//! (`<shadow>\node_modules` is a junction to `site\node_modules`). esbuild then
//! walks two reparse hops. Each case pairs a link creator (libuv junction, as
//! pnpm's symlink-dir makes it, or `directory_link::create`) with a short or
//! long target spelling, and runs two layers:
//!
//! - L1: raw esbuild in a fake shadow, without and with `--preserve-symlinks`.
//! - L2: zfb `bundle()`, two `bundle_with_session` ticks and a `bundle.exclude`
//!   transition on one session.
//!
//! Every layer runs before the case fails, so one Windows run shows which hop
//! breaks. Missing esbuild, node or 8.3 names fail the case; they never skip it.
//!
//! pnpm writes a `link:` entry as a RELATIVE symbolic link (`..\..\..\packages-ws\x`),
//! and Windows resolves a relative target against the path as traversed. Reached
//! through the `<shadow>\node_modules` junction it climbs out of the shadow, not
//! the project (direct-v8 run 40195192600). The `relative_symlink` case pins that:
//! its L1 rows only record what raw esbuild does through a plain junction, and
//! its L2 rows must pass.

use std::{
    collections::BTreeMap,
    ffi::OsString,
    fs,
    os::windows::{
        ffi::{OsStrExt, OsStringExt},
        fs::FileTypeExt,
    },
    path::{Path, PathBuf},
    process::Command,
};

use zfb_types::{directory_link, long_path::long_path_spelling};

use super::{bundle, bundle_with_session, BundleMode, BundlerInput, ShadowSession};

const LINKED_ROOT: &str = "linked-root-marker-4545";
const LINKED_SUB: &str = "linked-sub-marker-4545";
const TRANSITIVE: &str = "transitive-marker-4545";
const LOCAL_ALIAS: &str = "local-alias-marker-4545";

#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetShortPathNameW(long: *const u16, short: *mut u16, cch: u32) -> u32;
}

/// The 8.3 spelling of an existing `path`, or `None` when Win32 fails.
fn short_path_name(path: &Path) -> Option<PathBuf> {
    let input: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut buffer = vec![0u16; 260];
    for _ in 0..2 {
        // SAFETY: `input` is NUL-terminated and `buffer` holds exactly the
        // `cch` units passed; both outlive this synchronous call.
        let len =
            unsafe { GetShortPathNameW(input.as_ptr(), buffer.as_mut_ptr(), buffer.len() as u32) }
                as usize;
        if len == 0 {
            return None;
        }
        if len < buffer.len() {
            return Some(PathBuf::from(OsString::from_wide(&buffer[..len])));
        }
        buffer = vec![0u16; len];
    }
    None
}

fn short_spelling(path: &Path) -> PathBuf {
    short_path_name(path).unwrap_or_else(|| {
        panic!(
            "GetShortPathNameW failed for {}: {}",
            path.display(),
            std::io::Error::last_os_error()
        )
    })
}

#[derive(Clone, Copy, Debug)]
enum LinkCreator {
    /// `fs.symlinkSync(target, link, 'junction')`, the libuv call pnpm's
    /// symlink-dir makes.
    NodeJunction,
    /// `zfb_types::directory_link::create` (canonical `\??\` target).
    DirectoryLink,
    /// `std::os::windows::fs::symlink_dir` with a relative target, the link
    /// pnpm writes for `link:` on Windows (reparse tag 0xA000000C).
    RelativeSymlink,
}

#[derive(Clone, Copy, Debug)]
enum Spelling {
    Short,
    Long,
}

#[derive(Clone, Copy, Debug)]
struct Case {
    creator: LinkCreator,
    target: Spelling,
}

struct Fixture {
    packages_ws: PathBuf,
    linked: PathBuf,
    /// The project root handed to zfb: `long_path_spelling(short site path)`.
    site: PathBuf,
    /// `site\node_modules\@acme\linked`, the out-of-root link under test.
    linked_link: PathBuf,
    /// `linked\node_modules\acme-transitive`, the pnpm-isolated link.
    transitive_link: PathBuf,
}

fn preconditions() -> PathBuf {
    let esbuild =
        zfb_test_utils::locate_esbuild().expect("windows-lib-tests stages ZFB_ESBUILD_BIN");
    let node = Command::new("node")
        .arg("--version")
        .output()
        .unwrap_or_else(|e| panic!("node must be on PATH (setup-node runs in this job): {e}"));
    assert!(
        node.status.success(),
        "`node --version` failed ({}): {}",
        node.status,
        String::from_utf8_lossy(&node.stderr)
    );
    esbuild
}

fn assert_short_names_available(room: &Path) {
    let short = short_spelling(room);
    let long = long_path_spelling(room);
    assert_ne!(
        short,
        long,
        "8.3 short names are required on the runner volume: GetShortPathNameW and \
         long_path_spelling agree on {}, so the short/long spelling cases cannot run",
        room.display()
    );
    assert_eq!(
        fs::canonicalize(&short).unwrap(),
        fs::canonicalize(&long).unwrap(),
        "short {} and long {} must name the same directory",
        short.display(),
        long.display()
    );
}

fn write(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn spell(path: &Path, spelling: Spelling) -> PathBuf {
    match spelling {
        Spelling::Short => {
            let short = short_spelling(path);
            assert_ne!(
                short,
                long_path_spelling(path),
                "8.3 short names are required on the runner volume: {} has no short alias",
                path.display()
            );
            short
        }
        Spelling::Long => long_path_spelling(path),
    }
}

/// `to` relative to the directory `from`; both are absolute and share a prefix.
fn relative_path(from: &Path, to: &Path) -> PathBuf {
    let from = from.components().collect::<Vec<_>>();
    let to = to.components().collect::<Vec<_>>();
    let common = from.iter().zip(&to).take_while(|(a, b)| a == b).count();
    assert!(common > 0, "no common prefix between {from:?} and {to:?}");
    let mut relative = PathBuf::new();
    for _ in common..from.len() {
        relative.push("..");
    }
    for component in &to[common..] {
        relative.push(component.as_os_str());
    }
    relative
}

fn create_link(creator: LinkCreator, target: &Path, link: &Path) {
    match creator {
        LinkCreator::NodeJunction => {
            let output = Command::new("node")
                .arg("-e")
                .arg("require('fs').symlinkSync(process.argv[1], process.argv[2], 'junction')")
                .arg(target)
                .arg(link)
                .output()
                .expect("spawn node");
            assert!(
                output.status.success(),
                "node junction {} -> {} failed ({}): {}",
                link.display(),
                target.display(),
                output.status,
                String::from_utf8_lossy(&output.stderr)
            );
        }
        LinkCreator::DirectoryLink => directory_link::create(target, link).unwrap_or_else(|e| {
            panic!(
                "directory_link::create {} -> {}: {e}",
                link.display(),
                target.display()
            )
        }),
        LinkCreator::RelativeSymlink => {
            let relative = relative_path(link.parent().unwrap(), target);
            std::os::windows::fs::symlink_dir(&relative, link).unwrap_or_else(|e| {
                panic!(
                    "symlink_dir {} -> {} failed (symlink creation is required, never skipped): {e}",
                    link.display(),
                    relative.display()
                )
            });
            let stored = fs::read_link(link).unwrap();
            assert!(
                stored.is_relative(),
                "{} must store a relative target, got {}",
                link.display(),
                stored.display()
            );
        }
    }
    let metadata = fs::symlink_metadata(link).unwrap();
    assert!(
        metadata.file_type().is_symlink(),
        "{} must be a link after {creator:?}: {}",
        link.display(),
        describe_hop(link)
    );
}

/// `room\site`, `room\packages-ws\linked` and the transitive dependency in
/// pnpm's isolated layout, mirroring `packed-consumer.py` Stage 7. Both
/// pnpm-made links use the case's creator and target spelling.
fn write_fixture(room: &Path, case: Case) -> Fixture {
    let room = long_path_spelling(room);
    let packages_ws = room.join("packages-ws");

    let store =
        packages_ws.join("node_modules/.pnpm/acme-transitive@1.0.0/node_modules/acme-transitive");
    write(
        &store.join("package.json"),
        r#"{"name":"acme-transitive","version":"1.0.0","type":"module","main":"./index.js","exports":{".":"./index.js"}}"#,
    );
    write(
        &store.join("index.js"),
        &format!("export const TRANSITIVE = [\"{TRANSITIVE}\"];\n"),
    );

    let linked = packages_ws.join("linked");
    write(
        &linked.join("package.json"),
        r#"{"name":"@acme/linked","version":"1.0.0","type":"module","main":"./index.js","exports":{".":"./index.js","./sub":"./sub/index.js"},"dependencies":{"acme-transitive":"1.0.0"}}"#,
    );
    write(
        &linked.join("index.js"),
        &format!(
            "import {{ TRANSITIVE }} from \"acme-transitive\";\n\
             export const LINKED_ROOT = [\"{LINKED_ROOT}\", TRANSITIVE];\n"
        ),
    );
    write(
        &linked.join("sub/index.js"),
        &format!("export const LINKED_SUB = [\"{LINKED_SUB}\"];\n"),
    );
    write(&linked.join("sentinel.txt"), "linked sentinel #4545\n");
    fs::create_dir_all(linked.join("node_modules")).unwrap();
    let transitive_link = linked.join("node_modules/acme-transitive");
    create_link(case.creator, &spell(&store, case.target), &transitive_link);

    let site_dir = room.join("site");
    for dir in ["pages", "content", "components", "layouts"] {
        fs::create_dir_all(site_dir.join(dir)).unwrap();
    }
    write(
        &site_dir.join("package.json"),
        r#"{"name":"site","private":true,"type":"module","dependencies":{"@acme/linked":"link:../packages-ws/linked"}}"#,
    );
    write(
        &site_dir.join("tsconfig.json"),
        r#"{"compilerOptions":{"paths":{"~/*":["./*"]}}}"#,
    );
    write(
        &site_dir.join("components/local.ts"),
        &format!("export const LOCAL_ALIAS = [\"{LOCAL_ALIAS}\"];\n"),
    );
    write(
        &site_dir.join("pages/index.tsx"),
        r#"import { LINKED_ROOT } from "@acme/linked";
import { LINKED_SUB } from "@acme/linked/sub";
import { LOCAL_ALIAS } from "~/components/local";
export default function Home() {
  return <main>{LINKED_ROOT.join(" ")}{LINKED_SUB.join(" ")}{LOCAL_ALIAS.join(" ")}</main>;
}
"#,
    );
    write(
        &site_dir.join("node_modules/.zfb-4545-sentinel"),
        "node_modules sentinel #4545\n",
    );
    fs::create_dir_all(site_dir.join("node_modules/@acme")).unwrap();
    let linked_link = site_dir.join("node_modules/@acme/linked");
    create_link(case.creator, &spell(&linked, case.target), &linked_link);

    let site_short = short_spelling(&site_dir);
    let site = long_path_spelling(&site_short);
    assert_ne!(
        site, site_short,
        "the project root must be the long expansion of the short site spelling"
    );

    Fixture {
        linked_link: site.join("node_modules/@acme/linked"),
        transitive_link,
        packages_ws,
        linked,
        site,
    }
}

/// File type (with the Windows reparse tag), `read_link`, `canonicalize` and
/// `fsutil reparsepoint query` for one link hop.
fn describe_hop(path: &Path) -> String {
    let file_type = match fs::symlink_metadata(path) {
        Ok(metadata) => {
            let kind = metadata.file_type();
            format!(
                "{kind:?} is_symlink={} is_symlink_dir={} is_dir={}",
                kind.is_symlink(),
                kind.is_symlink_dir(),
                kind.is_dir()
            )
        }
        Err(e) => format!("symlink_metadata error: {e}"),
    };
    let fsutil = Command::new("fsutil")
        .args(["reparsepoint", "query"])
        .arg(path)
        .output()
        .map(|o| {
            format!(
                "{}{}",
                String::from_utf8_lossy(&o.stdout),
                String::from_utf8_lossy(&o.stderr)
            )
        })
        .unwrap_or_else(|e| format!("fsutil spawn error: {e}"));
    format!(
        "  hop {}\n    file_type: {file_type}\n    read_link: {:?}\n    canonicalize: {:?}\n    fsutil reparsepoint query:\n{}",
        path.display(),
        fs::read_link(path),
        fs::canonicalize(path),
        fsutil
            .lines()
            .map(|line| format!("      {line}"))
            .collect::<Vec<_>>()
            .join("\n")
    )
}

fn describe_fixture_hops(fixture: &Fixture) -> String {
    [&fixture.linked_link, &fixture.transitive_link]
        .iter()
        .map(|hop| describe_hop(hop))
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Debug, PartialEq, Eq)]
enum Entry {
    Dir,
    File(Vec<u8>),
    Link(String),
}

/// Every entry under `root` without following links.
fn snapshot(root: &Path) -> BTreeMap<PathBuf, Entry> {
    let mut entries = BTreeMap::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for child in fs::read_dir(&dir).unwrap() {
            let path = child.unwrap().path();
            let kind = fs::symlink_metadata(&path).unwrap().file_type();
            let relative = path.strip_prefix(root).unwrap().to_path_buf();
            if kind.is_symlink() {
                entries.insert(relative, Entry::Link(format!("{:?}", fs::read_link(&path))));
            } else if kind.is_dir() {
                entries.insert(relative, Entry::Dir);
                stack.push(path);
            } else {
                entries.insert(relative, Entry::File(fs::read(&path).unwrap()));
            }
        }
    }
    entries
}

/// Every entry under `root` with link targets, for diagnosing a shadow tree.
fn dump_tree(root: &Path) -> String {
    let mut lines = vec![format!("  tree {}", root.display())];
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(children) = fs::read_dir(&dir) else {
            lines.push(format!("    {} (unreadable)", dir.display()));
            continue;
        };
        for child in children.flatten() {
            let path = child.path();
            let relative = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .display()
                .to_string();
            match fs::symlink_metadata(&path).map(|m| m.file_type()) {
                Ok(kind) if kind.is_symlink() => lines.push(format!(
                    "    {relative} -> {:?} ({kind:?})",
                    fs::read_link(&path)
                )),
                Ok(kind) if kind.is_dir() => {
                    lines.push(format!("    {relative}\\"));
                    stack.push(path);
                }
                Ok(_) => lines.push(format!("    {relative}")),
                Err(e) => lines.push(format!("    {relative} (metadata error: {e})")),
            }
        }
    }
    lines.join("\n")
}

fn missing_markers(body: &str, markers: &[&str]) -> Vec<String> {
    markers
        .iter()
        .filter(|marker| !body.contains(**marker))
        .map(|marker| (*marker).to_string())
        .collect()
}

/// L1: esbuild alone, in a fake shadow whose `node_modules` is a
/// `directory_link::create` junction to `site\node_modules`.
fn raw_esbuild(esbuild: &Path, fixture: &Fixture, preserve_symlinks: bool) -> Result<(), String> {
    let shadow_dir = tempfile::tempdir().unwrap();
    let shadow = long_path_spelling(shadow_dir.path());
    assert!(
        !shadow.starts_with(fixture.site.parent().unwrap()),
        "the fake shadow {} must lie outside the room",
        shadow.display()
    );
    let shadow_nm = shadow.join("node_modules");
    directory_link::create(&fixture.site.join("node_modules"), &shadow_nm).unwrap();
    write(
        &shadow.join("entry.mjs"),
        "import { LINKED_ROOT } from \"@acme/linked\";\n\
         import { LINKED_SUB } from \"@acme/linked/sub\";\n\
         console.log(LINKED_ROOT, LINKED_SUB);\n",
    );
    let out_dir = tempfile::tempdir().unwrap();
    let outfile = long_path_spelling(out_dir.path()).join("out.mjs");

    let mut command = Command::new(esbuild);
    command.current_dir(&shadow).args([
        "--bundle",
        "--platform=neutral",
        "--format=esm",
        "--log-level=verbose",
    ]);
    if preserve_symlinks {
        command.arg("--preserve-symlinks");
    }
    let mut outfile_arg = OsString::from("--outfile=");
    outfile_arg.push(&outfile);
    command.arg(outfile_arg).arg("entry.mjs");
    let output = command.output().expect("spawn esbuild");

    let body = fs::read_to_string(&outfile).unwrap_or_default();
    let missing = missing_markers(&body, &[LINKED_ROOT, LINKED_SUB, TRANSITIVE]);
    if output.status.success() && missing.is_empty() {
        return Ok(());
    }
    Err(format!(
        "esbuild exit {} (cwd {}, preserve_symlinks={preserve_symlinks}); missing markers {missing:?}\n\
         hops:\n{}\n{}\n\
         --- esbuild stdout ---\n{}\n--- esbuild stderr (verbose) ---\n{}",
        output.status,
        shadow.display(),
        describe_hop(&shadow_nm),
        describe_fixture_hops(fixture),
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    ))
}

fn zfb_input(fixture: &Fixture, esbuild: &Path, mode: BundleMode) -> BundlerInput {
    let site = &fixture.site;
    let mut input = BundlerInput::for_project(site.clone(), mode, site.join("dist"), None);
    input.zudo_react_island_names = Some(vec![]);
    input.external = vec![
        "@takazudo/zfb/zudo-react".into(),
        "@takazudo/zfb-runtime".into(),
    ];
    input.esbuild_binary = Some(esbuild.to_path_buf());
    input.mock_subprocess_output = None;
    input.node_modules_dir = Some(site.join("node_modules"));
    input.tsconfig_paths = BTreeMap::from([(
        "~/*".to_string(),
        vec![site.join("*").to_string_lossy().into_owned()],
    )]);
    input
}

struct Integrity {
    packages_ws: BTreeMap<PathBuf, Entry>,
    site_node_modules: BTreeMap<PathBuf, Entry>,
}

impl Integrity {
    fn capture(fixture: &Fixture) -> Self {
        Self {
            packages_ws: snapshot(&fixture.packages_ws),
            site_node_modules: snapshot(&fixture.site.join("node_modules")),
        }
    }

    fn check(&self, fixture: &Fixture) -> Vec<String> {
        let mut problems = Vec::new();
        if snapshot(&fixture.packages_ws) != self.packages_ws {
            problems.push(format!(
                "{} changed (files, bytes or listing)",
                fixture.packages_ws.display()
            ));
        }
        if snapshot(&fixture.site.join("node_modules")) != self.site_node_modules {
            problems.push(format!(
                "{} changed (files, bytes or listing)",
                fixture.site.join("node_modules").display()
            ));
        }
        match fs::symlink_metadata(&fixture.linked_link) {
            Ok(metadata) if metadata.file_type().is_symlink() => {}
            other => problems.push(format!(
                "{} is no longer a link: {other:?}",
                fixture.linked_link.display()
            )),
        }
        if !fixture.linked.join("sentinel.txt").is_file() {
            problems.push("the linked package's sentinel.txt is gone".into());
        }
        problems
    }
}

fn check_run(
    when: &str,
    result: anyhow::Result<super::BundlerOutput>,
    fixture: &Fixture,
    integrity: &Integrity,
    shadow_root: Option<&Path>,
) -> Option<String> {
    let mut problems = Vec::new();
    match result {
        Ok(out) => {
            let body = fs::read_to_string(&out.bundle_path).unwrap_or_default();
            let missing =
                missing_markers(&body, &[LINKED_ROOT, LINKED_SUB, TRANSITIVE, LOCAL_ALIAS]);
            if !missing.is_empty() {
                problems.push(format!(
                    "bundle {} lacks markers {missing:?}",
                    out.bundle_path.display()
                ));
            }
        }
        Err(e) => problems.push(format!("bundle failed: {e:#}")),
    }
    problems.extend(integrity.check(fixture));
    if problems.is_empty() {
        return None;
    }
    let shadow = shadow_root.map(dump_tree).unwrap_or_default();
    Some(format!(
        "{when}:\n  {}\nhops:\n{}\n{shadow}",
        problems.join("\n  "),
        describe_fixture_hops(fixture)
    ))
}

/// L2: zfb's own bundler on the long project spelling.
fn zfb_layer(esbuild: &Path, fixture: &Fixture) -> Result<(), String> {
    let integrity = Integrity::capture(fixture);
    let mut failures = Vec::new();

    let result = bundle(zfb_input(fixture, esbuild, BundleMode::Production));
    failures.extend(check_run(
        "sessionless bundle()",
        result,
        fixture,
        &integrity,
        None,
    ));

    let mut session = ShadowSession::new(&fixture.site)
        .map_err(|e| format!("ShadowSession::new failed: {e:#}"))?;
    let ticks: [(&str, Vec<String>); 4] = [
        ("session tick 1", Vec::new()),
        ("session tick 2 (unchanged source)", Vec::new()),
        (
            "session tick 3 (bundle.exclude = **/*.secret)",
            vec!["**/*.secret".into()],
        ),
        ("session tick 4 (bundle.exclude back to empty)", Vec::new()),
    ];
    for (when, exclude) in ticks {
        let mut input = zfb_input(fixture, esbuild, BundleMode::Development);
        input.bundle_exclude = exclude;
        let result = bundle_with_session(input, Some(&mut session));
        failures.extend(check_run(
            when,
            result,
            fixture,
            &integrity,
            Some(session.shadow_root()),
        ));
    }

    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("\n\n"))
    }
}

fn run_case(case: Case) {
    let esbuild = preconditions();
    let room = tempfile::tempdir().unwrap();
    assert_short_names_available(room.path());
    let fixture = write_fixture(room.path(), case);

    // A relative symlink reached through a plain junction is broken by Windows
    // itself, so its raw-esbuild rows are recorded, not required.
    let l1_required = !matches!(case.creator, LinkCreator::RelativeSymlink);
    let results = [
        (
            "L1 raw esbuild",
            l1_required,
            raw_esbuild(&esbuild, &fixture, false),
        ),
        (
            "L1 raw esbuild --preserve-symlinks",
            l1_required,
            raw_esbuild(&esbuild, &fixture, true),
        ),
        ("L2 zfb", true, zfb_layer(&esbuild, &fixture)),
    ];
    let summary = results
        .iter()
        .map(|(layer, required, result)| {
            let verdict = if result.is_ok() { "PASS" } else { "FAIL" };
            let note = if *required { "" } else { " (recorded)" };
            format!("{layer}={verdict}{note}")
        })
        .collect::<Vec<_>>()
        .join(" ");
    eprintln!(
        "[#4545] {case:?} project={} {summary}",
        fixture.site.display()
    );
    for (layer, _, result) in results.iter().filter(|(_, required, _)| !required) {
        if let Err(e) = result {
            eprintln!("[#4545] {case:?} recorded {layer} failure:\n{e}");
        }
    }
    let details = results
        .iter()
        .filter(|(_, required, _)| *required)
        .filter_map(|(layer, _, result)| {
            result
                .as_ref()
                .err()
                .map(|e| format!("=== {layer} ===\n{e}"))
        })
        .collect::<Vec<_>>();
    if !details.is_empty() {
        panic!(
            "[#4545] {case:?} project={}: {summary}\n\n{}",
            fixture.site.display(),
            details.join("\n\n")
        );
    }
}

#[test]
fn node_junction_short_target() {
    run_case(Case {
        creator: LinkCreator::NodeJunction,
        target: Spelling::Short,
    });
}

#[test]
fn node_junction_long_target() {
    run_case(Case {
        creator: LinkCreator::NodeJunction,
        target: Spelling::Long,
    });
}

#[test]
fn directory_link_short_target() {
    run_case(Case {
        creator: LinkCreator::DirectoryLink,
        target: Spelling::Short,
    });
}

#[test]
fn directory_link_long_target() {
    run_case(Case {
        creator: LinkCreator::DirectoryLink,
        target: Spelling::Long,
    });
}

#[test]
fn relative_symlink() {
    run_case(Case {
        creator: LinkCreator::RelativeSymlink,
        target: Spelling::Long,
    });
}
