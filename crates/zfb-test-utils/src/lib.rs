mod cross_binary_lock;
mod html_normalize;
mod module_entry_probe;
mod sse;
mod watcher_handshake;
pub use cross_binary_lock::CrossBinaryE2eLock;
pub use html_normalize::normalize_html;
pub use module_entry_probe::{module_entry_urls, probe_module_entries, ModuleEntryProbe};
pub use sse::{
    assert_frame_has_data, decode_utf8_incremental, next_sse_event_name, next_sse_frame,
    wait_for_subscribers, wait_for_subscribers_polled, SseFrame,
};
pub use watcher_handshake::{watcher_live_handshake, HandshakeOpts, HandshakeResult};

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Locate an esbuild binary suitable for integration tests.
///
/// Resolution order (union of all existing 17+ per-test copies):
///
/// 1. `ZFB_ESBUILD_BIN` env var — if set and points to an existing file,
///    return it immediately. Deliberately **unvalidated**: an explicit
///    operator override is a documented trust boundary, exactly as in
///    `crates/zfb/build.rs` and `crates/zfb-build/src/bundler.rs`.
/// 2. Workspace-local slot:
///    `<workspace_root>/crates/zfb/binaries/esbuild/<binary>`, probed for
///    every candidate workspace root (see
///    [`candidate_workspace_roots`] — runtime cwd walk-up first, then
///    `current_exe` ancestry, with the compile-time
///    `CARGO_MANIFEST_DIR`-derived root only as a fallback).
///    Binary name is `esbuild` on Unix, `esbuild.exe` on Windows.
/// 3. pnpm nested store, per candidate root:
///    `node_modules/.pnpm/*/node_modules/@esbuild/<suffix>/bin/<binary>`
///    where `*` is each directory entry under `.pnpm` (pnpm's content-
///    addressed layout). Mirrors the shape in
///    `crates/zfb/tests/css_modules_components_build.rs:166-176`.
///    Note: the pnpm Windows package puts the binary at the package root
///    rather than under `bin/`; this path uses `bin/` uniformly following
///    the spec — a future Windows fix can adjust the suffix path if needed.
/// 4. Flat pnpm store, per candidate root:
///    `node_modules/.pnpm/node_modules/esbuild/bin/<binary>` — kept for
///    parity with `crates/zfb-build/tests/embedded_v8_snapshot_e2e.rs`.
/// 5. Portable PATH walk — iterates `$PATH` entries via
///    `std::env::split_paths` (platform-aware `:` vs `;` split) and
///    checks `<entry>/<binary>`. Does NOT shell out to `which`, which is
///    absent on Windows.
///
/// # Why every candidate is execution-probed (issue #2178)
///
/// Tiers 2-5 do not merely check `is_file()`: each candidate is spawned
/// once as `<candidate> --version` and is only returned if it *executes*
/// (any exit status counts — this is not a version check). In a fresh git
/// worktree the worktree's own slot is gitignored and absent, so the
/// candidate walk climbs to the *enclosing* main repo root — whose slot can
/// legitimately hold a wrong-architecture binary left by the release flow's
/// local cross-build. Returning it made whichever test spawned it die with
/// "Bad CPU type in executable" (os error 86), which sweeps misread as a
/// product regression. A candidate that fails to execute is skipped, and
/// the lookup continues in the same order — so a pnpm-store host-arch
/// binary is found and the tests RUN. Spawning (rather than sniffing the
/// executable header) also catches missing exec bits and non-binary blobs,
/// and is the same operation every consuming test performs anyway.
///
/// # Why the workspace root is re-derived at runtime (issue #1007)
///
/// The previous implementation derived the root exclusively from
/// compile-time `env!("CARGO_MANIFEST_DIR")`. Cargo deliberately keeps
/// compiled artifacts relocatable: dep-info source paths are stored
/// package-root-relative and the per-unit env vars cargo itself sets
/// (`CARGO_MANIFEST_DIR` among them) are excluded from the env-dep
/// freshness check (`translate_dep_info` in cargo's fingerprint module).
/// With a shared target dir (e.g. a global `build.target-dir`), an rlib
/// compiled from another checkout of this workspace — a since-deleted
/// `worktrees/<topic>/` path — is reused as "fresh", and its baked-in
/// root points at a directory that no longer has (or never had) the slot
/// binary. Every gated test then silently skipped. Runtime re-derivation
/// makes a stale rlib unable to pin an outdated path.
///
/// # Panics
///
/// Panics on two states, both of which are incidents rather than a
/// legitimately-missing esbuild — per issue #1007, a silent skip here is
/// the incident:
///
/// 1. **Harness bug** — the expected platform slot binary
///    (`crates/zfb/binaries/esbuild/<binary>`) exists as a file under a
///    candidate workspace root *and executes*, yet the lookup above still
///    failed to return it.
/// 2. **Stale staged binary** (issue #2178) — every candidate failed, and
///    at least one expected slot path holds a file that could not be
///    executed at all (wrong architecture, missing exec bit, not a
///    binary). The panic names the stale path, its underlying error, and
///    the remediation (`cargo check -p zfb` re-stages the slot).
///
/// When esbuild is genuinely absent (no slot file at all, no pnpm store
/// hit, no PATH hit) the function returns `None` so machines without
/// esbuild skip gracefully.
///
/// Callers should gate with
/// `let Some(esbuild) = locate_esbuild() else { return; }` to skip
/// gracefully, matching the convention in all existing integration tests.
pub fn locate_esbuild() -> Option<PathBuf> {
    // Step 1: ZFB_ESBUILD_BIN env var. Deliberately NOT execution-probed —
    // an explicit operator override is a trust boundary (see the doc above).
    if let Some(p) = std::env::var_os("ZFB_ESBUILD_BIN") {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Some(p);
        }
    }

    let roots = candidate_workspace_roots();
    let path_entries: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|path_var| std::env::split_paths(&path_var).collect())
        .unwrap_or_default();

    finish_esbuild_lookup(&roots, locate_esbuild_from(&roots, &path_entries))
}

