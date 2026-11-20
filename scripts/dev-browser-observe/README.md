# dev-browser-observe v1

Measures whether a source-built `zfb dev` updates an open Chromium page after
a content file edit. The harness launches the server and one browser it owns.
Any run that cannot be trusted stops before the next workload edit.

`node scripts/dev-browser-observe/cli.mjs --help` lists every command and exit
code. `cli.mjs schema <profile|receipt|record|build-receipt|fixture>` prints a
JSON schema.

## Workflow

1. Build the binary, then record it:
   `cli.mjs record-build --exe target/debug/zfb --profile dev --out <dir>/build-receipt.json [--toolchain X] [--build-command "..."]`.
   The receipt records the exe hash, the source SHA plus the `git diff HEAD`
   and untracked-file identity, the `Cargo.lock` hash and the toolchain
   (`rustc -vV`). It also records the target triple, profile, features, jobs,
   flags and target dir. For the vendor tree, it records every
   `target/<profile>/build/zfb-*/out/vendor` candidate. It chooses the one whose
   build-script `output` names it in `ZFB_VENDOR_DIR` and is newest, then
   hashes it. It also checks that every embedded source file's bytes occur in
   the exe.
2. Copy an example from `profiles/` and fill in `productSha`, `buildReceipt` and
   `outDir`. Paste the output of `cli.mjs pins --profile p.json` into `pins`. Put
   `outDir` outside any checkout, because a `node_modules/@takazudo/zfb*` in the
   work dir or any ancestor is rejected. The receipt lists every reachable
   ancestor `node_modules`.
3. `cli.mjs verify-identity --profile p.json` checks identity only. It starts no
   server and makes no edits.
4. `cli.mjs calibrate --profile p.json --label cal-1`, then
   `cli.mjs run --profile p.json --label run-1`. A label is never reused, and
   recovery from a failure needs a new label.

## What `run` refuses

`run` refuses before any workload edit and records `identity-rejected` when:

- the exe hash differs from the receipt;
- the receipt SHA or dirty identity differs from the profile's declaration;
- the vendor tree differs from the source packages at that SHA, using the
  `copy_ts_src` filter in `crates/zfb/build.rs`;
- an exe is missing embedded bytes;
- a local `@takazudo/zfb*` package resolves;
- a harness, fixture, observer or adapter pin changed.

It also refuses while another batch holds the lock (exit 5) or while an owned
server from an earlier batch is still alive (exit 6). The lock is a
`flock(LOCK_EX|LOCK_NB)` that a perl child holds. Deleting the lock file
releases nothing.

## Measurement

- **Clock:** one controller clock, `process.hrtime.bigint()`. `t0` is taken
  immediately before the `writeFile`, `rename` or `unlink` syscall. `t1` is
  taken when the Playwright binding arrives. The endpoint is
  **predicate confirmed + 2 rAF + binding delivery**, and it makes no paint
  claim. Browser clocks are never subtracted.
- **Observer:** `observer-init.js` is installed with `addInitScript`, so it
  survives native reloads. It keeps the armed expectation in `sessionStorage`.
  Its predicates are `text`, `attribute`, `count`, `absent`, `computed-style`
  (custom properties included) and `document-uuid-not`. A predicate that is
  already true before the write is `precondition-failed`. A delivery for any
  token other than the armed one is recorded as `late-token` and never counts
  as success.
- **Mechanism:** classified from evidence. A document UUID change together with
  a main-frame navigation is `native-full-reload`. The same document is
  `in-document-update`. A server PID change is `server-restart`.
- **No manufactured success:** during a measured attempt the controller never
  reloads, navigates or polls over HTTP. Supplemental checks, such as the home
  listing after a title edit, run after the timed endpoint as a separate
  navigation. They follow one rule: navigate once, then wait on an armed
  expectation, with no reload.

## Fail-stop and accounting

The batch stops after any of these outcomes: `timeout`; `stale` (a reload
happened but the predicate stayed false); `precondition-failed`; `write-error`;
`server-exit` (with `exitCode` and `signalCode`); `resource-limit` (RSS or wall
ceiling); `supplemental-failed`; or `restoration-failed`. The outcome record is
appended and fsynced first, and the batch stops before another workload edit.

`summary.json` reports planned, attempted, success, stale, timeout, error,
notAttempted and lateTokens, plus `lastArmedUnknownOperation`. Disarming the
observer does not cancel native work. Latency statistics (n, min, median,
nearest-rank p95, max) are conditional on success.

A `negative-controls` scenario declares `expect: "timeout"`, for example a
`stale-predicate` op. A timeout there is a control pass and still stops the
batch.

## Restoration and processes

Exact source bytes are snapshotted before each scenario and restored after it
and after any failure. Restoration is verified by hash and, while the server
is alive, by the original predicates reappearing in the page. Restoration is
labelled `cleanup` and is never a measured trial.

The server is spawned detached in its own process group, with `launcher: none`.
Its birth identity (pid, ppid, pgid, `lstart`, comm, args) is re-verified
before any signal. To stop it, the harness sends SIGTERM to the group, waits a
bounded time, sends SIGKILL to verified survivors, and then verifies they are
gone.

## Fixture `tiny-two-article`

The fixture is a zfb app with no `node_modules`. It has two Markdown articles,
`a` and `b`, with `title` and `category` frontmatter. Routes are `/`,
`/articles/<slug>` and `/independent`. Two content placements are selectable:
`internal`, or `external` with `allowOutsideRoot`.

Helper adapters live in `fixtures/tiny-two-article/adapters/`. `A0` is a
reconstructed direct-read helper. The managed CSS is `app/styles/global.css`,
which zfb links into every page; its custom property is
`--observe-css-marker`. A plain `.css` import from a page is not supported
because zfb does not emit those bytes.

## Not implemented

`mode: "fixed-distribution"` is accepted by the schema but rejected with
`not-implemented` (exit 4).

## macOS process sampling

`cli.mjs probe-mac` prints which `ps` fields work on the host. The fields are
`rss`/`vsz` in KiB, threads (the `ps -M` line count), `lstart`, and the
`sysctl.proc_translated` flag. A field that does not work is recorded as
`unavailable`.

- A sampled peak is not a lifetime high-water mark.
- Summed RSS is not unique memory.
- A SIGKILL exit is never read as OOM or a leak.
