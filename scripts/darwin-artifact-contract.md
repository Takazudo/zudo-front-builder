# Darwin release-input contract (owner Mac → admission)

Issue #4689 (parent #4687). Both Darwin targets' release inputs are produced and
qualified on the owner's Apple Silicon Mac, then admitted on Linux before any
irreversible release step. No hosted macOS runner is added, enabled, dispatched or
used as a fallback.

| Piece | File |
| --- | --- |
| Producer (Mac only) | `scripts/build-darwin-local.sh` |
| Shared constants, receipt writer | `scripts/darwin-artifact-contract.mjs` |
| Admission verifier (Linux-runnable, node builtins only) | `scripts/admit-darwin-artifacts.mjs` |
| Tests | `scripts/__tests__/admit-darwin-artifacts.test.mjs` (in-process), `scripts/__tests__/admit-darwin-artifacts-cli.test.mjs` (CLI exit codes and `--check-only` refusals) |

## Prerequisite: Mac evidence exists before a release starts

Only the owner's Mac can create Darwin evidence. A Codex Cloud session, a Linux
runner or any other non-Mac initiator can only **consume** Darwin inputs that were
already built, qualified and uploaded to the draft Release. When any of the ten
Darwin assets below is missing or rejected, the release is **blocked**: hand the
owner a paste-ready Mac prompt. Missing inputs are never a reason to use a hosted
macOS runner, to skip admission, or to publish without the Darwin carriers.

## Assets per target

For `{v}` = `packages/zfb/package.json` version and `{triple}` in
`aarch64-apple-darwin`, `x86_64-apple-darwin`:

| Asset | Contents |
| --- | --- |
| `zfb-{v}-{triple}.tar.gz` | gzip tar with exactly one regular root entry `zfb`, mode `0755` (locked contract in `release.yml`) |
| `zfb-{v}-{triple}.tar.gz.sha256` | one line `<64 lowercase hex>  <archive basename>` (GNU, two spaces) |
| `zfb-css-{v}-{triple}.node` | the `zfb-css-node` cdylib from `scripts/build-css-node.sh {triple} cargo` |
| `zfb-css-{v}-{triple}.node.sha256` | GNU line for the addon |
| `zfb-{v}-{triple}.qualification.json` | the receipt below |

The producer archives with `COPYFILE_DISABLE=1 tar --no-mac-metadata --no-xattrs`.
Plain macOS `bsdtar` adds a hidden `._zfb` AppleDouble entry whenever the binary
carries extended attributes (`tar -t` does not list it), which breaks the
one-root-file contract; the verifier rejects such an archive.

## Producer CLI

```sh
ZFB_X64_NODE=/path/to/x64/node \
  scripts/build-darwin-local.sh --source <full-sha> [--tag v<version>] \
  [--out-dir DIR] [--upload v<version>] [--check-only]
```

- Refuses a dirty tree (tracked changes or untracked non-ignored files), a
  `--source` that is not `HEAD`, a `--tag` that is not `v<version>` or does not
  peel to `--source` (without a local tag ref, a DRAFT Release for the tag whose
  `targetCommitish` is exactly `--source`; a branch-name target or a published
  Release is refused), an out dir inside the checkout or not empty, a non-Darwin
  host, a host without `hw.optional.arm64=1`, missing Rosetta 2 and a missing x64
  Node. `--check-only` stops after the argument and source-binding refusals.
