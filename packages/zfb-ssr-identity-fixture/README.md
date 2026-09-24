# Public SSR identity regression dependencies

`crates/zfb/tests/build_package_routes.rs` uses this private workspace package
for the public `withZudoSg` catalog route regression. CI's existing
`pnpm install --frozen-lockfile` step provisions it before nextest runs. The
test fails if the installed public package is absent; only the optional
esbuild prerequisite retains the repository's local self-skip convention,
and CI asserts that esbuild was provisioned.

The direct versions in `package.json` are exact, including
`@takazudo/zudo-sg@0.3.4`, `@takazudo/zudo-doc@5.27.0`, and
`preact@10.29.8`. `pnpm-lock.yaml` records their resolved peer graph and
the registry integrity for zudo-sg 0.3.4:
`sha512-PsTX9VmyAU9zZYs7fLBf7fiPX+sNPGxViceJm308YLEdFJ7XwTblnztbMXhYGcNq9lV1bTBh9QpwnJGJfV0hBw==`.
The tarball is the published npm package observed on 2026-09-24, not a
locally copied or patched route. The test builds its route through that
package's `withZudoSg` configuration and `ComponentThumb` renderer.
