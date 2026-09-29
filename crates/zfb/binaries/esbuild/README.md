# `crates/zfb/binaries/esbuild/`

Workspace staging directory for the esbuild standalone executable used by
`zfb-islands`. The production `zfb` binary embeds the staged file from
`$OUT_DIR/vendor/bin/` and extracts it to a temporary directory when needed.

The executable is not committed. `.gitignore` excludes the binary path, while
`.gitkeep` preserves this directory in git.

The workspace path is `crates/zfb/binaries/esbuild/esbuild` (or `esbuild.exe`
on Windows). Set `ZFB_ESBUILD_BIN` or configure
`EsbuildSubprocessConfig::with_binary_path` to use a different executable.

`crates/zfb/build.rs` downloads the pinned package for the current platform,
verifies the executable's SHA-256, stages it here, and copies it to
`$OUT_DIR/vendor/bin/esbuild` for embedding. Runtime resolution prefers an
explicit configured path, then `ZFB_ESBUILD_BIN`, then the embedded copy; a
direct workspace flow can fall back to the staged path.
