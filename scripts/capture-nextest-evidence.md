# Capture one nextest command's original evidence

`capture-nextest-evidence.py` wraps one `cargo nextest run` and archives its original
JUnit bytes before returning. Give every command a new evidence directory, outside
the source checkout (required). It requires Python 3.11+, Git, Cargo, and nextest
**0.9.140** on Linux or macOS. Other nextest versions fail before execution until
their configuration semantics have been audited. This helper does not run or
qualify anything on the owner's Mac by itself.

```sh
python3 scripts/capture-nextest-evidence.py \
  --output /absolute/evidence/watcher-command-01 \
  --profile ci \
  --target-dir /absolute/build-cache \
  -- -p zfb-watcher -E 'test(recursive_dirs)'
```

Use the selected immutable qualification checkout, its normal command-scoped
build settings, and the existing heavy guard around the wrapper when the command
requires it. Run commands serially. Do not run another unwrapped nextest writer
against the same report destination while capturing. The wrapper locks out other
instances of itself, but cannot coordinate an unwrapped process. Build/test
arguments after `--` are passed unchanged; it adds no retry or timing changes.
Compilation-only `--no-run` is rejected before execution: compile separately and
record that result as tests not run.

## Destination and provenance

The helper asks Cargo metadata for the actual workspace and build target directory.
Nextest's store is independently resolved as
`workspace_root / store.dir / profile / junit.path`; either configured path can be
absolute. `store.dir` defaults to `target/nextest`. `CARGO_TARGET_DIR` and Cargo's
`--target-dir` do **not** move that default store. The receipt records both locations.

Pass `--profile`, `--config-file`, `--tool-config-file TOOL:ABSOLUTE_PATH`,
`--manifest-path`, and `--target-dir` before the separator. The helper reproduces
0.9.140's built-in defaults, tool configuration precedence (first tool wins),
workspace or explicit configuration, profile inheritance, and `NEXTEST_PROFILE`.
JUnit must be enabled. Miri and archive/reused-metadata/remapped invocations are
unsupported and fail closed. Cargo `--config` overrides are also unsupported.

The resolver is audited against nextest tag `cargo-nextest-0.9.140`, commit
`a9fef2964e34f64ed4fceeee7c0c3559ce560920`, specifically
[store resolution and configuration merging](https://github.com/nextest-rs/nextest/blob/a9fef2964e34f64ed4fceeee7c0c3559ce560920/nextest-runner/src/config/core/imp.rs),
[JUnit paths](https://github.com/nextest-rs/nextest/blob/a9fef2964e34f64ed4fceeee7c0c3559ce560920/nextest-runner/src/config/elements/junit.rs),
and [default configuration](https://github.com/nextest-rs/nextest/blob/a9fef2964e34f64ed4fceeee7c0c3559ce560920/nextest-runner/default-config.toml).
That version's `show-config` does not expose an effective JUnit destination.

Each output directory contains:

- `receipt.json`: command argv, working directory, source HEAD and dirty status,
  Git-visible source fingerprints, nextest version, configuration paths/hashes, resolved paths, timestamps, child
  exit, wrapper exit, and evidence status/hash/run UUID.
- `stdout.log` and `stderr.log`: original command output. Output is captured to
  files; tail them in another terminal if live progress is needed.
- `junit.xml`: accepted original bytes, never reconstructed or summarized.
- `preexisting.xml`, if applicable: the prior report preserved before removing it
  from the shared destination. It is **not** evidence for this command.
- `candidate.xml`, if applicable: rejected newly found bytes retained for diagnosis.

Capture requires a new regular report file, a current file timestamp, nextest's
root UUID and run timestamp, and bytes different from the preceding report. Run
timestamps allow the producer's millisecond precision. Source HEAD, Git-visible tracked/untracked file bytes and symlink targets, and
configuration hashes are checked again after execution. Ignored build products are
excluded from the source fingerprint, as are the exact report, lock, and marker
paths owned by this capture. A collision with a tracked source path is refused. A dirty checkout is explicitly recorded;
a SHA alone does not certify its contents. Keep qualification checkouts clean and
exclusive. This is a local evidence aid, not protection against concurrent source
editing, clock changes, ignored source inputs, or a deliberately forged report.

## Results are separate from evidence completeness

A nonzero child exit is returned unchanged (signal termination uses shell status
128 + signal), whether XML was captured or missing. A successful child with missing,
stale, invalid, or source/configuration-mismatched evidence makes the wrapper exit `4`.
Preflight failures exit `2` and record `child_exit: null`, `execution: not_started`.

The destination is checked before anything is created. An existing path, including a
dangling symlink, is refused and left unchanged. Cargo metadata and the nextest
configuration identify the workspace and report path first; a destination inside the
workspace (also through a symlinked parent), equal to the report path or its capture
lock/marker, an ancestor of them, or below them is refused without creating any
directory and without starting the command. Those refusals, and failures that leave
the workspace or report path unknown (Cargo metadata, Git, or configuration errors),
write no receipt and say so on stderr. Later preflight failures (forwarded options,
Miri, nextest version, disabled JUnit) create the validated directory and record a receipt.

The receipt is written to a temporary name and renamed into place, so `receipt.json`
exists only when complete. If it cannot be written, stderr prints `receipt not written`
and the exit status still follows the rules above: `2` before the command starts, the
child's own status (or 128 + signal) when it failed, and `4` when it succeeded, because
the evidence is then incomplete. Logs and accepted `junit.xml` already saved are kept. Always inspect
both the child result and evidence status. Captured XML does not mean tests passed.
The helper does not parse counts: ignored and `no_run` entries retain their original
bytes and meanings. No tests are inferred from logs or rewritten as successes.

This prevents a future shared-destination overwrite; it does not recover the
missing watcher XML from the #4309 native receipt or alter that receipt's history.

## Offline checks

```sh
sh tests/unit/capture-nextest-evidence.sh
```

Temporary Git workspaces and a fake Cargo executable cover custom targets,
configured stores/absolute paths, config precedence, inheritance, sequential
replacement, missing/stale/replayed/malformed XML, unchanged ignored/no_run bytes,
source/configuration changes, and nonzero children. No Rust/native suite is run. The shell
entry is picked up by the existing offline-unit glob, without workflow changes.

```sh
sh tests/unit/capture-nextest-evidence-failure-handling.sh
```

The same fake-Cargo approach covers refused destinations (workspace, symlinked
parent, dangling symlink, report path collisions, existing paths) and receipt write
failures at preflight, after a successful child, after a nonzero child, and after a
signal. The fake Cargo turns the receipt path into a directory, so the injected failure
does not depend on file permissions and also fails for root.