- Builds serially, one target and one command at a time:
  `ZFB_RELEASE_VERSION={v} cargo build -p zfb --release --target {triple}` and
  `scripts/build-css-node.sh {triple} cargo`, each through the heavy guard
  (`HEAVY_GUARD`, else `$HOME/.claude/scripts/heavy-guard.sh`, else
  `$HOME/.codex/scripts/heavy-guard.sh`, else direct). `CARGO_BUILD_JOBS` is
  honored and recorded. The toolchain stays process-local (`RUSTUP_TOOLCHAIN` or
  the checkout's `rust-toolchain.toml`); the script never runs `rustup default`
  or `rustup override`. It fails if the build modified the checkout.
- Qualifies the bytes extracted from the archive it just wrote, each check once.
  arm64 checks run under `arch -arm64`; x86_64 checks run under `arch -x86_64`
  with `ZFB_X64_NODE`.
- Writes both receipts, then runs the admission verifier on its own output.
- `--upload TAG` uploads only to an existing **draft** Release, never with
  `--clobber`. Before uploading anything it compares every same-named remote
  asset: identical bytes are skipped, different bytes stop the run.

`release.yml` admits through `admit-darwin-artifacts.mjs` (#4690): its
`admit-darwin` job runs `scripts/release/admit-release-darwin.sh`, which downloads
exactly these ten assets from the Release into a fresh directory and fails,
naming this producer, when any is missing or rejected. The publish job places
Darwin carrier bytes only from that admitted set
(`scripts/release/place-darwin-carriers.mjs`, digests re-verified first).
`scripts/build-macos-x64-local.sh` no longer feeds releases: its `--upload` is
refused, and it remains only for the hosted carrier-evidence workflow.

## Receipt schema `zfb-darwin-qualification/v1`

| Field | Meaning |
| --- | --- |
| `schema` | `"zfb-darwin-qualification/v1"` |
| `version`, `tag` | release version; `tag` is `v{version}` or `null` |
| `target` | the Rust triple |
| `source.commit`, `source.tree` | `HEAD` and its tree at build time |
| `host.uname_m` | `uname -m` inside the measured process: `arm64` or `x86_64` |
| `host.os_version` | `sw_vers` name, version and build |
| `host.hw_optional_arm64` | `1`: the host is Apple Silicon |
| `host.translated` | `sysctl.proc_translated` measured inside the target's process: `0` arm64, `1` x86_64 |
| `host.arch_label` | `native-arm64` or `rosetta-translated` |
| `toolchain` | `rustc_vV`, `cargo_V`, `rust_toolchain_toml_sha256`, `cargo_lock_sha256`, `profile: "release"`, `features: "default"`, `env.ZFB_RELEASE_VERSION`, `env.CARGO_BUILD_JOBS` |
| `inputs` | SHA-256 of `Cargo.lock`, `rust-toolchain.toml`, `pnpm-lock.yaml` |
| `artifacts` | `{name, sha256, size}` for `binary` (the archived `zfb`), `addon`, `archive`, `archive_sha256_file`, `addon_sha256_file` |
| `file_output` | `file -b` of the binary and the addon |
| `qualification.checks[]` | `{name, command, exit_code, first_result, retries: 0, output, expected?}` |
| `qualification.status` | `passed` only when every required check exists and passed first time |
| `created_at` | ISO timestamp |
| `limitations` | x86_64: `functional only under Rosetta 2; no native Intel performance/timing/CPU-feature claim` |

The three inputs decide the shipped bytes: `Cargo.lock` pins every crate,
`rust-toolchain.toml` selects the compiler channel, and `pnpm-lock.yaml` pins the
`node_modules` tree that `crates/zfb`'s `build.rs` embeds.

Required checks for both targets: `macho-binary-arch` and `macho-addon-arch`
(`lipo -archs`), `proc-translated`, `cli-version` (`-V` equals `zfb {v}`),
`cli-help`, `node-identity` (Node 24 or later with the target's `process.arch`)
and `css-addon-smoke` (`scripts/smoke-css-node.mjs` loads and runs the addon).

## Admission verifier

```sh
node scripts/admit-darwin-artifacts.mjs --dir <downloaded assets> --version <v> \
  --source-commit <sha> --source-tree <sha> --repo-root <checkout> [--json-out FILE]
```

Exit 0 admitted, 1 rejected, 2 usage or I/O error. stdout (and `--json-out`)
carries `{schema: "zfb-darwin-admission/v1", admitted, version, source, inputs,
targets: {triple: {arch_label, receipt, archive, archive_sha256_file, addon,
addon_sha256_file, binary}}, errors, warnings}`. Input digests are read from the
git objects at `--source-commit` in `--repo-root`, not from its working tree, and
the commit must have tree `--source-tree`.

For each target it rejects:

- any of the five assets missing or not a regular file;
- a checksum file that is not one GNU line naming its asset, or that disagrees with the bytes;
- an archive that is not gzip tar, has more or fewer than one entry, names anything but `zfb`, is not a regular file, or has mode other than `0755`;
- a binary that is not a thin 64-bit Mach-O executable of the target CPU (`0x0100000c` arm64, `0x01000007` x86_64), or an addon that is not a thin Mach-O dylib/bundle of that CPU; fat/universal images are rejected;
- a receipt with another schema, version, target, tag, commit or tree;
- receipt artifact names, digests or sizes that differ from the actual archive, extracted binary, addon or checksum files;
- toolchain or input digests that differ from the source commit's `Cargo.lock`, `rust-toolchain.toml` or `pnpm-lock.yaml`;
- a qualification that is not `passed`, lacks a required check, repeats a check, or has a check with a non-zero exit, a non-`pass` first result or a retry;
- `cli-version` output other than `zfb {v}` or `proc-translated` output other than the target's value;
- arm64 not labelled `native-arm64` with `translated=0`; x86_64 not labelled `rosetta-translated` with `translated=1`, a check not run under `arch -x86_64`, a missing Rosetta limitation, or any "native Intel" claim;
- a non-Apple-Silicon host (`hw_optional_arm64` not `1`).

Across the directory it rejects any extra Darwin-tagged file and any other JSON
file carrying this receipt schema. Release asset names are unique, so a second
candidate for a slot is deliberate ambiguity about which bytes are authoritative;
it is rejected rather than warned about. Non-Darwin assets of the Release are ignored.

## What Linux admission cannot prove

Hashes bind bytes; they do not authenticate the operator or prove execution. A
self-consistent invented receipt is indistinguishable from an honest one, as in
[`LOCAL-NATIVE-EVIDENCE.md`](../research/v3-island-size/LOCAL-NATIVE-EVIDENCE.md).
Linux cannot run either Mach-O, so the qualification results are the owner's
attested record. The x86_64 result is Rosetta-translated functional evidence only;
it makes no native Intel performance, timing or CPU-feature claim, and a bug from
real Intel hardware reopens the row.