/// One candidate that existed as a file but failed the execution probe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectedCandidate {
    /// The candidate path that was skipped.
    pub path: PathBuf,
    /// The underlying spawn error, e.g.
    /// `"Bad CPU type in executable (os error 86)"`. Preserved verbatim —
    /// do NOT diagnose every execution failure as wrong-architecture.
    pub error: String,
}

/// Outcome of a candidate walk: what was selected, and every present-but-
/// unusable candidate that was skipped along the way.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EsbuildLookup {
    /// The first candidate that both existed and executed, if any.
    pub selected: Option<PathBuf>,
    /// Candidates that existed as files but could not be executed, in
    /// probe order. Non-empty alongside a `Some(selected)` means the
    /// lookup skipped a stale binary and kept going.
    pub rejected: Vec<RejectedCandidate>,
}

/// The candidate-walking core of [`locate_esbuild`], with its environment
/// injected rather than read from the process.
///
/// Exposed so the nested-worktree topology (a worktree root inside an
/// enclosing main repo root, the issue #2178 incident shape) is testable
/// without mutating process cwd — `std::env::set_current_dir` is a
/// process-global, and Rust 2024 rightly treats env mutation in a
/// multithreaded test binary as unsafe.
///
/// `roots` are candidate workspace roots in priority order (see
/// [`candidate_workspace_roots`]); `path_entries` are `$PATH` directories.
/// The `ZFB_ESBUILD_BIN` tier is deliberately *not* handled here — it is a
/// trust boundary that skips validation, and it lives in the environment-
/// gathering wrapper.
pub fn locate_esbuild_from(roots: &[PathBuf], path_entries: &[PathBuf]) -> EsbuildLookup {
    let binary = esbuild_binary_name();
    let mut lookup = EsbuildLookup::default();

    // Step 2: workspace-local binary slot, per candidate root.
    for root in roots {
        if consider_candidate(&mut lookup, slot_binary_path(root, binary)) {
            return lookup;
        }
    }

    for root in roots {
        // Step 3: pnpm nested store — node_modules/.pnpm/*/node_modules/@esbuild/<suffix>/bin/<binary>.
        if let Some(suffix) = esbuild_npm_suffix() {
            let pnpm_dir = root.join("node_modules/.pnpm");
            if let Ok(rd) = std::fs::read_dir(&pnpm_dir) {
                for entry in rd.flatten() {
                    let cand = entry
                        .path()
                        .join("node_modules/@esbuild")
                        .join(suffix)
                        .join("bin")
                        .join(binary);
                    if consider_candidate(&mut lookup, cand) {
                        return lookup;
                    }
                }
            }
        }

        // Step 4: flat pnpm store — node_modules/.pnpm/node_modules/esbuild/bin/<binary>.
        let flat_slot = root
            .join("node_modules/.pnpm/node_modules/esbuild/bin")
            .join(binary);
        if consider_candidate(&mut lookup, flat_slot) {
            return lookup;
        }
    }

    // Step 5: portable PATH walk — no `which` shell-out (missing on Windows).
    for dir in path_entries {
        if consider_candidate(&mut lookup, dir.join(binary)) {
            return lookup;
        }
    }

    lookup
}

