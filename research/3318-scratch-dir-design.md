# Scratch dir + per-invocation define: locked design

Design lock for epic #3339 (supersedes #3318), written by sub-task #3340. Sub-tasks #3341–#3348
implement it. Every citation below was checked against the tree at `base/scratch-dir` (`c5027a0b`).
Line numbers drift; the symbol names are the stable anchor.

Reported downstream: zudo-case#62 (a concurrent command overwrites a running dev's input) and
zudo-case#69 (isolate preview-origin defines for dev/check/build).

## Summary of the locked contract

- `--scratch-dir <PATH>` on `dev`, `build`, `check`, **and `preview`**, with env fallback
  `ZFB_SCRATCH_DIR` (flag wins; an empty or whitespace-only env value counts as unset).
- A scratch dir inside the project must sit **strictly under `<root>/.zfb-build/`**. A scratch dir
  outside the project is allowed anywhere that passes the overlap rules.
- Ownership: an exclusive advisory lock on `<scratch>/.zfb-scratch.lock`, held for the process
  lifetime. That file also marks the directory as a scratch root. A non-empty directory without
  it is rejected. zfb never deletes the scratch root and never reaps a scratch dir.
- One `ScratchLayout` type in **`zfb-types`** (`crates/zfb-types/src/scratch_layout.rs`) owns every
  zfb-generated path. The default layout is byte-identical to today.
- A scratch-dir dev never reads `outDir`. That covers the HTML seed leg, the `/assets/*` fallback
  layer, and both boot-lazy `dist_is_servable_seed` decisions. Dev logs one info line about it.
- `check` writes nothing zfb-owned. `--scratch-dir` on check only validates and locks the dir.
  `--define` on check validates and merges; it never reaches `tsc`.
- `--define KEY=EXPR` (repeatable, last occurrence wins) is on `dev`, `build`, and `check`, not on
  `preview`. It is merged once into the loaded `Config`. The effective define map is hashed into
  the owned-runtime build token (both SSR and islands call sites). A CLI override also enters the
  persisted-graph digest.
- Plugins get `scratchDir = <layout root>/plugins`, always present and absolute. It holds opaque
  intermediates only (option (a)), and zfb does not create the directory.

## Decision 1: Flag surface (`--scratch-dir`, `ZFB_SCRATCH_DIR`, `--define`)

**Recommendation (locked).**

- `--scratch-dir <PATH>` is a flattened clap group shared by `DevArgs`, `BuildArgs`, `CheckArgs`,
  and `PreviewArgs` (`crates/zfb/src/cli.rs:218`, `:235`, `:547`, `:509`). Add
  `#[derive(Debug, Args)] pub struct ScratchDirArg { #[arg(long = "scratch-dir", value_name = "PATH")] pub scratch_dir: Option<PathBuf> }`
  and `#[command(flatten)] pub scratch: ScratchDirArg` in each.
- `ZFB_SCRATCH_DIR` is read by hand in the resolver, not through clap's `env` attribute. `clap`
  is built with `features = ["derive"]` only (`crates/zfb/Cargo.toml:54`). The resolver is a pure
  function over an injected env value, e.g.
  `select_scratch_dir(flag: Option<&Path>, env: Option<&OsStr>) -> Option<PathBuf>`. Tests
  therefore never mutate the process environment. The same idiom appears at `parse_keep_flag`
  (`crates/zfb-build/src/bundler.rs:1497-1505`) and in dev's
  `std::env::var(...).ok().as_deref()` decision seams (`crates/zfb/src/commands/dev.rs:1502-1507`).
- Precedence is flag > non-empty env > none. A relative path resolves against the process cwd,
  which is also the project root: every command does `env::current_dir()`, e.g.
  `crates/zfb/src/commands/build.rs:108` and `crates/zfb/src/commands/check.rs:57`. The directory
  is created (`create_dir_all`) only after the pre-creation validation in Decision 3 passes.
- `--define KEY=EXPR` is a flattened clap group
  (`#[arg(long = "define", value_name = "KEY=EXPR", action = ArgAction::Append, value_parser = parse_define_override)]`)
  on `DevArgs`, `BuildArgs`, and `CheckArgs` only. `parse_define_override` returns an error
  (clap exit code 2) in these cases:
  - no `=`, split on the **first** `=`;
  - an empty key;
  - an empty expression;
  - a reserved key.

  The reserved list moves from the function-local const at `crates/zfb/src/config.rs:2974-2978` to a
  module-level `pub(crate) const RESERVED_DEFINE_KEYS`. The config validator and the CLI parser
  then share one list. Beyond that, the key syntax is **not** validated, matching config
  `bundle.define`, where esbuild reports a bad key.
- Repeated keys on the CLI: **the last occurrence wins**, which is esbuild's own `--define`
  semantics. That lets a script append an override to a shared argument list.
- Merge happens once, right after `config::load_from_dir`, into the owned `Config`. It is merged
  per key over `bundle.define`, and the CLI wins. This follows the existing "mutate the owned
  config before any consumer reads it" discipline at `crates/zfb/src/commands/build.rs:117-170`,
  where the strict-* overrides are written back before plugin serialization. Helper:
  `config::apply_define_overrides(cfg: &mut Config, overrides: &[(String, String)])`, which does
  `cfg.bundle.get_or_insert_with(Default::default).define.get_or_insert_with(Default::default)`.
  Every consumer already reads the map through `resolve_bundle_define`
  (`crates/zfb/src/config.rs:1803`):
  - SSR: `crates/zfb/src/commands/bundler_input.rs:535`;
  - islands: `crates/zfb/src/commands/build.rs:4506`;
  - client scripts: `crates/zfb/src/commands/build.rs:5944`;
  - dev client scripts: `crates/zfb/src/commands/build.rs:6586`;
  - module workers: `crates/zfb/src/commands/build.rs:2242`.

  So all of them see the override with no per-pipeline plumbing. Plugins see it too, in
  `ctx.config.bundle.define`, because the same `Config` is serialized into hook contexts
  (`crates/zfb/src/commands/dev.rs:1419-1421`).
