# dev-browser-observe v1

Measures whether a source-built `zfb dev` updates an open Chromium page after
a content file edit. The harness launches the server and one browser it owns.
Any run that cannot be trusted stops before the next workload edit.

`node scripts/dev-browser-observe/cli.mjs --help` lists every command and exit
code. `cli.mjs schema <profile|receipt|record|build-receipt|fixture>` prints a
JSON schema.

## Workflow

1. Build the binary, then record it:
   `cli.mjs record-build --exe <frozen>/zfb --vendor-dir <frozen>/vendor --target-dir <target> --profile dev --repo <checkout> --toolchain 1.99.1 --build-command "..." --out <dir>/build-receipt.json`.
   `--exe` is the binary runs will use. It may be a frozen copy outside the
   target dir; the receipt then also records the original build output
   (`--build-exe`, which defaults to `<target-dir>/[<triple>/]<profile-dir>/zfb`)
   and whether the two hashes match. `--vendor-dir` binds an explicit vendor
   copy. Without it, the newest `target/<profile>/build/zfb-*/out/vendor` whose
   build-script `output` names it in `ZFB_VENDOR_DIR` is used. Every target-dir
   candidate is recorded either way. The receipt also records:
   - the exe hash;
   - the source SHA plus the `git diff HEAD` and untracked-file identity;
   - the `Cargo.lock` hash and the toolchain (`rustc -vV`);
   - the target triple, profile, features, jobs, flags and target dir;
   - whether every embedded source file's bytes occur in the exe.
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

## Write methods

Every op declares `writeMethod`, and the profile declares
`restoration.writeMethod`. The method changes what the watcher reports, so it
is never implied. An atomic save produces a rename and an extra sibling event,
while an in-place write produces one `Modified`.

| Method | What happens | Ops |
| --- | --- | --- |
| `in-place` | `open(O_WRONLY\|O_TRUNC)` on the existing inode, never `O_CREAT` | content and CSS edits (default) |
| `atomic-rename` | temp sibling `.<name>.dbo-<hex>.tmp` in the same dir, then `rename` | content and CSS edits |
| `unlink` | `unlink` | `delete-article` |
| `create` | `open(O_CREAT\|O_EXCL)` | `create-article` |
| `rename` | `rename` to `<slug>-renamed.md` (or `to`) | `rename-article` |

Each attempt records the method, the inode before and after, and any temp
sibling. Restoration rewrites changed files with `restoration.writeMethod`
(`in-place` or `atomic-rename`), recreates missing files with `create`, and
removes extra files with `unlink`. Every restoration action is recorded with
its method.

## Diagnostic profiles

`ZFB_DEV_TIMING` is allowed in `server.env` only when the profile sets
`diagnostic: true`. Outside a diagnostic profile, the harness also strips an
inherited `ZFB_DEV_TIMING` from the server's environment. The receipt records
every effective `ZFB_*` variable and any variable that was removed. Every
attempt record carries the server output lines printed from arming until the
attempt ends, each with milliseconds relative to `t0`; `server.log` holds all
lines. `profiles/tiny-internal-a0-diagnostic.example.json` compares an
in-place title edit with an atomic-rename one.

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

## Round-2 controls

- **Livereload frames:** every attempt records the zfb livereload WebSocket
  frames (`/__zfb/reload/ws`, JSON `{ event, data }`) with `msFromT0`. A new
  document after a main-frame navigation with a `page` frame is
  `native-full-reload`; without that frame it is
  `full-reload-without-page-frame`. The same document with a `css` frame is
  `css-replacement`.
- **Pre-write stability:** a navigation, new document, or `page`/`css` frame
  between the target page's load and `t0` records `precondition-unstable` and
  stops, with no write.
- **Quiet window:** after each cleanup's live confirmation the harness holds
  `timeouts.quietMs` (default 1500) and records any navigations or frames.
- **Armed-wait sampling:** the harness samples owned PIDs every
  `limits.sampleEveryMs` (default 1000) during the armed wait. It uses a
  non-blocking `ps`, so the event loop that timestamps `t1` is not stalled. A
  ceiling trip records `resource-limit` and stops.
- **`expect: "classify"`:** a timeout is recorded as `timeout` and the batch
  still stops. The CLI exits 0 with `stoppedByClassifyTimeout`, and the
  timeout is never counted as success.
