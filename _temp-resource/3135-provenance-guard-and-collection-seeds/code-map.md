# Code map — Provenance Guard and Collection Seeds (epic #3135)

Planning-time exploration, verified at `741c26db`. Line numbers drift — treat them as starting points and re-grep the symbol.

## Track A — release / provenance (#3136, #3137, #3140)

### Today there is no blocking guard

- Publish-time: `emit_trust_downgrade_advisory` (`scripts/publish-npm-packages.sh:266-322`) — one `::warning` + job summary, never fails (`:318`). Tests: `tests/unit/publish-npm-packages.sh:268-316` (run by `health.yml:200-212`).
- After the fact: `scripts/check-provenance-drift.mjs`, weekly via `drift-net.yml:99-123` (`provenance-latest` job; cron `43 3 * * 3`); files a deduped issue on failure.
- Skills/checklist describe the later drift failure as "intended supervision, not a bug": `.claude/skills/l-make-release/SKILL.md:4,23,545,658`, `RELEASE_DAY_CHECKLIST.md:263-266`.

### Fast-Mac path end to end

- `l-make-release/SKILL.md`: `:23` flag meaning; `:63-68` Step 1 Darwin precondition; `:497-507` reuse-draft (strips Mac assets without `--fast-mac`); `:541-579` Step 10 runs `./scripts/build-macos-x64-local.sh --upload v<version>` (`:565`); `:650-669` report text; `:789-806` recovery guidance.
- `l-make-mac-release-binary/SKILL.md:13-21` standalone separate-Mac flow; `:79-82` same build script.
- `scripts/build-macos-x64-local.sh:35-43` archive contract; `:289-327` `gh release upload --clobber`.

### release.yml

- `:76-85` dispatch input `skip_macos_x64`.
- `detect-mac-local` job `:238-307`; checks out `needs.release-context.outputs.checkout_sha` at `:253` (old tree on recovery!); decides `mac_local_present` from Release assets `:283-307`.
- `prepare-matrix` `:309-352` drops the `macos-15-intel` leg when mac-local (`:344-346`).
- `publish` job `:804`; `id-token: write` `:815-820`; `.release-control` checkout at `github.sha` `:832-837`; downloads the pre-uploaded archive `:858-901`; lockstep version check `:943`; publish context `:1007`; binary mode `:1026-1050`; skip-existing-version `~:1052`.
- Publish mode selection `:1065-1093`: normal `all-provenance` (`:1066-1071`); fast-Mac `mac-local` (`:1077-1082`, darwin-x64 without `--provenance`); recovery `.release-control/scripts/publish-npm-packages.sh recovery-no-provenance` (`:1088-1093`, none of the 10 attested).
- `notify` `:1144-1207` fires only on failure.

### publish-npm-packages.sh

- Package dirs `:78-105` (`PLATFORM_DIRS` + `NONPLATFORM_DIRS`); `publish_platform_packages` `:208-227` (`:220` clears `prov` for darwin-x64 in `mac-local`); non-platform `:229-264`; mode validation `:334-340`; advisory call `:350`.

### check-provenance-drift.mjs (reuse in the preflight)

- Imports `PUBLISHED_PACKAGES` from `retire-next-dist-tag.mjs` (`:48`); `hasAttestation` `:53`; pure `classifyPackage` `:72-119` (publish-date ordering; prereleases ignored when latest is stable, `:98-101`); `fetchPackument` with retry `:132`; `checkAll` `:164`; `report` `:180-224`.
- Tests: `scripts/__tests__/check-provenance-drift.test.mjs:32-290`.

### Lockstep package lists (four places)

`scripts/retire-next-dist-tag.mjs:52-63` (`PUBLISHED_PACKAGES`), `scripts/advance-latest-dist-tag.sh:40-49`, `scripts/publish-npm-packages.sh:78-105`, `release.yml:1030-1034`.

### Docs

- `RELEASE_DAY_CHECKLIST.md`: flows `:9-24`; macOS-x64 escape hatch `:92-157` (fast-Mac `:119-156`); recovery `:158-194`; recovery trust downgrade `:196-236`; provenance trade-off + 2.13.0 restore `:238-266`.
- `docs/src/content/docs/guides/troubleshooting.mdx:51-80` (+ `docs-ja` twin): `ERR_PNPM_TRUST_DOWNGRADE` entry covers only 2.12.0.

## Track B — bundler staging (#3138, #3139, #3141, #3142, #3143)

All in `crates/zfb-build/src/bundler.rs` unless noted.

### Verified root cause

