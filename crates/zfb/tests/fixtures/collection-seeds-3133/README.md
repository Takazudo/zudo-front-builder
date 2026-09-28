# collection-seeds-3133

Workspace fixture for the staging regression test in
[`collection_seed_3133_baseline.rs`](../../collection_seed_3133_baseline.rs).
It verifies that an out-of-root content collection seeds staging only with
files that survive its include filter.

## Shape

The temporary project has an `apps/site` zfb app and several sibling workspace
packages. `apps/site/tsconfig.json` has a non-empty `paths` map so the test
exercises the workspace package-resolution path. The two config variants are
identical except that `zfb.config.with-collection.json` declares a collection
root at `../../packages/ui/src/components` with `include: ["**/*.mdx"]`;
the control config has no collection.

The collection contains an included MDX file and sibling TSX files excluded by
the filter. Both variants must report
`physical_scans=0 logical_visits=0 workspace_staging_activated=false`. This
keeps the regression focused on seed selection rather than framework or CSS
runtime behavior.

## Temporary package tree

The harness creates a fresh workspace copy for each run. It builds a synthetic
pnpm-style store for the private package and workspace links, copies the
vendored `@takazudo/zfb` and `@takazudo/zfb-runtime` trees into the consumer's
store layout, and links Hono beside the runtime. Nothing under `node_modules/`
is checked in. The app's dependency links mirror the fixture manifest, while
only packages reached by the selected import graph enter the staging walk.

A physical dependency linked from several workspace packages gives the test
multiple logical paths to one canonical directory. The staging walker should
visit the logical paths as required while parsing a physical package only
once. See the Rust test's comments for the current harness and measured stats.