/// Turn a finished candidate walk into the `Option<PathBuf>` callers gate
/// on, emitting the notice/panic that keeps every non-obvious outcome
/// visible.
///
/// `roots` is re-probed here rather than reusing the walk's own record:
/// the issue #1007 tripwire must be derived *independently* of the walk,
/// so a lookup that wrongly skips a root cannot also hide that it did.
///
/// # Panics
///
/// See [`locate_esbuild`] — this function owns both panic states.
pub fn finish_esbuild_lookup(roots: &[PathBuf], lookup: EsbuildLookup) -> Option<PathBuf> {
    let EsbuildLookup { selected, rejected } = lookup;

    if let Some(selected) = selected {
        // Success via a later candidate is still worth one line: a silently
        // bypassed stale slot is how #2178 stayed confusing for a whole sweep.
        for skipped in &rejected {
            eprintln!(
                "zfb-test-utils: skipped invalid or wrong-architecture staged esbuild at \
                 {path} ({error}) — using {selected} instead; run `cargo check -p zfb` to re-stage",
                path = skipped.path.display(),
                error = skipped.error,
                selected = selected.display(),
            );
        }
        return Some(selected);
    }

    let slot_probes = probe_slot_binaries(roots);
    match classify_slot_probes(&slot_probes) {
        SkipKind::GenuinelyAbsent => {
            eprintln!(
                "zfb-test-utils: esbuild not found (ZFB_ESBUILD_BIN, workspace slot, \
                 pnpm stores, PATH all missed) — gated test skipped"
            );
            None
        }
        SkipKind::InvalidSlots { invalid_slots } => panic!(
            "zfb-test-utils (issue #2178): invalid or wrong-architecture staged esbuild at \
             {invalid} — run `cargo check -p zfb` to re-stage. No usable esbuild was found \
             anywhere else either (pnpm stores, PATH), so gated tests cannot run. Probed slot \
             paths: {probed:?}. This state must never become a silent skip.",
            invalid = describe_rejections(&invalid_slots),
            probed = describe_probes(&slot_probes),
        ),
        SkipKind::HarnessBug { present_slots } => panic!(
            "zfb-test-utils harness bug (issue #1007): the esbuild slot binary exists at \
             {present:?} but locate_esbuild() failed to return it. Checked slot paths: \
             {checked:?}. This state must never become a silent skip — fix the lookup.",
            present = present_slots
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>(),
            checked = slot_probes
                .iter()
                .map(|probe| probe.path.display().to_string())
                .collect::<Vec<_>>(),
        ),
    }
}

/// Probe the expected platform slot path under every candidate root,
/// tri-state. Present-but-unusable is its own answer — collapsing it into
/// either "absent" or "present" is what let #2178 hide.
fn probe_slot_binaries(roots: &[PathBuf]) -> Vec<SlotProbe> {
    let binary = esbuild_binary_name();
    roots
        .iter()
        .map(|root| {
            let path = slot_binary_path(root, binary);
            let state = if !path.is_file() {
                SlotState::Absent
            } else {
                match probe_runnable(&path) {
                    Ok(()) => SlotState::Valid,
                    Err(error) => SlotState::Invalid(error),
                }
            };
            SlotProbe { path, state }
        })
        .collect()
}

/// Probe one candidate: skip it silently when it is not a file, record it
/// in `rejected` when it is a file that cannot be executed, select it when
/// it runs. Returns `true` when the candidate was selected.
fn consider_candidate(lookup: &mut EsbuildLookup, candidate: PathBuf) -> bool {
    if !candidate.is_file() {
        return false;
    }
    match probe_runnable(&candidate) {
        Ok(()) => {
            lookup.selected = Some(candidate);
            true
        }
        Err(error) => {
            lookup.rejected.push(RejectedCandidate {
                path: candidate,
                error,
            });
            false
        }
    }
}

/// Execute `<candidate> --version` and report whether the OS could run it
/// at all.
///
/// Deliberately **not** a version check: any exit status means the file is
/// a runnable binary for this host, which is the whole question. Only a
/// failure to spawn (ENOEXEC, EBADARCH / os error 86, EACCES, …) rejects
/// the candidate, and the error is carried verbatim rather than being
/// re-diagnosed as "wrong architecture".
///
/// Known limitation, accepted rather than over-validated: Apple's
/// `posix_spawnp` implements execvp's historical ENOEXEC fallback, so an
/// executable-bit-set file with no recognized magic is re-run under
/// `/bin/sh` and counts as "spawned". That does not affect the issue #2178
/// incident — a wrong-architecture Mach-O has valid magic and fails with
/// EBADARCH before any fallback — and sniffing headers to close it would
/// re-introduce the format-guessing this probe exists to avoid.
fn probe_runnable(candidate: &Path) -> Result<(), String> {
    Command::new(candidate)
        .arg("--version")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|_| ())
        .map_err(|err| err.to_string())
}