1. `:3072-3117` builds `source_graph_roots`; `:3083-3089` pushes `content_dir` (no collections) or every `resolver.resolve(&collection.root)`, ignoring include/exclude and `allow_outside_root`. Out-of-root collection files have no logical importer; their imports resolve from the synthetic `<project>/entry` importer (`:10524-10531`).
2. `collect_project_source_module_graph_seed_files` `:10345`, walk `:10361-10362` (`follow_links(true)`); applies only `bundle.exclude`, `is_pruned_infra_dir`, node_modules skip. Seeds = `raw_source_extension` (`:6868`: ts tsx js jsx mjs cjs mts cts) + `.css`. **`.mdx`/`.md` are never seeds.** Doc comment `:10334-10344`: "Every source file is itself a seed, so no transitive first-party discovery is needed".
3. `extend_node_modules_dependency_staging` `:10393`: `visited` (logical) `:10459`; `deferred_live_dependencies` `:10462` (pushes `:10489`, `:10575`); workspace deps staged immediately `:10568-10572`; `deferred_physical_dependencies` `:10604` (push `:10718`); `workspace_staging_active` flip `:10769-10780`; deferred drained `:10787-10832`; fixpoint ends `:10834`. Same predicate recomputed as `workspace_package_staging_active` `:3186`.
4. Per-package scan `:10622-10660` — `WalkDir::new(&physical_root).follow_links(true)`, prunes nested `node_modules`/`.git` only, calls `collect_runtime_import_specifiers_from_file` (`crates/zfb-build/src/module_worker.rs:2112`, full swc parse). `package_was_symlinked` depends on the logical root (`:10618-10619`). Canonical-first branch `:10667-10674` gives each dep logical path `logical_root/node_modules/<pkg>`; `staged_equivalent_dependency_is_reachable` `:10177` dedupes only along ancestor node_modules. **A pnpm-private dep reachable via N logical paths is scanned N times; no per-physical-dir cache.**
5. Copy: `materialise_isolated_exact_dir` `:7203` (walk `:7218-7219`), called `:4208`, `:4253`. On unix with empty `bundle.exclude` + workspace staging + no preserve-symlinks, ordinary deps are **symlinked** (`link_ordinary_dependency_to_canonical_source` `:7157`, guards `:4194-4205`, `:4239-4250`) — scan dominates, not copy.
6. `crates/zfb/src/commands/bundler_input.rs:352-362` maps `config.collections` → `ContentCollectionSpec` (name, root, include, exclude, id_strip_suffix; **no** allow_outside_root) for both dev and build.

### Config / filter

- `crates/zfb/src/config.rs` collection struct ends `:864` (`allow_outside_root`); validation `:2299-2312`; tests `:4000-4090`.
- `ContentCollectionSpec` `bundler.rs:200-216`; available at `:3085` before the seed walk at `:3108`.
- `zfb_content::collection::CollectionFilter::new` / `matches_relative` — `crates/zfb-content/src/collection.rs:182/203/260`. Used by `materialise_collection` (`bundler.rs:8839`, applied `:8925-8944` — md/mdx/tsx only; `tsx` counts as content extension, so non-included `.tsx` is dropped from the shadow copy).

### Why collection roots seed the graph

- Origin cd9a9b1a (#1645): with `bundle.exclude` set there is no live `<shadow>/node_modules`, so every bare import must be staged.
- `source_graph_roots` feeds only `root_entry_dependency_seed_files` → staging (`:3149`). Dev watcher / narrowing / HMR are config-driven: `crates/zfb/src/commands/dev.rs:202` (`configured_collection_watch_paths`), `:238` (`derive_watch_roots`), `:1523` (`resolve_roots`), `:2174-2184` (`out_of_root_watch_roots`). Materialisation is separate (`preflight_raw_tree` `:3270`, `materialise_collection` `:3382`).
- Useful for #3142: `compile_mdx_to_jsx_module_cached` (`crates/zfb-content/src/mdx_jsx_emit.rs:~3772`), swc `collect_import_specifiers` (module_worker path `:2112`), `discover_module_preprocessing_with_context` (`bundler.rs:~2950`, transitive first-party closure — verify it accepts `allowOutsideRoot` files).

### Existing tests

- Out-of-root: `crates/zfb/tests/collections_outside_root_build_check_snapshot.rs` (fixture `tests/fixtures/collections-outside-root/`); `crates/zfb/tests/dev_out_of_root_collection_e2e.rs:560` (fixture `dev-out-of-root-basic/`, skips without esbuild); fixture `dev-content-reload-2063-outofroot/`; `crates/zfb-build/tests/bundler_out_of_root_import.rs`, `bundler_out_of_root_import_workspace.rs`. None have imports / workspace packages / sibling components.
- Staging: `crates/zfb-build/tests/bundler_exact_match_resolution.rs` (61 tests) — e.g. `node_modules_exact_target_stages_allowed_bare_dependency_closure` (973), `node_modules_symlinked_package_stages_non_hoisted_canonical_dependency_only_without_preserve` (1034), `workspace_isolation_only_falls_back_to_pnpm_store_without_explicit_preserve` (1122), `cyclic_pnpm_dependencies_reach_a_fixed_point_with_bundle_exclude` (1214), `first_party_package_stages_pnpm_symlinked_bare_dependency_closure` (1429), `page_bare_import_is_staged_under_bundle_exclude` (3101), `alias_colliding_transitive_dependency_is_nested_under_its_importer` (3325).
- Bundler unit tests: `workspace_package_staging_*` (12921, 12944, 13014), `materialise_workspace_package_is_real_bounded_copy_on_every_platform` (17978), `glob_fixed_point_stages_plain_dependency_closure_of_a_matched_file` (22544).
- Heavy / env-gated: `crates/zfb-build/tests/bundler_root_workspace_stage_escape_audit_armed_regression.rs:102`; `crates/zfb/tests/client_bundling_cross_pipeline.rs:1356`. Also `bundler_workspace_pkg_alias.rs`, `bundler_sibling_wholesale_mirror.rs`, `module_worker_workspace_first_party_roots.rs`.
- Nothing tests `collect_project_source_module_graph_seed_files` or `extend_node_modules_dependency_staging` directly.

### Lessons that apply

- `.claude/skills/l-lessons-client-bundling/SKILL.md`: Rust gathers candidates, esbuild picks — don't reimplement resolution (rules out "walk only exports-reachable files"); don't reopen live-tree escape hatches (keep stage-escape audit tests green); escalate after two review rounds on the same seam.
- `.claude/skills/l-lessons-dev-watcher-narrowing/SKILL.md`: watcher membership is separate from bundler seeds; prove any new guard/cache with revert-and-fail; instrument with `ZFB_DEV_TIMING=1` before theorising; run `cargo check --no-default-features -p zfb --tests` if touching `embed_v8` cfg boundaries.