- `preview` takes **`--scratch-dir` but not `--define`**. Preview serves already-built output and
  bundles nothing, so a define flag there would be a no-op that looks meaningful.

**Rejected alternatives.**

- clap `env = "ZFB_SCRATCH_DIR"`: it needs a new clap feature, it treats an empty value as set,
  and it pushes precedence tests into process-env mutation.
- No `preview` flag, with preview reporting the default layout. An exported `ZFB_SCRATCH_DIR`
  would then be silently ignored by one command. Plugin preview intermediates would also land in
  the shared default root while dev and build are isolated.
- `--define` on preview: it has no bundling consumer.
- `--config <path>`: config discovery also drives root detection. That surface is much larger
  than the define-only need (epic, "Alternatives rejected").
- Rejecting duplicate CLI keys: it breaks the append-an-override script idiom and differs from
  esbuild.

## Decision 2: Ownership, lifetime, markers

**Recommendation (locked).**

- **Lock file:** `<scratch>/.zfb-scratch.lock`, exposed as the const
  `zfb_types::scratch_layout::SCRATCH_LOCK_FILE_NAME`. Open it with
  `OpenOptions::new().read(true).write(true).create(true).truncate(false)` and take
  `File::try_lock()`, the exclusive advisory lock stable since Rust 1.89. This is the same
  primitive as the shadow-session owner lock (`crates/zfb-build/src/bundler.rs:1490`,
  `:1817-1838`) and the liveness probe (`crates/zfb/src/commands/watcher_liveness_probe.rs:419-433`).
  Keep the `File` alive in the returned lease for the whole command, until `run` returns, and let
  the OS release it on exit.
  - `Err(TryLockError::WouldBlock)` is a **hard error**: "scratch dir `<path>` is in use by another
    zfb process (lock `<path>/.zfb-scratch.lock`); give each concurrent command its own
    `--scratch-dir`".
  - `Err(TryLockError::Error(e))`, a filesystem without advisory locks, is **also a hard error**
    that names `e` and suggests a local-filesystem path. This deliberately differs from the shadow
    session's warn-and-continue (`bundler.rs:1823-1837`). There, a lockless filesystem only
    weakens a reaper. Here, exclusivity *is* the feature the user opted into, and the no-flag
    default takes no lock at all, so nothing regresses.
- **The lock file doubles as the scratch-root marker.** A scratch root is a directory that
  contains `.zfb-scratch.lock`. The file is never deleted, because deleting a lock file is racy.
- **Non-empty unmarked directories are rejected.** After creation and canonicalization, read the
  directory:
  - empty → accept;
  - contains `.zfb-scratch.lock` → accept, as a reused scratch root;
  - anything else → hard error: "refusing to use non-empty directory `<path>` that zfb did not
    create as a scratch dir; pass an empty or new directory".

  This stops a typo such as `--scratch-dir /tmp` or `--scratch-dir ~/Documents` from writing
  `bundle.mjs` / `graph.bin` into a user directory.
- **Do not reuse `.zfb-generated-output-v1`** (`OWNED_OUTPUT_MARKER`, `bundler.rs:2802`) as the
  ownership proof. That marker means "exclude these generated bytes from the owned-runtime source
  token". The bundler writes it into *every* bundle outdir (`bundler.rs:5337-5340`), so an old
  bundle outdir would pass as an owned scratch dir. The scratch root is the bundle outdir
  (Decision 4), so the bundler keeps writing that marker there as it does today. That is
  harmless, and it also keeps the scratch root out of any sibling token walk (`bundler.rs:3113`).
- **Acquisition order** (`crates/zfb/src/commands/scratch_dir.rs`, new):
  1. absolutize against cwd and lexically normalize;
  2. existing-prefix canonicalize;
  3. validate (Decision 3);
  4. `create_dir_all`;
  5. `fs::canonicalize`;
  6. **re-validate on the now-canonical path** (a symlink created between steps 3 and 5 cannot
     slip through);
  7. check emptiness and marker;
  8. `try_lock`.

  If two processes race on the same new directory, one wins the lock. The other then sees the
  lock file (a marked, reused root) and gets `WouldBlock`.
- **No reaping, no recursive delete of the scratch root.** Inside the root, zfb deletes only:
  - files it tracked in this process: dev companion pruning, `prune_dev_companions`
    (`crates/zfb/src/commands/dev.rs:5048-5080`);
  - its own dev-pages `TempDir` session (`dev.rs:10468-10480`);
  - lock-proven-dead `session-*` probe dirs under the zfb-created `watcher-liveness-probe/`
    child (`watcher_liveness_probe.rs:173-229`).

  All three already happen today under `.zfb-build/`.
- Contention test shape: a **second `File` handle** on the same lock file in the same process
  must get `try_lock().is_err()`. `flock` locks are per open-file-description, so this works in
  one process. It mirrors the existing test at `bundler.rs:15578-15595`. No subprocess is needed.