- **`warmOrder`** (scenario): `index-first` (`/` then the detail page),
  `detail-first` (the reverse), or `none` (only the target page). These are
  unmeasured controller navigations before the first `t0`, recorded as `warm`.
- **`fixture.trailingSlash`:** writes `trailingSlash: true` into the config;
  routes and predicates use `/articles/<slug>/`.
- **Calibration:** `reload-survival-controller-triggered` uses
  `location.reload()`. `native-reload-survival` makes a real, unmeasured
  in-place body edit of `b.md` while armed on its page. It requires a new
  document, `navigationType` `reload` and a livereload `page` frame, then
  restores and confirms the original state live. With a memoizing adapter
  (A1) that body edit may legitimately not render, so calibrate with A0.

## Round-3 controls

- **Delivery ack:** the observer persists `delivered` only after the
  controller's binding acknowledges the notification. If a document is replaced
  mid-delivery, the expectation stays armed and the next document re-delivers.
  The controller records repeats of an already-resolved token as `duplicates`,
  never as late or success.
- **Cleanup settle:** before live checks, cleanup waits until there has been no
  livereload frame, main-frame navigation or new document for `quietMs`,
  counted from the restore write or the last such event and bounded by
  `restoreMs`. It records `settle` with `nativeUpdateObserved`.
- **Live-check recovery:** when a native reload aborts a live-check navigation
  (`ERR_ABORTED`), the harness waits for the resulting document. If that
  document is not the target, it navigates once more, never reloading. The
  attempt is recorded in `recovery`, and a second abort fails the check. A
  failed check records `firstReadActual` and a read-only `failure` snapshot:
  URL, title, `readyState`, armed state, the predicate now, and a body excerpt.
- **Negative-control injections** (`inject`, only in `set: "negative-controls"`;
  `profiles/neg-*.example.json`). Each one stops the batch. A run stopped only
  by declared controls exits 0.

  | Inject | Declared outcome |
  | --- | --- |
  | `kill-server-during-wait` | after the write, SIGKILL the birth-verified native process; `server-exit` with `exitCode: null` and `signalCode: "SIGKILL"`, no restart |
  | `restore-blocked` | `chmod 0444` on the target before restoration; `restorationFailures: 1`; the mode is reverted in a recorded `inject` step |
  | `write-error` | the write targets a never-existing sibling, so `in-place` refuses it; `write-error` before any DOM wait (no watcher event is caused) |
  | `precondition-true` | an always-true predicate; `precondition-failed`, with no write proven by equal `targetSha256Before` and `targetSha256After` |

- **Lock contention:** run `cli.mjs lock-contention-check --profile p.json`. A
  `hold-lock` child holds the profile's lock while a second `run` must exit 5
  without creating its label dir. The command prints JSON and exits 0 when that
  holds.

## Round-4 hardening

- **Endpoint validity:** a delivery for the armed token that arrives before the
  write is recorded as `precondition-unstable` (reason
  `notified-before-write`) and no write happens. A notify with `t1 <= t0` is
  rejected the same way (reason `notify-not-after-t0`). Neither is ever a
  success.
- **Always clean up:** a throwing page or observer call inside the batch
  records `controller-error`, then cleanup and `batch-end` still run, and
  `summary.json` uses the live batch accounting. Materialize failures are
  recorded as `materialize-error`, with receipt, summary and `run-end` still
  written.
- **Exit disposition:** a batch exits 0 only if it never stopped, or stopped
  solely on declared controls with no undeclared error, timeout or stale. A
  `restore-blocked` cleanup is credited only when nothing else had already
  stopped the batch.
- **Waits:** `disarm()` cancels a pending measured wait, so a later delivery
  can never be routed to an earlier attempt's waiter.
- **Descendants:** every newly seen owned process is appended to the registry
  (`type: "owned"`), and the next-batch guard checks those entries too. Once
  the root has died, stop also scans for the root's recorded process group,
  accepting only members that started no earlier than the root. If the group
  number has been taken over by a different leader, nothing is trusted.
- **Profile constraints:** `restore-blocked` requires
  `restoration.writeMethod: "in-place"`, and `write-error` requires
  `in-place` or `atomic-rename`.

## Round-5 hardening

- **Server output:** server output is read line by line, and readiness matches
  only complete lines, so a URL split across chunks is never cut short. Once
  ready, the harness stops scanning for readiness. It keeps at most 20,000
  lines in memory, and `logSince` reports any `droppedBeforeMark`.