fn describe_rejections(rejections: &[RejectedCandidate]) -> String {
    rejections
        .iter()
        .map(|rejected| format!("{} ({})", rejected.path.display(), rejected.error))
        .collect::<Vec<_>>()
        .join(", ")
}

fn describe_probes(probes: &[SlotProbe]) -> Vec<String> {
    probes
        .iter()
        .map(|probe| match &probe.state {
            SlotState::Absent => format!("{} (absent)", probe.path.display()),
            SlotState::Valid => format!("{} (valid)", probe.path.display()),
            SlotState::Invalid(error) => format!("{} (invalid: {error})", probe.path.display()),
        })
        .collect()
}

/// Candidate workspace roots for the slot/pnpm probes, highest priority
/// first, deduplicated:
///
/// 1. Ancestors of `std::env::current_dir()` that look like the workspace
///    root (`Cargo.toml` file + `crates/` dir). Cargo runs test binaries
///    with the crate's manifest dir as cwd, so this finds the *real*
///    checkout the tests run from — including a `worktrees/<topic>/` root
///    first and the enclosing main repo root after it.
/// 2. Ancestors of `std::env::current_exe()` — covers runners that invoke
///    the test binary from an unrelated cwd, as long as the target dir
///    lives inside the workspace (a shared/global target dir won't match;
///    that's fine, this is an extra probe).
/// 3. The compile-time `CARGO_MANIFEST_DIR`-derived root, kept as a
///    fallback only. This value can be stale when a shared target dir
///    reuses an rlib compiled from another checkout path (issue #1007),
///    which is why it is probed last.
pub fn candidate_workspace_roots() -> Vec<PathBuf> {
    let mut roots: Vec<PathBuf> = Vec::new();

    if let Ok(cwd) = std::env::current_dir() {
        for dir in cwd.ancestors() {
            if is_workspace_root(dir) {
                push_unique(&mut roots, dir.to_path_buf());
            }
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        for dir in exe.ancestors() {
            if is_workspace_root(dir) {
                push_unique(&mut roots, dir.to_path_buf());
            }
        }
    }

    // CARGO_MANIFEST_DIR = <repo>/crates/zfb-test-utils → parent = <repo>/crates → parent = <repo>.
    // Pushed unconditionally (no marker check): if stale it simply fails
    // the file probes, preserving the pre-#1007 fallback behavior.
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Some(root) = manifest_dir.parent().and_then(Path::parent) {
        push_unique(&mut roots, root.to_path_buf());
    }

    roots
}

/// What probing one expected platform slot path found.
///
/// Tri-state on purpose (issue #2178): a `(path, bool)` shape cannot tell
/// "a working binary the lookup failed to return" (the #1007 harness bug)
/// apart from "a file that cannot execute on this host" (a stale staged
/// binary), and the two demand different diagnostics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotState {
    /// Nothing at the slot path. The slot *directory* being non-empty
    /// never counts — it permanently holds `.gitkeep` and `README.md`.
    Absent,
    /// A file that executed when probed.
    Valid,
    /// A file that exists but could not be executed at all; carries the
    /// underlying spawn error verbatim.
    Invalid(String),
}

/// One expected platform slot path (`<root>/crates/zfb/binaries/esbuild/
/// <binary>`) and what probing it found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SlotProbe {
    /// The exact slot path probed.
    pub path: PathBuf,
    /// The probe result.
    pub state: SlotState,
}

/// Classification of a failed esbuild lookup, decided from explicit probe
/// inputs so the panic contract is hermetically testable (issue #1007).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipKind {
    /// No slot binary exists anywhere we know to look — esbuild is
    /// genuinely absent and the caller should skip gracefully.
    GenuinelyAbsent,
    /// At least one expected slot binary path IS a runnable file, yet the
    /// lookup failed to return it — a harness bug that must panic loudly.
    HarnessBug {
        /// The slot paths that exist AND execute despite the failed lookup.
        present_slots: Vec<PathBuf>,
    },
    /// No usable candidate anywhere, and at least one expected slot path
    /// holds a file that cannot be executed on this host (issue #2178).
    /// Actionable and loud: the staged binary is stale, not missing.
    InvalidSlots {
        /// The unusable slot paths with their rejection errors.
        invalid_slots: Vec<RejectedCandidate>,
    },
}

