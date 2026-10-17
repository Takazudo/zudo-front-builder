# Main v4.5.3 follow-up — native macOS arm64, 2026-10-16

Measured source: merge `cc3494135de356a56386b604c5117c67aa1c6df2`. It merges
main `d70dd6d94ab1b89a65fee9ed7727747d1de860f2` (v4.5.3: #4282, #4284, #4286
and the release bump) into the previous head `503e4e2d`. The tree was clean
before every check below. No held #4234, #4259 or #4260 commit was imported.
This directory is evidence, not activation, phase-two CLI adoption or native
size measurement.

Host: native Darwin arm64 (macOS 26.6.1, not Rosetta), rustup Rust/Cargo
1.99.0 (`b940084d7`), official Node v24.14.0 (binary digest equal to the
nodejs.org darwin-arm64 tarball), Python 3.12.13, pnpm 12.8.2. Heavy builds and
gates ran one at a time under the machine heavy guard. `logs/host-tools.txt`
records the OS, the official Node tarball and binary digests against nodejs.org
`SHASUMS256.txt`, the rustc/cargo identity, the no-V8 Wind compiler digest and
the esbuild digest. The earlier Linux
receipts in `refresh-linux-2026-10-16/` were produced on Linux x86_64 with Rust
1.99.1. This run neither replaces nor qualifies them; it is separate Darwin
evidence.

## Merge and Wind

The single conflict was `docs/public/wind-examples/manifest.json`. The real
combined-source no-V8 compiler (`cargo build --locked -p zfb
--no-default-features`, binary `b9a30f872e69a25bccbd9da01f1d029a2a9de31958f16b20d877f269024c0843`)
regenerated all 173 samples and then passed `--check`. Only
`compiler.sourceSha256` changed, to
`69648324c3b7b0b515134b4938803c2377f98db3c9a080ca4a1d18df71b194e9`; example
rows and assets are byte-identical. Wind generator tests 19/19 and reference
tests 22/22 passed.

Main's `pnpm-lock.yaml` (`370bf324…`), package versions (4.5.3) and both
island-size contracts (`decision.json` `b8b990cb…`, `decision-linux-x64.json`
`08abd3ff…`) are byte-for-byte main. `Cargo.lock` (`230f0acb…`) is main's
unchanged lock plus only the candidate `zfb-js-host` record and the
`zfb-render` edge rewire (`sha1`/`sha2` → `zfb-js-host`).
`cargo metadata --locked --offline` resolves it.

## Acceptance (`acceptance.json`)

`accept.py --generated --guard …` passed on its first complete attempt with
explicit esbuild 0.25.12 (`3e030ee2…`, the zfb build's staged binary), Chrome
for Testing 149.0.7827.55, official Node and rustup `rustc` 1.99.0. Results:
45 native and 9 shared-service tests; 63 config, 27 service and 47
module/worker rows per engine; 8 HTTP and 7 outer-config rows; 8 freshly
generated packets per engine; and 7/7 exact parity rows. Chromium observed
`Count: 0` → `Count: 1` and `rgb(12, 34, 56)`, with no page or console errors.
Both engines map the authored throw to line 6, column 9.

All six executables were re-hashed after the gates and were unchanged. The
frozen snapshot (`generated-manifest.json`, digest
`7a0ec90aa8b98b0109e4292fd3a01b0c7cfa8d7caf0f4205793027d096c4703f`) fed both
engines in this run. It differs from the Linux run's `94e0cb7d…`. The
differences are host absolute paths (including those embedded in
`bundle.mjs.map` and the wrapper), the source commit, the 4.5.3 package
manifest and pnpm lock, and tool version logs. The entry point's fixed
limitation string still says "Linux x86_64 native evidence only". The recorded
`environment` (macOS, arm64) is authoritative for this run.

## Affected regressions (`logs/`)

- Deno-backed renderer: `cargo test --locked -p zfb-render --lib` plus seven
  integration tests, **296 passed**, the same set as the Linux receipt.
- No-V8 `cargo check --locked --no-default-features -p zfb --tests`: passed.
- `zudo-wind --lib` 117, `zfb-content mdx_jsx_emit_hast` 52, and
  `zfb-css package_exports_imports` 22: all passed.
- Python checker suite, 95 tests: passed with `TMPDIR` on a non-symlinked
  directory (`python-checker-resolved-tmpdir.log`). With the default macOS
  `TMPDIR`, the suite reports 42 errors (`python-checker.log`):
  - 41 are `SystemExit: 2` (`--output must be a new directory below … target/`).
    The tests mock `ROOT` with an unresolved `/var/folders/…` path, while
    `accept.py` compares it with a resolved `/private/var/…` output path. That
    is a harness portability gap on Darwin, not a behavior change from this
    merge.
  - 1 is `KeyError: 'stdout_base64'` in
    `test_real_watchdog_preserves_partial_bytes_and_failed_outer_rows`. The
    report omits that key unless non-UTF-8 bytes arrive before the 0.3 s
    watchdog fires, so this is a timing race. It passed in the second run, but
    a pass on rerun is a flaky-test triage signal, not a clean pass.

  Both are recorded, and no test was modified.

## Producer closures (Cargo metadata, not native measurement)

`closure-{old,new}-{x86_64-unknown-linux-gnu,aarch64-apple-darwin}.json` were
recorded with held #4259 tool `9912ad75` from a separate detached checkout,
which was not imported. Roots: main `d70dd6d9` (old) and `cc349413` (new).
Results: Linux 497→498 and Darwin 481→482. On both targets, the only delta is
the added `zfb-js-host 0.0.0 (path)` with features `[default, services]`. All
four row sets are identical to the committed Linux-host receipts in
`refresh-linux-2026-10-16/`. These are workspace-resolved metadata proofs. They
are not Darwin native size evidence, and no declaration, staging or admission
was performed.

## Failures retained

- `compiler-build-attempt1-wrong-toolchain.log`: Homebrew `cargo` ignored
  `RUSTUP_TOOLCHAIN` and selected rustc 1.94; the retry used the rustup
  1.99.0 binaries explicitly.
- `*-attempt1-guard-not-invoked.log`: the guard was never executed because the
  shell did not split the command variable. Those checks did not run, and the
  later logs are their only runs.
- `python-checker-attempt1-py39.log`: the system Python 3.9 lacks
  `hashlib.file_digest`.
- `island-size-budget.log`: **2 of 32 still fail**, the same inherited
  assertions as before. They are the reviewed Darwin and Linux zero-allowance
  checks under `--reason cargo-producer-lock`. The contracts pin `Cargo.lock`
  `13956cf2…`, while this candidate checkout and any artifact measured from it
  carry `230f0acb…`. This is a provenance gate for the internal rewire (#4259),
  not a regression from the main merge. No ceiling, allowance, contract or
  script was changed.

`copy-integrity.json` lists the SHA-256 of every file here as copied, before
any repository hook ran. Remaining gates are unchanged: #4234 both-platform
control and replay, #4234 then #4259 integration, independent #4260
integration, refresh against the integrated tooling, compatible Linux/Darwin
saved-byte measurements, paired staging and review, separately authorized
apply, and final enforced verification.