- **`server.log`:** writes after close are counted and dropped, and stream
  errors are recorded instead of thrown. `closeLog()` first waits, bounded, for
  the child's pipes to close.
- **Stop must verify absence:** absence requires the root's own exit event. If
  absence is unverified or the stop throws, the exit is 6 and `summary.json`
  records it as `stopFailure`.
- **Unverifiable birth:** the birth identity is re-read after spawn with
  bounded retries. If it is still unreadable it is recorded as
  `birthUnverifiable`. Stop then signals, individually, only members of the
  process group that started at or after the spawn second
  (`mode: "unverifiable-birth-pgid-scope"`).
- **Bounded waits:** armed-wait sampling covers only birth-matching live owned
  PIDs. Calibration pings time out instead of hanging. `hold-lock --ms`
  releases stdin and exits.
- **Lock errors:** exit 5 means lock contention only; any other lock failure
  is a controller error (exit 8).
- **After-stop cleanup:** once another outcome has stopped the batch,
  after-stop cleanup does not apply the `restore-blocked` chmod, and records
  the skip.

## Round-6 controls

- **Uncredited injections:** a declared `inject` that does not produce its
  declared outcome stops the batch with `negative-control-not-fired`. It records
  `control-not-fired` and gives a non-ok exit. Examples: a kill refused by the
  birth check, a notify that wins the race, a block that did not take, or a
  write that unexpectedly succeeded. The batch never continues.
- **`restore-blocked` timing:** the blocked restoration runs immediately after
  the injected op, so no later op touches the blocked file. After `revert-mode`,
  the harness retries the restoration once without the block
  (`retry: true`), so the work tree is not left mutated. The blocked attempt's
  `restorationFailures` stays counted, and a failed retry is counted on top of
  it.
- **Arm errors:** a throwing `page.arm()` becomes an attempt with outcome
  `arm-error`.
- **Calibration live checks:** a throwing live check inside the
  `native-reload-survival` calibration cell is recorded as a failed check, so
  earlier cells' results are kept.
- **`hold-lock`:** `--ms` requires a finite, non-negative number.

## Round-7 hardening

- **Unverifiable birth:** a root whose birth was unverifiable at spawn is
  registered as an `owned` row once a snapshot sees its real birth. Registry
  rows marked `birthUnverifiable` match the next-batch guard by pid + pgid +
  start at or after the spawn second.
- **Shutdown order:** the server is stopped first. The browser is then closed
  with `context.close()` and `browser.close()` each bounded by 10 s; a timeout
  is recorded. Record writes in shutdown are best-effort and reported on
  stderr, so a failing write cannot skip the stop, the summary or the receipt.
- **Calibration bound:** the `observer-init-after-navigation` cell is bounded
  by `timeouts.attemptMs`.
- **Profile and CLI validation:** `server.host` must be an explicit address,
  because `0.0.0.0` and `::` print a multi-line banner. `hold-lock --ms`
  accepts 0–2147483647. A label created concurrently exits 2.
- **Stream decoding:** output is decoded with a per-stream UTF-8 decoder
  before it is split into lines.

## Field Notes fixture (`kind: "field-notes"`)

`fixture.kind: "field-notes"` runs the original 100-article Field Notes ZFB
app. That app is not in this repo. `fixture.source.dir` points at the
parent-provided source dir, and the profile pins its tree hashes: `apps/zfb`
without `lib/content.ts`, `content/`, `shared/`, and the generator's
`datasetSha256`. `cli.mjs pins` prints all of them, and `verify-identity`
rejects any drift.

The adapters (`original-memoized`, `recovered-snapshot` and `direct-read`)
replace `apps/zfb/lib/content.ts`. Their provenance is recorded in
`fixtures/field-notes/manifest.json`. `managedCss: true` adds a declared
`styles/global.css` with a custom property.