**Rejected alternatives.** Warn-and-continue on a lockless filesystem, because the user-facing
concurrency promise would silently not hold. A pid file with liveness checks is vulnerable to pid
reuse, and the codebase already rejected it (`watcher_liveness_probe.rs:151-160`). A dedicated
second marker file (for example `.zfb-scratch-v1`) adds nothing over the lock file. Accepting any
non-empty directory risks polluting user directories.

## Decision 3: Overlap and placement rules

All comparisons use canonical paths. A path that does not exist yet uses **existing-prefix
canonicalization**: canonicalize the longest existing ancestor, then re-append the missing
components. Hoist the nested helper at `crates/zfb/src/commands/build.rs:3025-3038`
(`canonicalize_existing_prefix`) to `zfb_types::helpers::canonicalize_existing_prefix` and reuse
it. `canonicalize_or_lexical` (`crates/zfb/src/commands/dev.rs:399-402`) is not good enough here:
its all-or-nothing lexical fallback misses an alias through a symlinked ancestor of a
not-yet-created `outDir`. "Overlap" means `a == b || a.starts_with(b) || b.starts_with(a)`
(component-wise `Path::starts_with`).

Each failure is a hard error that names the rule and both paths.

- **R1: not the project root or an ancestor of it.** The scratch dir must not equal or contain the
  canonical project root.
- **R2: in-project placement.** If the canonical scratch dir is inside the canonical project root,
  it must be a **strict descendant of `<root>/.zfb-build/`**. It must also not be `.zfb-build`
  itself, not inside one of the default layout's reserved children, and no path component below
  `.zfb-build/` may be a reserved child name. The reserved names are `dev-pages`, `dev-assets`,
  `watcher-liveness-probe`, and `plugins`, exposed as `zfb_types::scratch_layout::RESERVED_CHILD_NAMES`.
  Out-of-project locations are allowed, subject to R3–R6.
  - Why reject arbitrary in-project directories: five independent walkers would see a non-hidden
    in-project scratch dir, while every one of them already skips `.zfb-build/`.
    - The shadow extra-top-level-dirs mirror (`enumerate_extra_top_level_dirs`,
      `crates/zfb-build/src/bundler.rs:5961-5991`) copies every non-hidden, non-gitignored
      top-level directory into each shadow session. That includes a sibling session's live
      `bundle.mjs` and `dev-pages/*.html`.
    - The owned-runtime token walk (`bundler.rs:3100-3119`) skips only `.zfb-*` names, the
      selected output dir, and marker-carrying dirs.
    - The zudo-wind walk (`crates/zudo-wind/src/walk.rs:8-20`, `:91`) skips hidden dirs and the
      `SKIP` names.
    - The CSS `SourcePlan` excludes only `.zfb-build` and `.zfb` (`crates/zfb/src/commands/css_source_plan.rs:81-87`).
    - The templates' `.gitignore` covers `.zfb-build` (`crates/zfb/templates/basic-blog/.gitignore:3`).

    Requiring `.zfb-build/<name>` makes every walker correct by construction. The rule can be
    relaxed later without breaking anyone; an allowance could not be taken back.
  - Why reserved names: `.zfb-build/{dev-pages,dev-assets,watcher-liveness-probe}` are the
    *default* layout's live directories (`dev.rs:10463-10464`, `:10496-10497`,
    `watcher_liveness_probe.rs:96-101`), and `plugins` is the default plugin scratch dir
    (Decision 9). A scratch root there would collide with a concurrent default-layout `zfb dev`.
    It could also be swept by the probe's `session-*` reaper (`watcher_liveness_probe.rs:173-229`).
- **R3: `outDir`, both directions.** Use the command's effective output directory:
  - build and preview: CLI `--outdir` > config, via `resolve_outdir_arg` + `resolve_outdir` /
    `resolve_under_root` (`crates/zfb/src/commands/resolve.rs:164`, `:31`, `:19`);
  - dev and check: `cfg.out_dir` (`dev.rs:1287`).

  `zfb build` wipes `outDir` (`wipe_outdir_contents`, `resolve.rs:108`), and dev reads it as a
  seed (Decision 6). Either direction of overlap is therefore destructive or leaky.
- **R4: `publicDir`, both directions.** `public/*` is copied into `outDir` by build and served
  by dev. It is usually already excluded by R2, but R4 covers an out-of-project `publicDir`.
- **R5: authored watch roots, both directions.** The set is:
  - `resolve_roots(project_root, cfg).relative_watch_roots` joined to the root
    (`DEFAULT_WATCH_ROOTS` + in-root collections, `dev.rs:145-160`, `:237`, `:477`);
  - the canonical out-of-root collection roots (`ResolvedRoots::out_of_root_watch_roots`,
    `dev.rs:453`);
  - `cfg.extra_watch_paths`, canonicalized, with missing entries skipped silently (the dev
    warning path stays in `resolve_extra_watch_paths`, `dev.rs:4578`).

  Expose this set as `pub(crate) fn authored_watch_roots(project_root, cfg) -> Vec<PathBuf>`
  next to `resolve_roots`. Neither is gated on `embed_v8`, so check and preview can call it.
  Dynamic watches — CSS `@import` auto-watch and the SSR module-dep registry
  (`policy.rs:647`) — are discovered after resolution, so they are not validated. The scratch
  root joins `set_zfb_written_roots` (Decision 4), so their reconcile ignores it. Apply R5 to all
  four commands, so a directory that is valid for `build` is also valid for `dev`.
