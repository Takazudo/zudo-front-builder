# #4166 result: the EN compatibility freeze is a view-transition Layerize of table scrollers, fixed with paint containment

Profiler: `tests/docs-navigation-structure/nav-stall-profile.mjs` (see `NAV-STALL.md` there). All runs used Chromium headless on macOS 26.6.1 arm64 with `prefers-reduced-motion: reduce`. Each run had 1 warmup round plus 4 measured interleaved rounds.

Both hosts served **one built docs tree** from a detached scratch worktree, and `preflight` confirms `servedMatchesDist`. The static host was `tests/docs-wind-preview/serve-dist.mjs`; the actual Cloudflare local host was `wrangler dev --local`.

## Files

| File | Tree | What changed |
| --- | --- | --- |
| `baseline.json` | `252ae14a` (published zfb 4.3.3 / zudo-doc 6.1.1 docs host) | nothing (both hosts, all cases) |
| `e1-vt-disabled.json` | same tree | view transitions disabled (static, EN/JA desktop) |
| `e2-wrapper-visible.json` | same tree | injected `.overflow-x-auto{overflow-x:visible}` |
| `e3-contain-paint.json` | same tree | injected `.overflow-x-auto{contain:paint}` (adds EN mobile) |
| `e4-cell-wrap.json` | same tree | injected `td,th{overflow-wrap:anywhere}` inside the wrappers |
| `e5-content-visibility.json` | same tree | injected `.overflow-x-auto{content-visibility:auto}` |
| `fix-bf036811.json` | `bf036811` (the committed fix) | none injected; both hosts, all cases, compared to the baseline |

The committed files are compact: identity, settings, preflight, census-free stats, the comparison, and every round's per-navigation values. The full JSON (with census, events and long-task lists) and the gzipped CDP traces (about 17 MB per run) stayed in the manager's local scratch. They were not uploaded, so this summary is the durable evidence.

## Observation 1 (EN desktop Layerize) reproduces, and the cause is isolated

Baseline, EN desktop 1440×1000, entering and leaving `/docs/zudo-wind/compatibility/`: **4/4 rounds froze on both hosts.**

| Host | Direction | Longest task, median / max | Layerize, median / max |
| --- | --- | --- | --- |
| static | enter | 4,799 / 5,308 ms | 4,669 / 5,165 ms |
| static | leave | 4,809 / 4,972 ms | 4,674 / 4,842 ms |
| wrangler | enter | 4,951 / 5,641 ms | 4,802 / 5,405 ms |
| wrangler | leave | 5,337 / 5,743 ms | 5,124 / 5,551 ms |

Controls showed no freeze in any round:

| Control | Longest task | Layerize |
| --- | --- | --- |
| JA desktop | 0.2–0.3 s | about 7 ms |
| EN mobile 390 | about 0.9 s | about 0.75 s |
| Ordinary page pair | about 0 | about 1.5 ms |

**Census.** The page has about 17k nodes and is about 258k px tall, with 23 `ContentTable` wrappers (`div.overflow-x-auto`).

- At EN 1440, two of those tables overflow their wrapper (1,112 px table in a 1,016 px wrapper).
- In JA none overflow, because Japanese text can break.
- At EN 390 all of them overflow.

**Controlled experiments**, run on the same tree and host with EN desktop as the target:

| Experiment | Longest task, median | Layerize, median | Freeze rounds |
| --- | --- | --- | --- |
| E1: view transitions off | 0.24 s | 0.7 ms | 0/4 |
| E2: wrappers `overflow-x: visible` | 0.32 s | 7.5 ms | 0/4 |
| E3: `contain: paint` on wrappers | 0.23 s / 0.19 s | 4–6 ms | 0/4 (EN mobile 0.18–0.22 s) |
| E4: cell `overflow-wrap: anywhere` | 0.25 s / 0.19 s | 6–8 ms | 0/4 (EN mobile 0.33–0.43 s) |
| E5: `content-visibility: auto` | 0.20 s / 0.14 s | 2–3 ms | 0/4 |

So the freeze needs both conditions: a view-transition soft navigation, and the content tables acting as overflowing scroll containers. Removing either one eliminates it.

## Observation 2 (Wrangler pushState→swap gap) is the same freeze

On the leave navigation, the baseline's pushState→before-swap gap tracked the freeze on **both** hosts: static 4.8 s and wrangler 5.4 s. It fell with the fix, to 0.14–0.16 s on both. The plain static server shows it too, so in this matrix the gap is not Wrangler-specific. It is the outgoing page's Layerize/snapshot work landing between pushState and swap.

#4156's original 21-run plain-static observation could not be compared against the vanished traces, so this explanation rests only on this matrix.

## Repair

`docs/src/styles/global.css` applies `.zd-content .overflow-x-auto:has(> table.border-collapse) { contain: paint; }`. The wrapper already clips its content with `overflow-x: auto`, so rendering and horizontal scrolling are unchanged. The only sticky element on the page is the header, which sits outside the wrappers.

I chose E3 over E5 because `content-visibility: auto` changes off-screen layout (intrinsic-size estimates), and that risks scroll jumps for anchor navigation. E2 and E4 alter table readability. E1 removes a feature.

### The predeclared target was met by a wide margin

Fix tree `bf036811`, both hosts, all cases: **0/4 freeze rounds everywhere**.

| Host | Direction | Longest task, median | Layerize, median |
| --- | --- | --- | --- |
| EN desktop, static | enter / leave | 216 / 178 ms | 5.3 / 8 ms |
| EN desktop, wrangler | enter / leave | 202 / 238 ms | 5.4 / 7.6 ms |

The controls did not regress:

- JA desktop: unchanged.
- EN mobile: improved from about 0.9 s to about 0.22 s.
- Ordinary page pair: unchanged.

The real host's `verify.mjs`, `content-verify.mjs` and `workshop-verify.mjs` pass on the fixed build.

## Limits

- Chromium only, one Mac. These are local comparison numbers, not a CI wall-clock budget.
- Every case logs the same pre-existing console message, "Blocked script execution in 'about:srcdoc'", from sandboxed preview iframes. It occurs equally in every case, including controls and the fixed tree.
- The upstream `ContentTable` in zudo-doc would be the more general place for this containment. The host rule is the scoped repair for this site.
