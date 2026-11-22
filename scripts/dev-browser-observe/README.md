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

`ZFB_DEV_TIMING` and `ZFB_DEV_PROVENANCE_TRACE` are allowed in `server.env`
only when the profile sets `diagnostic: true`. Outside a diagnostic profile,
the harness also strips either one when inherited from the server's
environment. `ZFB_DEV_PROVENANCE_TRACE` turns on the `[zfb-prov]` trace (see
below). The receipt records
every effective `ZFB_*` variable and any variable that was removed. Every
attempt record carries the server output lines printed from arming until the
attempt ends, each with milliseconds relative to `t0`; `server.log` holds all
lines. `profiles/tiny-internal-a0-diagnostic.example.json` compares an
in-place title edit with an atomic-rename one.

## Provenance trace (`[zfb-prov]`)

In a diagnostic profile with `ZFB_DEV_PROVENANCE_TRACE: "1"` in `server.env`,
zfb prints one `[zfb-prov] {json}` stderr line per event. Each line has `ev`,
`seq` (strictly increasing), `ms` and, inside a tick, `tick`. The harness keeps
these lines in their own buffer, so neither the 400-line `serverLog` cap nor the
general line cap can drop them. Any buffer loss before an attempt's mark is
recorded in `droppedBeforeMark` and `lossNote`.

Every attempt record in a diagnostic run carries `provenance`:

- `traceEnabled` (from the effective server env), `linesSeen`, and
  `traceMissing`, which is true when the gate is on but no line arrived since
  arming.
- `parseErrors`: malformed lines are kept here, never dropped. Also
  `seqViolations`, and `truncatedLists`, which lists every `x_truncated: n`
  cut at the source.
- `events`: the parsed events. `targetConsumer` is the page source file for
  `targetPath`, from the fixture's route table.
- `causalChain` and `chainComplete`.

The chain uses the trace's own ids only, never HTTP order or controller
labels. The deciding tick is the last `intake` tick that names a path the
attempt wrote, compared as absolute paths in raw and realpath forms. Renames
count both `from` and `to`. Each link is `observed` or `missing`:

1. `intake`: every matching intake, with its tick and the matched paths.
2. `select` for that tick: `selected`, `all_pages` with `all_reasons`,
   `fan_out_safe`, `noop`, and the changes for the written paths.
3. `generation`: `adopted` (refresh `refreshed`, or a worker), `unchanged`
   (refresh `skipped`), or `retained` (a `reject`, with its stage and the
   retained generation). The snapshot outcome is recorded too.
4. `render_plan` (whether the target page was planned) and `stale`.
5. `observations`: the first event after the adoption whose generation equals
   the adopted one. Recorded: its `first_reader`, and whether the target
   consumer read the written entry.
6. `edges`: the event for the same drain. Recorded: the target consumer's slot
   statuses (`replaced`, `retired`, `retained`, ...) and whether a group links
   the target consumer to the written entry.
7. `publish` of the target with that generation: lazy by `url`, eager by
   its per-URL output path. The eager `page` field is not used to match: zfb
   can report an output-keyed id there instead of the page source.
8. `dom`: the outcome the harness observed (`t1`, latency).

`observations` counts only a drain flagged `first_drain_of_generation: true`.
Drains flagged `false` are skipped and listed in `notFirstDrains`. If every
drain of the generation is flagged `false`, the link is `missing` with a
`not-first-drain` reason. A lazy `publish` links to its drain only when
`drain_contains_page` is `true`; one flagged `false` is kept under `unlinked`.
If an older binary omits either flag, the link is `unverified`: it is never
`observed` and the chain is not complete (`causalChain.unverified`). Per-path
`select` fields, such as a `global_removed` reason or `external_override`, and
any unknown extra fields pass through unchanged.

Contract 1e493c9bc adds these rules:

- **`first_drain_of_generation`:** `null` means no generation was known, so
  the link is `unverified`.
- **`graph_written`:** an `edges` group counts as a graph edge only when this
  is `true`. The group check is `groupLinksTargetToEntry`, and
  `groupNamesTargetAndEntry` records the group's contents regardless.
  Without the flag, the edges link is `unverified`.
- **Lazy `publish` page:** a lazy publish that names its `page` must name the
  target page and URL. With `page: null`, the URL alone decides.
- **`select` dirty pages:** `targetDirty` reads each written path's `dirty`,
  which is a page list or `"all"`.
- **Ordering:** downstream events are tied to the attempt by generation ids,
  not by `seq` adjacency. A `worker` line can be printed after the events it
  enabled. The only order used is that nothing for a tick comes before its
  intake.

Review hardening:

- **`edges`:** observed only when `applied: true` and `graph_written: true`.
  `applied: false` (`token_superseded`, `drain_failed`) is `failed` with its
  reason. If two edges events share a drain id, the failure wins.
  `graph_written: false` is `not-written`.
- **Eager `publish.output`:** relative to the dist root (`index.html`,
  `articles/a/index.html`). Both sides are normalised before matching.