- **R6: no nesting inside another scratch root, and no zfb-reaped names.**
  - Reject when any **strict ancestor** of the canonical scratch dir contains
    `.zfb-scratch.lock`. This is a cheap walk up to the filesystem root.
  - Reject when any path component starts with `zfb-shadow-session-`, or equals
    `zfb-dev-watcher-liveness-probe` or `watcher-liveness-probe`. Those are the names swept by
    `reap_stale_shadow_sessions` (`bundler.rs:2068-2140`; its lock-file-absent branch
    `remove_dir_all`s at `:2132`) and by the probe sweep (`watcher_liveness_probe.rs:114`,
    `:173-229`). A scratch dir in one of those places could otherwise be deleted by zfb itself.
  - Accepted residual: claiming an *outer* directory around an already-existing scratch root is
    not detected, because that would need a recursive walk. The collision surface is small: only
    the reserved child names can clash. The docs tell users to use sibling directories.

**Rejected alternatives.** Allowing any in-project location: see the five walkers in R2. Warning
instead of rejecting for in-project non-`.zfb-build` dirs: the shadow mirror copy is silent and
racy, so a warning is not enough. Comparing lexical paths only: the macOS `/var` → `/private/var`
aliasing, and the symlinked-`outDir` cases the #3342 tests require.

## Decision 4: Layout and the `ScratchLayout` type

**Crate: `zfb-types`**, new module `crates/zfb-types/src/scratch_layout.rs`, re-exported from
`crates/zfb-types/src/lib.rs`. `zfb-types` is the dependency-free shared-types crate (`serde`,
`sha2`, `hex` only; `crates/zfb-types/Cargo.toml`). `zfb` depends on it directly, and so do
`zfb-build` (`crates/zfb-build/Cargo.toml:46`) and `zfb-server` (`crates/zfb-server/Cargo.toml:36`).
None of those edges needs V8. The type is **pure path arithmetic, no I/O**. Resolution,
validation, and locking live in `crates/zfb/src/commands/scratch_dir.rs` (Decisions 1–3), because
they need `Config`. `zudo-wind` stays independent of `zfb-types`: its `SKIP` list keeps its
default names, and the layout's roots reach it through the existing `SourcePlan.exclusions`
channel (`crates/zudo-wind/src/walk.rs:38-40`, `:162-167`). **No change to `walk.rs` is needed.**

```rust
pub const DEFAULT_SCRATCH_DIR_NAME: &str = ".zfb-build";
pub const DEFAULT_STAGING_DIR_NAME: &str = ".zfb";          // graph.bin (default) + first-party staging JSON
pub const SCRATCH_LOCK_FILE_NAME: &str = ".zfb-scratch.lock";
pub const DEV_PAGES_DIR_NAME: &str = "dev-pages";
pub const DEV_ASSETS_DIR_NAME: &str = "dev-assets";
pub const LIVENESS_PROBE_DIR_NAME: &str = "watcher-liveness-probe";
pub const PLUGIN_SCRATCH_DIR_NAME: &str = "plugins";
pub const GRAPH_BIN_FILE_NAME: &str = "graph.bin";
pub const RUNTIME_BUNDLE_BASENAME: &str = "bundle-runtime.mjs";
pub const RESERVED_CHILD_NAMES: &[&str] = &[DEV_PAGES_DIR_NAME, DEV_ASSETS_DIR_NAME, LIVENESS_PROBE_DIR_NAME, PLUGIN_SCRATCH_DIR_NAME];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScratchLayout { project_root: PathBuf, root: PathBuf, session: bool }

impl ScratchLayout {
    pub fn default_for(project_root: &Path) -> Self;            // root = <project_root>/.zfb-build, session = false
    pub fn for_scratch_dir(project_root: &Path, scratch_root: PathBuf) -> Self; // #3342; session = true; caller passes the canonical root
    pub fn project_root(&self) -> &Path;
    pub fn root(&self) -> &Path;
    pub fn is_session(&self) -> bool;
    pub fn bundle_outdir(&self) -> PathBuf;       // == root (bundle.mjs, bundle-runtime.mjs, their maps, wasm assets)
    pub fn dev_pages_root(&self) -> PathBuf;      // root/dev-pages
    pub fn dev_assets_root(&self) -> PathBuf;     // root/dev-assets
    pub fn liveness_probe_dir(&self) -> PathBuf;  // root/watcher-liveness-probe
    pub fn plugin_scratch_dir(&self) -> PathBuf;  // root/plugins  (#3346; never created by zfb)
    pub fn graph_bin(&self) -> PathBuf;           // default: <project_root>/.zfb/graph.bin; session: root/graph.bin
    pub fn written_roots(&self) -> Vec<PathBuf>;  // default: [<p>/.zfb, <p>/.zfb-build]; session: [<p>/.zfb, <p>/.zfb-build, root]
}
```

- **Default layout is byte-identical.** Every accessor on `default_for` returns exactly today's
  path:
  - `bundle_outdir` = `<root>/.zfb-build` (`crates/zfb/src/commands/bundler_input.rs:258-263`);
  - `dev-pages` and `dev-assets` (`dev.rs:10463-10464`, `:10496-10497`);
  - `.zfb/graph.bin` (`dev.rs:1629`);
  - the probe dir (`watcher_liveness_probe.rs:96-101`);
  - `written_roots` equal to the `.zfb` / `.zfb-build` pair at `dev.rs:2216-2225` and
    `css_source_plan.rs:81-87`.

  `default_for` takes no lock and creates nothing. #3341 pins these values in a unit test.
- **The session layout mirrors the default shape**, with the scratch root playing the role of
  `.zfb-build/`. It adds `graph.bin` at the root, so a concurrent session never shares
  `.zfb/graph.bin` (loaded at `dev.rs:3472`, saved at `:3915`). The default `.zfb/` and
  `.zfb-build/` stay in a session's `written_roots`, because a concurrent default-layout process
  still writes there.