| Op | What it writes | Timed predicate | Supplemental (after the endpoint) |
| --- | --- | --- | --- |
| `edit-body` | token in the first paragraph | detail body | none |
| `edit-shared-component` | footer text in `Layout.tsx` | footer on the active page | `/archive/1/` footer |
| `edit-managed-css` | `--fieldnotes-managed-marker` | computed style | none |
| `edit-public-css` | `--fn-public-marker` in `public/site.css` (`expect: classify`) | computed style | none |
| `edit-title` | title | detail `h1`, or the home card | home card, category card, archive card, related cards of the referrers |
| `edit-category` | `category` + `categoryLabel` | detail eyebrow | old and new category membership, card meta, recomputed related lists |
| `add-article` | `create` of the next article | hero count | new URL body, archive page, category membership |
| `delete-article` | `unlink` | hero count | old URL gone (404/410 and not still rendered), lists, related lists |
| `rename-slug-change` | `rename`, then an in-place `slug` update | new card on home/archive | old URL gone, new URL body, category membership |
| `rename-filename-only` | `rename` (slug kept) | `expected-negative` | observe error/last-good, then rename back and require live restore |
| `edit-invalid-frontmatter` | broken YAML | `expected-negative` | observe error/last-good, then restore bytes and require live restore |
| `hidden-edit` | body of an unvisited article | none while elsewhere; see below | none |
| `burst` | `count` titles at `spacingMs` | latest token only | none |
| `rapid-sequence` | title then category, or add then delete | both, or the final state after settle | as `edit-category`, or the final-state checks |

Notes on the special ops:

- **`hidden-edit`** records the active page's reloads, frames and document
  changes for `observeMs`, then waits `accessDelayMs`. That wait is not update
  latency. It then navigates and records `firstDisplayAfterNavigationMs`, plus
  every earlier request for the article's URL (`prefetchRequests`).
- **`burst`** reports `latencyFromFirstWriteMs` and `latencyFromLastWriteMs`,
  and `watchSeen` lists any intermediate token the observer saw.
- **Expected-negative ops** belong only in scenarios with
  `set: "expected-negative"`. Each one counts as `expected-negative-observed`
  only when every observed page is in its declared `expectState` and the
  recovery restores live behaviour. Anything else is
  `expected-negative-mismatch` or `recovery-failed`: an error and a stop.

After each scenario, restoration reverts renames with `rename`, removes
created files and restores bytes. It then live-checks the home page and
counts, the touched detail pages, every archive page and a category page.

`run --scenario id[,id]` runs selected families, so one classify stop does not
hide the others. The matrices are in `profiles/field-notes-*.example.json`.

## Restoration and processes

Exact source bytes are snapshotted before each scenario and restored after it
and after any failure. Restoration is verified by hash and, while the server
is alive, by the original predicates reappearing in the page. Restoration is
labelled `cleanup` and is never a measured trial.

The server is spawned detached in its own process group, with `launcher: none`.
Its birth identity (pid, ppid, pgid, `lstart`, comm, args) is re-verified
before any signal. To stop it, the harness sends SIGINT to the group (zfb's
graceful path), then SIGTERM after `stopTermMs`, then SIGKILL after
`stopKillMs`, and then verifies the processes are gone. `endings` records the
signal after which each process disappeared. `outDir` must be outside every
git work tree.

## Fixture `tiny-two-article`

The fixture is a zfb app with no `node_modules`. It has two Markdown articles,
`a` and `b`, with `title` and `category` frontmatter. Routes are `/`,
`/articles/<slug>` and `/independent`, each with a trailing slash when
`fixture.trailingSlash` is set. Two content placements are selectable:
`internal`, or `external`, which is `../content/articles` with
`allowOutsideRoot`, as in the original reproducer.

`fixture.configFormat` selects the config file. `json` (the default) writes a
generated `zfb.config.json`. `ts` writes `zfb.config.ts`, which is the
byte-exact original (`configs/original.zfb.config.ts.src`, sha256
`7839c840…f602`) when the profile matches its shape: `external` placement,
`trailingSlash: true` and `wind: false`. `verify-identity` checks that file's
hash. Any other `ts` combination gets a generated `defineConfig` file labelled
`RECONSTRUCTED`. The receipt records `configFormat`, `configFile`,
`configProvenance`, `configSha256` and `configOriginalSha256`.

Helper adapters live in `fixtures/tiny-two-article/adapters/`; see the
adapter README. `A0` is a reconstructed direct-read helper. `A1` is the exact
original memoized helper, and `verify-identity` checks its hash. `A2` is A1
plus a snapshot-touch line. `A3` is a reconstructed diagnostic that rebuilds
when the snapshot slot changes. The home page, the detail render and the
detail route's `paths()` all call the same `getIndex()` from `lib/content.ts`. The managed CSS is `app/styles/global.css`,
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