- **Timing against `t1`:** the deciding intake is the last one at or before
  `t1`, and a publish after `t1` does not count. Expected-negative recovery
  writes are not part of the timed write set; they get their own
  `recoveryChain`.
- **Target evidence:** a chain whose events are all observed must also show
  that each link concerns the target. Otherwise it is `chainEventsOnly`, never
  complete, and `lackingTarget` names what is missing. The evidence per link
  is:
  - select: `targetDirty` or `all_pages`.
  - render plan: `targetPlanned`; in lazy mode the target's output must also
    be in `stale`.
  - observations: the target consumer read the written entry or the
    aggregate. Every article shares `[slug].tsx`, so a read of another entry
    is not evidence.
  - edges: a written graph edge links the target to the entry.
- **Render mode:** taken from the server's effective env, the way zfb dev
  resolves it: `ZFB_LAZY_DEV_RENDER`, then `ZFB_DEV_EAGER`, else lazy by
  default. It is recorded as `provenance.renderMode` with its source. A lazy
  tick renders the edited entries' own routes eagerly; those are published
  in-tick with mode `eager` and never appear in `stale.outputs`. So the lazy
  render-plan evidence is `targetPlanned` plus either the target's stale
  output or an in-tick publish of it with this generation. Eager mode needs
  only `targetPlanned`. An unknown mode gets the lazy rule.
- **Evidence drain:** observation and edge evidence come from a drain tied to
  the target's publish, recorded as `observations.evidence` and
  `edges.evidence`. For a lazy publish it is that publish's drain, which must
  have `drain_contains_page: true`. For an eager publish it is the first drain
  of the generation after this tick's `render_plan`, preferring a drain
  tagged with the same `tick` (`via` says which). A sibling article
  rendered through the shared `[slug].tsx` in another drain does not count.
  Aggregate reads count only for the written entry's collection.
- **Raw page strings:** `targetPageStrings` records the page strings as
  emitted, including same-basename strings that did not match, so a native
  run can confirm the absolute `PageId` form.
- **Observation reasons:** an unchanged generation (refresh skipped) gives
  `generation-not-adopted-this-tick`, and `first_drain_of_generation: null`
  gives `no-current-generation`.
- **Summary counts:** `summary.json` has `provenanceCounts` with
  `chainComplete`, `chainEventsOnly`, `incomplete` and `traceMissing`.

`chainComplete` requires every link except `stale`, which is recorded only.
When the deciding tick was rejected, only intake,
select and generation are required, and `rejected: true` is set.
`summary.json` lists each attempt's `chainComplete`, `missing` and
`traceMissing`.

Examples: `profiles/tiny-{internal,external}-a1-provenance-diagnostic.example.json`
cover A1, the original helper, with title and body edits under every warm
order. `profiles/field-notes-original-memoized-provenance-diagnostic.example.json`
is the Field Notes version.

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
  (`mode: "unverifiable-birth-pgid-scope"`). On Linux, procps `lstart` can
  read up to 1 s before the real start (it adds the process start to a boot
  time truncated to whole seconds), so the root pid alone (pid = recorded root
  pid = pgid) may show the previous second. Every other group member must
  still start at or after the spawn second, so an older stray member of a
  reused group never enters the scope. macOS gets no slack.
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
  start at or after the spawn second. A root row (pid = pgid) on Linux may
  start up to 1 s earlier, for the same procps `lstart` reason; no other
  row gets that slack.
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
| `edit-prop` | `description` + token (`prop`, only `description`) | detail `.lede`, or the home card description | home card, archive card, category card, related cards of the referrers |
| `rename-case-only` | `rename` to a case variant (`Article-000NN.md`) | declared in the profile (see below) | `expected-negative`: observe, rename back, require live restore |
| `css-in-rejected-tick` | managed CSS, then broken frontmatter (`set: "recovery"`) | CSS computed style on the open page after the repair | the article restored live |
| `css-in-rejected-discovery-tick` | managed CSS, then a new invalid article (`invalid`) | CSS computed style on the open page after the repair | the new article's body, then the lists |
| `edit-during-rejected` | break `article`, edit `other`'s title, repair | `other`'s title on its open detail page | `other` on home (or its archive card), `article` restored |

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

**`edit-prop`** records `declared` before the write: the field, the old and
new value, the timed predicate and every rendered output it will check.

**`rename-case-only`** renames `<slug>.md` to `toName` (default: first letter
flipped). `caseRenameSteps: "direct"` is one `rename`; `"via-temp"` renames
through a dot-temp sibling and records both steps. Before the attempt the
harness probes the work dir's case behaviour: it creates a lower-case probe,
stats the upper-case variant, compares dev and inode, and removes the probe.
Nothing is assumed from the platform. Each rename records `readdirBefore`,
`readdirAfter` and the inodes. When the entry name did not change, the outcome
is `rename-noop`, an error that stops the batch. The expectation is never
inferred. In `set: "expected-negative"` it must declare `expectState`. In a
workload scenario it must declare `targetPath` and `predicate`, and it must be
the scenario's last op. zfb-content derives the entry slug from the file path
with the extension stripped and the case kept, so `Article-00008.md` becomes the
slug `Article-00008`. Its frontmatter `slug` stays `article-00008`, and the
Field Notes helpers throw `Article slug mismatch`. Restoration renames the
variant back on the same inode, then checks the exact entry names, hashes and
live pages. Restoration never unlinks an extra file whose lower-cased name, or
whose dev and inode, matches a snapshot file. On a case-insensitive volume that
file can be the article's only inode. It is left in place for inspection, and
the cleanup reports `restoration-failed`.