- **`.zfb/` staging stays where it is.** `KNOWN_FIRST_PARTY_STAGING_JSON_DIRS = &[".zfb"]`
  (`bundler.rs:1050`) is authored-import surface. Moving it would break imports.
- **An out-of-project bundle outdir works with every consumer (verified by reading the code):**
  - V8 host: `bundle_asset_root` is `bundle_path.parent()` (`crates/zfb/src/v8_host_adapter.rs:142-147`,
    `:289-294`), and the bundle is read from that absolute path (`:183`, `:313`). The loader's
    wasm/sourcemap containment is canonical and relative to that root only
    (`crates/zfb-render/src/embedded_v8/module_loader.rs:318-360`).
  - The dev re-reads (V8 boot and the skip key, `dev.rs:8879-8893`) read
    `BundlerOutput.bundle_path`. That path is derived from `BundlerInput.outdir`, so threading the
    outdir is sufficient. **No change is needed in `v8_host_adapter.rs`.**
  - Build adapter pass: `AdapterBundleInput.input_bundle` is the returned absolute path
    (`crates/zfb/src/commands/build.rs:7393-7405`). Wasm assets follow a bundle-relative contract
    (`crates/zfb-build/src/adapter.rs:186-194`), which the Cloudflare adapter enforces relative to
    `dirname(inputBundlePath)` (`packages/zfb-adapter-cloudflare/src/emit-worker.mjs:32-64`).
    The bundler's own wasm containment is canonical against the outdir (`bundler.rs:5809-5840`).
  - Build-token digest: the output-dir exclusion (`bundler.rs:3105-3109`) matters only for
    in-project paths. An out-of-project root is never walked. An in-project root is under
    `.zfb-build/` (R2) and is skipped by the `.zfb-` name rule (`bundler.rs:3118`) before the
    output-dir check.
  - Esbuild wasm output keys relative to the shadow cwd resolve through
    `resolve_metafile_output_path` (`bundler.rs:5849-5866`).

  #3347 exercises this end to end with a Cloudflare build that uses an out-of-project scratch dir.
