# CSS API verification

The native binding is an `rlib` in workspace builds. Build the loadable addon
separately so workspace feature unification cannot link embedded V8 into it:

```sh
bash scripts/build-css-node.sh x86_64-unknown-linux-gnu cross
node scripts/smoke-css-node.mjs packages/zfb-linux-x64-gnu/zfb-css.node
```

Use `cargo` instead of `cross` on the Darwin and Windows targets. The helper
stages only the addon; existing CLI build/staging steps still own the executable.
Linux uses the release containers from `Cross.toml` and checks GLIBC <=2.34.
Run local heavy commands through the workspace heavy guard.

The Linux packed consumer test requires built SDK/slugify JavaScript, pnpm,
Node, a fresh CLI and addon, and `strace`:

```sh
pnpm --filter @takazudo/zfb-slugify --filter @takazudo/zfb build
ZFB_LINUX_X64_BINARY=/absolute/path/to/zfb \
ZFB_LINUX_X64_CSS_ADDON=/absolute/path/to/zfb-css.node \
bash scripts/smoke-css-api-packed.sh
```

It packs the real carrier with npm (preserving executable permissions), packs
the SDK with pnpm (applying published exports), installs outside the workspace,
and pins the tested dependencies to those local tarballs. It checks installed
paths and declarations, removes the carrier CLI, then verifies the real API.
It reuses CSS fixtures and golden expectations alongside API-specific anchors,
preset/manifest ownership, configuration projection, diagnostics, recovery,
output atomicity and concurrent calls with distinct roots.

The final process compiles in a project with filesystem writes denied. Its
native syscall trace must contain only the initial Node exec, thread-only clones,
and no filesystem mutations or writable opens. This observes Rust subprocesses
as well as JavaScript subprocesses. The trace and installed fixture remain in
the printed clean-room directory for inspection.

Set `ZFB_CSS_API_BROWSER=1` after installing Playwright Chromium to run the bounded
computed-style fixture as well. `CHROMIUM_EXECUTABLE` can select a local Chromium.
The fixture checks utility placement in both directions and the minimal reset.
The scaffold CI gate runs this packed test, trace and browser fixture. Release
jobs run thin native load/compile checks on all five carriers; Linux checks use
Node 22.0.0. Fast-Mac requires x64 Node execution before uploading its separate
addon/checksum assets. Installer archives still contain exactly one CLI binary.

Source-only checks, a Linux debug addon, and baseline main CI do not establish
all-platform release acceptance. Retain the delivery gate until the current
revision's five release artifacts are built, staged and executed successfully.