/// Decide how a failed lookup must behave, from the tri-state probe of
/// every expected platform slot path.
///
/// Precedence: a *runnable* slot the lookup failed to return is always the
/// #1007 harness bug, whatever else was seen; otherwise an unusable slot
/// file is the #2178 stale-binary state; otherwise esbuild is genuinely
/// absent and the caller skips gracefully.
pub fn classify_slot_probes(slot_probes: &[SlotProbe]) -> SkipKind {
    let present_slots: Vec<PathBuf> = slot_probes
        .iter()
        .filter(|probe| probe.state == SlotState::Valid)
        .map(|probe| probe.path.clone())
        .collect();
    if !present_slots.is_empty() {
        return SkipKind::HarnessBug { present_slots };
    }

    let invalid_slots: Vec<RejectedCandidate> = slot_probes
        .iter()
        .filter_map(|probe| match &probe.state {
            SlotState::Invalid(error) => Some(RejectedCandidate {
                path: probe.path.clone(),
                error: error.clone(),
            }),
            SlotState::Absent | SlotState::Valid => None,
        })
        .collect();
    if invalid_slots.is_empty() {
        SkipKind::GenuinelyAbsent
    } else {
        SkipKind::InvalidSlots { invalid_slots }
    }
}

/// Boolean-probe adapter over [`classify_slot_probes`], kept for the
/// original issue #1007 contract tests: `true` means the slot path is a
/// file that runs, `false` means nothing is there. It cannot express the
/// #2178 present-but-unusable state — use [`classify_slot_probes`] for new
/// call sites.
pub fn classify_failed_lookup(checked_slots: &[(PathBuf, bool)]) -> SkipKind {
    let slot_probes: Vec<SlotProbe> = checked_slots
        .iter()
        .map(|(path, is_file)| SlotProbe {
            path: path.clone(),
            state: if *is_file {
                SlotState::Valid
            } else {
                SlotState::Absent
            },
        })
        .collect();
    classify_slot_probes(&slot_probes)
}

/// Expected platform slot binary path under `root`.
fn slot_binary_path(root: &Path, binary: &str) -> PathBuf {
    root.join("crates/zfb/binaries/esbuild").join(binary)
}

/// A directory is treated as a workspace root when it has a `Cargo.toml`
/// file and a `crates/` directory — the layout of this workspace.
fn is_workspace_root(dir: &Path) -> bool {
    dir.join("Cargo.toml").is_file() && dir.join("crates").is_dir()
}

fn push_unique(roots: &mut Vec<PathBuf>, root: PathBuf) {
    if !roots.contains(&root) {
        roots.push(root);
    }
}

/// Returns the esbuild binary file name for the current platform.
///
/// `esbuild.exe` on Windows, `esbuild` everywhere else.
fn esbuild_binary_name() -> &'static str {
    if cfg!(windows) {
        "esbuild.exe"
    } else {
        "esbuild"
    }
}

/// Returns the esbuild npm package suffix for the current platform/arch,
/// or `None` for unsupported combinations (causing step 3 to be skipped).
///
/// These suffixes match the `esbuild_platform_meta` table in
/// `crates/zfb/build.rs:238` — keep both in sync if either changes.
fn esbuild_npm_suffix() -> Option<&'static str> {
    if cfg!(target_os = "linux") && cfg!(target_arch = "x86_64") {
        Some("linux-x64")
    } else if cfg!(target_os = "linux") && cfg!(target_arch = "aarch64") {
        Some("linux-arm64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "x86_64") {
        Some("darwin-x64")
    } else if cfg!(target_os = "macos") && cfg!(target_arch = "aarch64") {
        Some("darwin-arm64")
    } else if cfg!(target_os = "windows") && cfg!(target_arch = "x86_64") {
        Some("win32-x64")
    } else {
        None
    }
}

/// Expands to `PathBuf::from(env!("CARGO_BIN_EXE_zfb"))` at the call site.
///
/// `CARGO_BIN_EXE_zfb` is set by Cargo only when compiling integration
/// tests for the crate that owns the `zfb` binary (`crates/zfb/`). The
/// macro form is required so that `env!()` expands at the *caller's*
/// compile unit — a plain function call cannot access a test-binary env
/// var from another crate.
///
/// Usage: place `use zfb_test_utils::zfb_binary;` in your test file, then
/// call `zfb_binary!()` to obtain the path to the compiled `zfb` binary.
#[macro_export]
macro_rules! zfb_binary {
    () => {{
        ::std::path::PathBuf::from(env!("CARGO_BIN_EXE_zfb"))
    }};
}