- **Sites routed through the layout (#3341, no behavior change):**
  - `bundler_input.rs:261` (`project_root.join(".zfb-build")`) becomes a `bundle_outdir: &Path`
    parameter. That is `ScratchLayout::bundle_outdir()` from both callers, `build.rs:7059` and
    `dev.rs:8795`.
  - `build.rs:7395` uses `RUNTIME_BUNDLE_BASENAME`.
  - `dev.rs:10463` `dev_html_root_for` and `:10496` `dev_assets_root_for` become
    `layout.dev_pages_root()` / `layout.dev_assets_root()`.
  - `dev.rs:1629` becomes `layout.graph_bin()`.
  - `dev.rs:2216-2225` becomes `layout.written_roots()`, chained with `dev_html_root`,
    `dev_assets_root`, and the existing `outDir` guard.
  - The overlap-guard error text at `dev.rs:1335` and `:1365` names the layout root instead of the
    literal `.zfb-build/`.
  - `css_source_plan.rs:81-87`: `CssSourcePlanInputs` gains `zfb_written_roots: Vec<PathBuf>`,
    filled by `gather_css_source_plan_inputs` (`:444`), which gains a
    `zfb_written_roots: &[PathBuf]` parameter. Callers pass `layout.written_roots()`: the
    build/dev payload at `build.rs:1447` and the standalone `zfb css` / `zfb wind` plan at
    `css_support.rs:102`, which uses `ScratchLayout::default_for`.
  - `watcher_liveness_probe.rs:44-126`: `resolve_probe_parent_dir` takes the probe parent from
    `layout.liveness_probe_dir()`. Its canonical twin becomes
    `canonicalize_existing_prefix(&layout.liveness_probe_dir())`. For the default layout that
    equals today's `canonical(project_root).join(".zfb-build/watcher-liveness-probe")` whenever
    `.zfb-build` is not itself a symlink. The temp-dir relocation fallback (`:112-116`) is
    unchanged.

**Rejected alternatives.**

- A new `zfb-scratch` crate: this is one small type, and `zfb-types` already exists for exactly
  this.
- Putting the type in `crates/zfb`: `zfb-build` and `zfb-server` could not name it.
- Putting the type in `zfb-build`: `zfb-server` also depends on it, and the type is independent
  of the build pipeline.
- A separate `<scratch>/bundle/` subdirectory: it diverges from the default shape for no gain.
- Keeping `graph.bin` in `.zfb/` for sessions: two concurrent dev sessions would overwrite each
  other's persisted graph.
- Teaching `zudo-wind` the zfb layout: it would couple the owned engine to zfb, when the
  `exclusions` channel already exists.

## Decision 5: Temp files that are already isolated stay put

These are **not** moved into the scratch dir, and the design deliberately leaves them where they
are:

- **Esbuild entry, tsconfig, and virtual temp files in the project root.** Examples are
  `.zfb-esbuild-entry-*.tsx` (`crates/zfb-islands/src/esbuild.rs:1563`), the `.zfb-islands-tsconfig-*`
  and `.zfb-worker-tsconfig-*` classes (`:1592`, `:1598`), and `.zfb-virtual-*.mjs`
  (`crates/zfb-plugin-resolver/src/lib.rs:77`). They must live where esbuild's upward
  `node_modules` lookup starts. Each one is a uniquely named `NamedTempFile`. Their orphan sweep
  requires both a 30-minute age and a successful `try_lock` (`esbuild.rs:1746-1788`), so they are
  already safe under concurrency.
- **Shadow sessions**, `zfb-shadow-session-*` under `shadow_parent_dir`. The parent is guaranteed
  to be outside the project (`crates/zfb-build/src/bundler.rs:1219-1222`). Each session holds its
  own `.zfb-owner.lock` (`:1490`, `:1817-1838`).
- **Config-loader tempdirs**, `zfb-config-*` in the system temp dir
  (`crates/zfb-config-loader/src/loader.rs:255-258`).
- **The plugin TS bundle temp files** (`.zfb-plugin-bundle-*.mjs`, swept in
  `crates/zfb-build/src/plugin_bundler.rs:338-360`). They are per-file, lock-gated, and next to
  the plugin source.

The sharing is intentional; nothing is simply left over. R6 keeps a scratch dir away from the
shadow and probe reapers' name classes.

## Decision 6: Dev ignores `outDir` under a scratch dir

**Recommendation (locked).** A session-layout dev never reads the configured `outDir`. Compute
`let dist_seed: Option<PathBuf> = (!layout.is_session()).then(|| dist_root.clone());` once in
`commands::dev::run`, and log exactly one `output::info` line right after scratch resolution, for
example "scratch dir in use (`<root>`): ignoring prebuilt `<outDir>` — no dist/ fallback for HTML
or /assets". The code paths:

1. **Deferred-bundling decision.** `defer_dev_bundle_decision(..., dist_is_servable_seed(&dist_root), ...)`
   at `dev.rs:1502-1507` becomes `dist_seed.as_deref().is_some_and(dist_is_servable_seed)`.
   Short-circuiting also skips the recursive `dist/` walk (`dist_is_servable_seed`, `dev.rs:5467`).
2. **Boot-lazy Auto decision.** `run_boot_render(..., dist_root: &Path)` (`dev.rs:4285`, with
   `BootLazyMode::Auto => dist_is_servable_seed(dist_root)` at `:4316`) takes
   `dist_seed: Option<&Path>` instead, and Auto becomes `dist_seed.is_some_and(dist_is_servable_seed)`.
   The call site's `dist_root_for_boot` (`dev.rs:2944`, used at `:3505`) becomes the `Option`.
   With no seed, Auto falls through to the eager render. The existing "try cold" hint
   (`should_hint_cold_mode`, `dev.rs:5289`) stays accurate, so it is kept. Cold, the default
   (`BOOT_LAZY_DEFAULT`, `dev.rs:5198`), never consulted the seed.
3. **Request-time HTML seed leg.** `read_from_dist(&state.dist_root, …)` in `serve_page`
   (`crates/zfb-server/src/routes.rs:1524-1530`) gains the gate `&& state.dev_dist_seed`. This leg
   is also the **stale-document path**: a route marked stale whose render-on-request produced no
   file falls through here during the boot window (the waterfall doc comment at `dev.rs:3166-3187`).
4. **Request-time `/assets/*` fallback.** `build_core_router` layers
   `[dev_assets_root/assets, dist_root/assets]` (`routes.rs:684-699`). With
   `dev_assets_root.is_some() && !state.dev_dist_seed`, mount only
   `ContainedAssetsService::new(dev_assets_root.join("assets"))`.
5. `ServeOpts` (`crates/zfb-server/src/lib.rs:783`) and `AppState` (`routes.rs:375`) gain
   `pub dev_dist_seed: bool`:
   - the dev construction at `dev.rs:3076-3087` sets `!layout.is_session()`;
   - the `lib.rs:1054` conversion copies it;
   - every other construction (`embed.rs:286` and the zfb-server tests) sets `true`.

   The flag has no effect outside `ServerMode::Dev`: preview and embed already mount
   `dist/assets` single-root (`dev_assets_root = None`), and they skip the dist HTML leg.

What deliberately does **not** change:

- `create_dir_all(&dist_root)` (`dev.rs:1290-1293`) is an idempotent mkdir, not a read of stale
  bytes.
- The `outDir` entry in `set_zfb_written_roots` is kept.
- `BuildHookContext.out_dir` for preBuild (`dev.rs:1418-1425`) keeps plugin API parity.
- The `/__zfb/ready` exclusion strings (`routes.rs:925-934`).

Default dev, without a scratch dir, is byte-for-byte today's behavior.

**Rejected alternatives.**

- Pointing `dist_root` at an empty directory inside the scratch dir: it hides intent and makes
  "never reads dist" untestable.
- An env toggle: a seed carrying a foreign define is always wrong under a session, so there is
  nothing to opt into.
- Filtering the seed by a define hash: `dist/` bytes carry no machine-readable define identity.

## Decision 7: What `check` writes, and `--define` for check

**Findings.** `zfb check` (`crates/zfb/src/commands/check.rs:56-132`) loads config, validates
collection frontmatter, scans `pages/` for SSR request-param findings with `Router::scan` +
`build_prerender_map` (`check.rs:243-256`; `render_pipeline.rs:1319` reads sources only), and runs
`tsc --noEmit` with `current_dir(project_root)` (`check.rs:297-308`). It never starts a bundler, V8,
or a plugin host, so it **writes no zfb-owned artifact**. The only shared incremental output is a
`.tsbuildinfo`, and tsc writes that only when the *user's* tsconfig opts into `incremental` or
`composite`. The scaffolded template does not (`crates/zfb/templates/basic-blog/tsconfig.json`,
which sets `noEmit`).

**Recommendation (locked).**

- `check --scratch-dir` resolves, validates (Decision 3), and locks (Decision 2), then writes
  nothing into the directory. That keeps the flag, the env var, and the "one command per scratch
  dir" semantics uniform, so a script that exports `ZFB_SCRATCH_DIR` works for every command.
- **`.tsbuildinfo` is not relocated.** It is user-owned tsc state, the same as running `tsc` by
  hand concurrently. Relocating it would mean injecting `--tsBuildInfoFile`. tsc rejects that
  without `incremental`/`composite` (TS5069), so zfb would have to resolve the tsconfig `extends`
  chain to decide. Injecting `--incremental false` would change the user's settings and conflicts
  with `composite` (TS6379). The docs (#3348) name it as shared state the user controls.
- `check --define` is **parsed, validated, and merged only**. It gets the same `parse_define_override`
  errors, and the merged map has no consumer in check. It does **not** reach tsc or schema
  validation. `tsc` has no define concept; the documented pattern is to declare the global's type
  in the user's own `.d.ts` (`declare const __ORIGIN__: string;`). Accepting the flag lets one
  argument list serve dev, build, and check.

**Rejected alternatives.** Rejecting `--scratch-dir` on check would make the env var silently
ignored by one command. Feeding defines to tsc through a generated `.d.ts` would change the
typecheck surface and would have to infer types from raw expressions. A `--define` that check
ignores without validating would let a malformed value pass check and fail build.

## Decision 8: Define identity sites

The scratch-dir **location** must not change any identity. The effective define map is a
`BTreeMap` (`crates/zfb/src/config.rs:1190`, `resolve_bundle_define` at `:1803`), so its iteration
order is deterministic.

| # | Site | Crosses invocations? | Locked treatment |
|---|---|---|---|
| 1 | `zudo_react_build_token_with_inputs_and_output` (`crates/zfb-build/src/bundler.rs:2832`), called by the SSR bundler (`bundler.rs:5279`, which embeds `globalThis.__zfb.zudoReactBuild`) and the islands builder (`crates/zfb/src/commands/build.rs:4889-4896`, shared by build and dev through `build_default_islands_payload_with_bundle_options`, `build.rs:4343`) | Yes. It is the hydration identity checked in `packages/zfb/src/island-boundary.ts:37-48`. Today two invocations with different defines get the **same** token, so a mixed page (dev SSR plus another invocation's islands) hydrates silently. | **Add `define: &BTreeMap<String, String>`** as the fifth parameter. The three thin wrappers (`bundler.rs:2804-2830`) pass an empty map. Hash it **only when non-empty**, as a `b"define\0"` section of `key\0value\0` pairs after the virtual-module section. A define-less project's token is then byte-identical to today's. SSR passes `&input.define_vars` (`bundler.rs:353`); islands pass `&bundle_define` (`build.rs:4506`). **Both sites must receive the same effective map**, otherwise SSR and islands tokens diverge and hydration fails; a test must pin SSR token == islands token for one define. |
| 2 | `ModuleWorkerBuildContext` (`crates/zfb-build/src/module_worker.rs:54`; define hashed at `:195-198`), built at `build.rs:2242`, `:5944`, `:6586` and `bundler.rs:3432` | Yes (the worker `?v=` URL) | **Already covered.** Every constructor reads the post-merge `Config`, so it sees the override with no code change. #3345 adds a regression test showing a CLI define changes the worker version. |
| 3 | Content-hashed emitted filenames: islands and chunk names, client scripts, prod assets (`sha256_8`, `crates/zfb-build/src/pipeline/prod.rs:647`) | Yes | **Covered by construction.** They hash emitted bytes, and the define is baked into those bytes. No change. |
| 4 | Dev rebundle skip key `compute_bundle_skip_key` (`crates/zfb/src/commands/dev.rs:8879`) | **No.** It is per process, and the define is constant for a dev process because config is never re-read. | **No change.** It hashes the full bundle bytes (`dev.rs:8883-8900`), which already contain the substituted define. Adding the map would be redundant. The doc comment gets one line saying so. |
| 5 | Persisted-graph digest `compute_manifest_digest` (`dev.rs:4485`, over `ManifestDigest::compute`, `crates/zfb-graph/src/persist.rs:126`) | Yes (`graph.bin` reuse) | It already hashes the full bytes of `zfb.config.{json,ts}`, so a config define is covered. **Fold the CLI overrides in when non-empty:** `ManifestDigest::from_bytes(sha256(digest.as_bytes() ‖ b"cli-define\0" ‖ k\0v\0…))`, computed in `dev.rs`. There is no `zfb-graph` API change, and the default digest stays byte-identical. A mismatch only costs a cold graph. (`assemble_boot_graph` keeps live Module edges, `dev.rs:4546-4558`, so a stale graph over-invalidates and never under-invalidates. The fold is for hygiene.) |
| 6 | Shadow-session `config_fingerprint` (`bundler.rs:3408-3411`) | No (per process) | N/A. It fingerprints the markdown pipeline, which runs before esbuild applies the define. |
| 7 | Embedded `node_modules` cache key (`crates/zfb/src/embedded_node_modules_cache.rs:210`) | Yes | N/A. It hashes the vendored package bytes only. |

**Location neutrality.** Site 1 hashes logical paths and bytes only. The `output_dir` argument
only *excludes* a directory. A session root is either under `.zfb-build/` (skipped by name) or
outside the project (never walked), so moving it cannot change the digest. Site 2 hashes plugin
alias targets relative to the project root or workspace (`module_worker.rs:205-225`), never to the
scratch dir. #3345's test: the same define with two different session layouts gives an identical
token at sites 1, 2, and 5.

**Out of scope.** The SSR-only `PUBLIC_*` env payload (`public_env_define_args`, `bundler.rs:1422-1448`)
is process-env-derived and not part of this contract. Its existing precedence rule — an explicit
`bundle.define` entry wins for the exact expression — now also covers CLI defines, because they
are merged into `bundle.define`.

## Decision 9: Plugin `scratchDir` semantics

**Recommendation (locked): option (a), opaque intermediates only.**

- Every hook context carries `scratchDir: string`, always present and absolute, equal to
  `ScratchLayout::plugin_scratch_dir()`:
  - default layout: `<projectRoot>/.zfb-build/plugins`;
  - session layout: `<canonical scratch root>/plugins`.

  This deliberately refines the epic's "the layout root". The root namespace belongs to zfb:
  `bundle.mjs`, `bundle-runtime.mjs`, their maps, esbuild wasm assets with content-hash names
  (`bundler.rs:5344-5345`, `:5717-5760`), `graph.bin`, and the `dev-*` directories. A child
  directory makes collisions impossible.
- **zfb does not create the directory.** Creating it would add an entry to the default
  `.zfb-build/` layout. Plugins call `mkdir(scratchDir, { recursive: true })`, and each plugin
  should use its own `<scratchDir>/<plugin-name>/` subdirectory. The path is under `.zfb-build/`
  or out of the project, so it is already excluded from every walker (Decision 3 R2) and gitignored.
- Contexts and fields:
  - Rust, `crates/zfb-build/src/plugin_runner.rs`: `BuildHookContext` (`:218`), `DevRegisterContext`
    (`:236`), and `SetupHookContext` (`:253`) each gain `pub scratch_dir: PathBuf`, which serde
    camel-cases to `scratchDir`.
  - `PluginHost::run_setup` (`:814`) and `run_preview_setup` (`crates/zfb-build/src/plugin_registries.rs:751`)
    take `scratch_dir: &Path`.
  - Construction sites:
    - `crates/zfb/src/commands/build.rs:238`, `:351`;
    - `dev.rs:1418`;
    - `crates/zfb/src/commands/plugins.rs:128` (`run_plugin_setup`) and `:290`
      (`build_dev_middleware_set`, which gains `scratch_dir: &Path`);
    - `crates/zfb/src/commands/preview.rs:219-230`;
    - plus the tests in `plugin_runner.rs`.
  - JS: `crates/zfb/js/plugin-host.mjs` forwards `scratchDir` in the three context literals
    (`:295` build hooks, `:357` setup, `:639` dev/preview middleware).
  - TS: `packages/zfb/src/plugins.ts` adds `scratchDir: string` (non-optional) to
    `ZfbBuildHookContext` (`:89`), `ZfbDevMiddlewareContext` (`:151`),
    `ZfbPreviewMiddlewareContext` (`:187`), and `ZfbSetupContext` (`:277`).
- **Preview provides it:** the session layout under `--scratch-dir` / `ZFB_SCRATCH_DIR`,
  otherwise the default. `check` runs no plugin host.
- **Plugin isolation requires plugin adoption.** zfb's own artifacts become concurrency-safe
  under a scratch dir. A plugin's own staging (for example zudo-doc's `<root>/.zudo-doc/`) stays
  shared until that plugin moves its intermediates under `scratchDir`. #3346 files the zudo-doc
  upstream issue.
- **Why not (b), importable generated routes.** A route module under a scratch dir cannot be
  imported without new mechanism:
  - Out of the project, it violates the canonical graph contract that
    `virtual_module_absolute_symlink_escape_keeps_contract_error`
    (`crates/zfb-build/tests/module_worker_plugin_virtual_absolute_deps.rs:114`) pins.
  - Under `.zfb-build/` it is a hidden directory that no walker stages. The only staged dot-path
    is the fixed allowlist `KNOWN_FIRST_PARTY_STAGING_DIRS = &[".zudo-doc/routes-src"]`
    (`bundler.rs:1040`, plus the lexical import handling at
    `crates/zfb/src/commands/package_routes.rs:680`). The stage-escape audit would hard-fail it.

  (b) would need a per-invocation staging allowlist entry, an import-specifier rewrite, and
  audit coverage. Record it as a follow-up and do not build it in this epic.

**Rejected alternatives.** Exposing the layout root itself: zfb and plugin names would collide.
Making `scratchDir` optional: plugins would have to branch on it, and the epic's delegated decision
(non-optional) holds. Option (b) now: see above.

## Downstream map

| Sub-issue | Owns |
|---|---|
| #3341 | `ScratchLayout` + consts in `zfb-types`; `canonicalize_existing_prefix` hoist; route all Decision 4 sites through `ScratchLayout::default_for`; default-layout pin test. No behavior change. |
| #3342 | The `ScratchDirArg` clap group and `scratch_dir.rs` (select, validate R1–R6, create, lock, marker). Call it at the top of the four `run`s and hold the lease. `ScratchLayout::for_scratch_dir`. |
| #3343 | Thread the resolved layout through dev, build, preview, and check paths and all exclusion sites (Decision 4). |
| #3344 | Decision 6 (four read sites + `dev_dist_seed`). |
| #3345 | The `DefineArgs` clap group and `--define` parse, merge, and validation (Decision 1); identity sites 1, 2, and 5 (Decision 8). |
| #3346 | Decision 9, plus the zudo-doc upstream issue. |
| #3347 | Heavy end-to-end confirmation. |
| #3348 | EN and JA docs for the locked surface. |