**Recovery ops** run only in `set: "recovery"` and must declare
`expectState` for the rejected generation. The steps are:

1. Write CSS first, then content, as the Rust e2e does.
2. Wait, bounded by `rejectionWaitMs`, for a rejection line (`was not
   published`, or `[zfb] rebuild tick failed`) printed after the broken write.
   Without one the outcome is `rejection-not-observed` and nothing is repaired.
   Otherwise the repair could join the broken write in one valid tick. Then
   wait for a quiet window.
3. Classify the rejected state. For the CSS ops this reads the open page in
   place: markers and the last-good predicate. First it checks that the page
   is still the prepared document: the same docUUID, no new document, no
   main-frame navigation, and a settle that reached quiet. If not, the outcome
   is `open-page-replaced`. The document's original navigation status is
   recorded as `originalNavigationStatus` and is not used as evidence. For
   `edit-during-rejected` it observes the declared paths, then navigates back
   once.
4. Make any writes declared for the rejected state. After them the open page
   gets the same unchanged-document check.
5. Arm on the open page.
6. Repair: `t0` is this write.
7. Wait for the armed notify (`t1`), then run the declared checks.

The controller never reloads or navigates the open page between the arm and
`t1`. A predicate that is already true at the arm is `precondition-failed`,
unless the op declares `beforeRepair: "may-be-present"`; then no latency is
claimed.

The CSS ops parse the server lines from the first write to the rejection for
`[zfb-timing] intake paths:`. Lines from before the first write are dropped,
and only the last intake line before the rejection decides. This gives
`sameTick`:

- `true`: the last intake line names both paths.
- `false`: the paths were in different intakes. The outcome is
  `not-same-tick`, and the batch stops.
- `unknown`: there was no timing line, or the server log was truncated.

With `requireSameTick: true` (diagnostic profiles with
`server.env.ZFB_DEV_TIMING: "1"` only), `unknown` stops as
`same-tick-unknown`. Without it, an otherwise good `recovery-observed` is counted
in `recoveryUnprovenTick`, never in `recoveryPasses`. Every recovery record
carries `sameTickProven` (`true`, `false`, or `null` when the op has no tick
premise or it is unknown), and so does the `recovery` list in `summary.json`.
Passes are split into `recoverySameTickProvenPasses` and
`recoveryNoTickPremisePasses`. These outcomes are errors that stop the batch:
`rejection-not-observed`, `open-page-replaced`, `recovery-state-mismatch`,
`recovery-failed`, `not-same-tick` and `same-tick-unknown`. A timeout after the repair stays `timeout`.

**`fixture.placement`** (Field Notes): `external` is the default and copies
the source layout exactly. `internal` is a versioned derived overlay. The
articles are copied into `apps/zfb/content/articles` and the source `content/`
dir is not copied. `apps/zfb/zfb.config.ts` is replaced by
`fixtures/field-notes/overlays/internal-placement/zfb.config.ts.overlay`; the
only change is the collection `path` (`content/articles`, no
`allowOutsideRoot`). `manifest.json` `placements.internal` records the
provenance (`derived`), the base sha256 (`a68c2bdb…f915`), the overlay sha256
(`ddce03ff…161e`) and the exact diff. `verify-identity` and materialization
reject a different base or overlay. The source pins are still verified.
Materialization also refuses a source whose `datasetSha256` is not the
manifest's, and a materialized copy whose dataset hash differs.
Receipts record `placement`, `layout`, `configSha256`, `configBaseSha256` and
`materializedDatasetSha256`. Every op follows the materialized content dir.

After each scenario, restoration reverts renames with `rename`, removes
created files and restores bytes. It then live-checks the home page and
counts, the touched detail pages, every archive page and a category page.

`run --scenario id[,id]` runs selected families, so one classify stop does not
hide the others. The matrices are in `profiles/field-notes-*.example.json`.
For each adapter, `*-q.example.json` (external), `*-internal-q.example.json`
and `*-recovery-diagnostic-q.example.json` (`ZFB_DEV_TIMING=1`,
`requireSameTick`) cover the new families.

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

`profiles/tiny-{internal,external}-a{0,1,2,3}-body-category.example.json`
cover A0–A3 in both placements. Each warm order (`index-first`,
`detail-first`, `none`) has three cells: body on the detail page, category on
the detail page, and category on `/`. Each cell is its own scenario. A1
remains exactly the original helper.

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
