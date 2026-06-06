# zfb-plugin-resolver

Shared exact-match resolver inputs for the two esbuild call sites in the workspace.

Both the main page/layout bundler (`zfb-build::bundler`) and the islands bundler
(`zfb-islands::esbuild`) need to forward plugin-registered aliases and virtual modules to
esbuild. This crate provides that shared logic.

## Why this crate exists

esbuild's `--alias` flag uses **prefix-with-slash** matching: registering `@/foo` would
silently match `@/foo/bar` too, contradicting the exact-match contract that
`zfb-render::BundleModuleLoader::resolve_alias` enforces in the V8 host. This helper
produces the inputs needed for exact-match aliases by expressing them as
`compilerOptions.paths` entries in a synthetic tsconfig instead — a bare specifier in
`paths` (no `*` wildcard) is a literal exact match in TypeScript / esbuild.

A tiny leaf crate is also necessary to break a dependency cycle: `zfb-build` depends on
`zfb-render` and `zfb-content`; `zfb-islands` does not depend on `zfb-build` (and
`zfb-build` re-uses `zfb-islands` through the orchestrator's bundling fan-out). Placing
this logic in either crate would create a cycle or pull in the entire orchestrator graph.

## Public API

### `build_resolver_inputs`

```rust,ignore
pub fn build_resolver_inputs(
    aliases: &[(String, String)],
    virtual_modules: &[(String, String)],
    working_dir: &Path,
) -> Result<ResolverInputs>
```

Given plugin-registered aliases and virtual modules:

1. Writes each virtual module's source to a `.mjs` temp file under `working_dir`
   (held alive in `ResolverInputs::_temp_files`).
2. Builds `paths_entries` — a `Vec<(specifier, absolute-path)>` with paths
   normalized to POSIX forward-slash form for platform-stable JSON.

Callers write the merged entries into a synthetic tsconfig and pass `--tsconfig=<path>` to
esbuild. No `--alias` flags are emitted for plugin entries.

Falls back to the system temp dir when `working_dir` is not an existing directory (matches
the islands path's existing unit-test convention).

### `ResolverInputs`

```rust,ignore
pub struct ResolverInputs {
    /// (specifier, absolute-path) pairs for compilerOptions.paths.
    pub paths_entries: Vec<(String, String)>,
    /// Held-alive temp-file handles for virtual-module .mjs files.
    pub _temp_files: Vec<NamedTempFile>,
}
```

The caller MUST keep the `ResolverInputs` value alive until esbuild has finished reading
the virtual-module files — `Drop` deletes them from disk.

### `merge_into_tsconfig_paths`

```rust,ignore
pub fn merge_into_tsconfig_paths(
    paths: &mut BTreeMap<String, Vec<String>>,
    entries: &[(String, String)],
)
```

Folds `entries` into the caller's existing `compilerOptions.paths` map.
**Collision policy: user wins.** Pre-existing entries (from `BundlerInput::tsconfig_paths`)
are left untouched; plugin entries are additive.

## Tests

```sh
cargo test -p zfb-plugin-resolver
```

Tests in `src/lib.rs` cover: empty inputs, alias exact-match (no wildcard), virtual-module
temp-file materialization under `working_dir`, system-tmpdir fallback when `working_dir`
is missing, `merge_into_tsconfig_paths` (new keys, collision policy, combined).
