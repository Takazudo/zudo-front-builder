# zfb-plugin-resolver

Shared resolver inputs for the page/SSR bundler (`zfb-build::bundler`) and
browser islands, client scripts, and module-worker jobs
(`zfb-islands::bundle_job`). These callers pass typed requests to the native
`zfb-bundler`; this crate does not locate or launch a compiler executable.

## Resolution contract

Plugin aliases are exact-match registrations. An alias `@/foo` must not also
claim `@/foo/bar`. The helper expresses aliases as literal
`compilerOptions.paths` entries, with no wildcard, and merges them into the
caller's synthetic tsconfig without replacing user entries.

Direct resolver aliases supplement the paths map for virtual modules and
eligible single-file plugin aliases, including imports from installed package
entrypoints. Directory aliases stay paths-only. This preserves the constraints
introduced for the former esbuild backend (#269, #1263, #1267, #3002); current
callers consume `(specifier, target)` pairs through `BundleRequest.resolve.aliases`.
The public `virtual_module_alias_flags()` and `plugin_alias_flags()` formatters
still return the legacy `--alias:<specifier>=<path>` spelling. They remain
available, but the workspace's native bundlers do not build CLI argv from them.

A small shared crate lets `zfb-build` and `zfb-islands` use the same policy
without making the browser bundler depend on the build orchestrator.

## Public API

### `build_resolver_inputs`

```rust,ignore
pub fn build_resolver_inputs(
    aliases: &[(String, String)],
    virtual_modules: &[(String, String)],
    working_dir: &Path,
    user_tsconfig_paths: &BTreeMap<String, Vec<String>>,
    reserved_specifiers: &[&str],
) -> Result<ResolverInputs>
```

- `aliases` contains `(from, absolute filesystem target)` registrations.
- `virtual_modules` contains `(specifier, ESM source)` registrations.
- `working_dir` anchors materialized virtual-module files and their dependency
  lookup. An existing directory receives `.zfb-virtual-*.mjs` files; a missing
  directory uses the system temp directory with the `zfb-virtual-` prefix.
- `user_tsconfig_paths` is the consumer's own `compilerOptions.paths` map.
  When an exact key or wildcard claims a virtual specifier, that registration
  is suppressed from both output resolver lists. Its file is still held alive.
- `reserved_specifiers` lists caller-owned direct aliases. Current page/SSR
  and browser jobs reserve `zfb` for the public SDK mapping. An overlapping
  plugin alias stays paths-only.

The helper returns all four fields below; paths use POSIX forward slashes.

```rust,ignore
pub struct ResolverInputs {
    pub paths_entries: Vec<(String, String)>,
    pub virtual_module_alias_args: Vec<(String, String)>,
    pub plugin_alias_args: Vec<(String, String)>,
    pub _temp_files: Vec<VirtualModuleFile>,
}
```

`paths_entries` contains every plugin alias and each user-unclaimed virtual
module. `virtual_module_alias_args` contains only those virtual modules.
`plugin_alias_args` contains aliases whose target is an existing file, whose
name passes the retained alias-key validation, which the user does not claim,
and which have no equal or slash-prefix collision with another alias, virtual
module, or reserved specifier (`a` overlaps `a/b`, but not `ab`). The page/SSR
caller additionally admits direct plugin aliases only inside its staging root.

`_temp_files` holds content-addressed `VirtualModuleFile` handles, not
`NamedTempFile`s. A virtual module's filename is derived from its specifier and
source. Concurrent holders can share it; the last holder removes it on drop.
Keep the entire `ResolverInputs` alive until the native bundle operation has
finished reading these files, including a module-worker job that uses them.

### Paths and raw-import helpers

`merge_into_tsconfig_paths(&mut paths, &entries)` adds exact entries to the
caller's `BTreeMap<String, Vec<String>>`; existing keys win. For virtual modules,
`build_resolver_inputs` also handles wildcard user claims before this merge.

`read_tsconfig_paths`, `read_tsconfig_paths_file`, and their `_into_map`
variants read JSONC and the supported `extends` chain. `TsConfigPaths` carries
`base_dir`, the explicit/inherited optional `base_url`, and `TsPathAlias`
entries; `resolve_tsconfig_path_target` converts relative targets to absolute
paths. `collect_tsconfig_dependency_inputs` and
`collect_tsconfig_dependency_chain` expose config dependencies for invalidation.

`RawImportAliasContext` and `resolve_raw_target_with_aliases` apply the shared
first-party tsconfig paths/baseUrl context to terminal `?raw` imports.
`resolve_raw_target` is the relative-target convenience form;
`validate_raw_candidate` checks the candidate boundary.

## Tests

```sh
cargo test -p zfb-plugin-resolver
```

Tests cover exact alias entries, virtual-file materialization and sharing,
missing-directory fallback, user exact/wildcard claims, single-file alias
eligibility and reserved-specifier collisions, tsconfig merges and inheritance,
and raw-import resolution.
