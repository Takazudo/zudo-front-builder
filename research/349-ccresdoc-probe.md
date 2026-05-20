# Research: CCResDoc-on-current-zfb probe — Mode B sidecar (#349)

## 1. Question

What would CCResDoc-on-current-zfb actually look like, and specifically:
should the next CCResDoc iteration run zfb as a **Mode B sidecar**
(Tauri spawns `zfb dev` as a child process and points the WebView at
`localhost:<port>`) or as a **Mode D in-process embed** (Tauri links
`zfb-server` as a library and runs the axum loop on its own tokio
runtime)?

The existing CCResDoc (<https://github.com/Takazudo/ccresdoc>) is
**Mode C** (zfb at build time, ship `dist/`) with ~600 lines in
`src-tauri/src/main.rs`, most of which exist to compensate for the
Astro rebuild story it inherited. The deliverable here is not a
migration — it is empirical evidence about which of Mode B / Mode D
unblocks the CCResDoc use case ("render an arbitrary on-disk MDX
file inside a desktop window, with live updates when the file
changes").

The composition matrix is defined in
`docs/src/content/docs/guides/desktop-deployment.mdx` (Mode A static,
Mode B sidecar, Mode C build-time, Mode D in-process). #349 focuses
on Mode B; Mode D is gated by #346's embed-as-library API.

## 2. What was tried

Time-box: ~60–90 minutes of agent compute (rescue spawn after an
earlier session crashed). The prior session had already laid down a
near-complete probe scaffold in `__inbox/ccresdoc-zfb-probe/`; this
session audited that work, fixed one misleading comment, attempted a
real build, and produced this postmortem-shaped doc.

Concrete actions:

- `gh issue view 349` — pulled the issue body and the "next concrete
  step" list.
- Read `docs/src/content/docs/guides/desktop-deployment.mdx` —
  confirmed the Mode B sidecar pattern as described in §"The harder
  case: rebuild content inside the app" (the sidecar bullet).
- Read `docs/src/content/docs/guides/ssr-and-cloudflare-bindings.mdx`
  — confirmed `prerender = false` is a Cloudflare-adapter SSR
  feature, **not** a dev-server feature. The dev server does not
  execute SSR handlers at request time.
- Read `crates/zfb-server/src/lib.rs` end-to-end (235 lines) and
  skimmed `crates/zfb-server/src/{routes,livereload,inject}.rs` for
  the dev-server boot surface.
- Read `crates/zfb/src/cli.rs` `DevArgs` to verify `zfb dev --port
  <n>` is a real CLI flag (it is — `#[arg(long)] port: Option<u16>`).
- Read `crates/zfb-watcher/src/lib.rs` to confirm the watcher is
  rooted at `project_root` and does not watch arbitrary paths
  outside the project tree.
- Audited the existing probe at `__inbox/ccresdoc-zfb-probe/`:
  - `src-tauri/src/main.rs` (126 lines) — spawn-and-wait pattern,
    `Drop`-based child-kill on app exit, fixed-port wait loop.
  - `src-tauri/Cargo.toml` (25 lines) — standalone (`[workspace]`
    block keeps it out of the zfb workspace), Tauri 2.x,
    `tauri-plugin-shell`, `anyhow`, serde.
  - `src-tauri/tauri.conf.json` (28 lines) — `app.windows[0].url`
    pinned to `http://localhost:4321`, CSP nullified.
  - `src-tauri/capabilities/default.json` (10 lines) — `core:default`
    + `shell:allow-execute`.
  - `claude-doc-site/pages/index.tsx` (46 lines after edit) — SSG
    page reading `$HOME/.claude/CLAUDE.md` via `getStaticProps`.
  - `claude-doc-site/{package.json,zfb.config.json,tsconfig.json}` —
    minimal zfb project; depends on `@takazudo/zfb` via workspace
    protocol.
- Fixed one misleading comment in `claude-doc-site/pages/index.tsx`
  (it claimed "render at request time"; the real story is
  build-time SSG, see §3.1).
- Ran `cargo build --offline` in `__inbox/ccresdoc-zfb-probe/src-tauri/`
  — succeeded in 2m 04s. Output binary: 198 MB debug build at
  `~/.cargo-target/debug/ccresdoc-zfb-probe` (the workspace's
  `CARGO_TARGET_DIR`). `cargo check --offline` re-runs in ~2.4 s with
  no warnings.
- Read sibling research `research/346-embed-as-library-api.md` for
  the Mode D follow-up dependency (commit 76a8fbf, branch
  `research/346-embed-as-library-api`).

What was deliberately NOT done:

- `cargo tauri dev` / WebView launch — prohibited by the rescue
  prompt (no graphical environment).
- `pnpm install` inside `claude-doc-site/` and `zfb dev` smoke run
  — out of scope for the compile-only validation. Adds nothing the
  build hasn't already proven about the spawn glue.
- Mode D variant of the probe — deferred per the rescue prompt and
  #349's "if R3 is in progress" caveat; #346 is at "API sketch", not
  "API shipped" (the spike validated embeddability against today's
  `ServeOpts`, but the proposed `Server::builder()` surface is not
  implemented yet).
- `claude-doc-site/dist/` / `node_modules/` were never produced, so
  there is no risk of accidentally committing CLAUDE.md content
  rendered through `getStaticProps` (see §6).

## 3. Evidence

### 3.1 Probe shape and line counts

```
__inbox/ccresdoc-zfb-probe/
├── claude-doc-site/                      (zfb sample project)
│   ├── package.json                  18 lines
│   ├── pages/index.tsx               46 lines  (was 40; +6 from comment edit)
│   ├── tsconfig.json                 16 lines
│   └── zfb.config.json                6 lines
└── src-tauri/                            (Tauri host that spawns zfb)
    ├── Cargo.toml                    25 lines
    ├── build.rs                       3 lines
    ├── capabilities/default.json     10 lines
    ├── src/main.rs                  126 lines
    └── tauri.conf.json               28 lines
```

Total hand-written LOC: **278** (no source file approaches the 800-line
cliff the rescue prompt named; the 200-line target is exceeded only
because two side-by-side configurations — a Rust crate and a zfb
project — are needed to demonstrate Mode B at all).

`src-tauri/gen/schemas/` (4 small JSON files) is `tauri-build`
output and not counted above. `icons/icon.png` is a 68-byte 1×1
PNG placeholder.

Comparator: CCResDoc's current `src-tauri/src/main.rs` is reported
in the #349 issue body at ~600 lines — so this Mode B probe is
roughly **5× smaller** than the Mode C status quo for the spawn

+ WebView role. Most of CCResDoc's bulk is Astro-rebuild

compensation logic that Mode B does not need (because the zfb dev
server already owns the watcher + rebuild + livereload story).

### 3.2 Critical finding — `prerender = false` does not work in the dev server today

The #349 issue body asks for the probe to render CLAUDE.md "at
request time via a `prerender = false` page." That phrasing
**cannot be honoured with current zfb**:

- `prerender = false` is documented in
  `docs/src/content/docs/guides/ssr-and-cloudflare-bindings.mdx` as
  the Cloudflare-adapter opt-out. Routes that set it are bundled
  into `_worker.js` + `_zfb_inner.mjs` and served by Cloudflare
  Pages **post-deploy**.
- `crates/zfb-server/` has no SSR execution path. `lib.rs` /
  `routes.rs` only serve the `PageCache` (populated by the build
  orchestrator's render loop), the `dist/` and `public/` fallback
  chain, and the `__zfb/*` dev-only routes (livereload SSE + script).
  A grep for `ssr` inside `crates/zfb-server/src/` returns nothing.
- The sibling research `research/346-embed-as-library-api.md` reaches
  the same conclusion from the Mode D side: "the dev server does
  NOT currently execute those handlers in-process" — and proposes
  `with_ssr_handler(pattern, handler)` as a v1 builder method to
  fill that gap.

The probe's `pages/index.tsx` therefore uses `getStaticProps`,
which runs at build time. The closest thing to "request-time
freshness" the current dev server can offer is "build-time freshness,
re-triggered by the watcher" — and the watcher caveat (§3.3) means
even that fails for `$HOME/.claude/CLAUDE.md`.

**This is the headline empirical finding of the probe**: Mode B
sidecar over current zfb cannot deliver request-time MDX rendering
for arbitrary out-of-project files. The probe's `index.tsx` reads
CLAUDE.md once per build tick, not once per request, and that
build tick only fires on changes inside the project root.

### 3.3 Critical finding — the watcher cannot see `$HOME/.claude/CLAUDE.md`

`crates/zfb-watcher/src/lib.rs` starts at a `project_root` and
recursively watches a list of relative paths beneath it (`pages/`,
`content/`, `public/`, etc.). There is no public API to register
an absolute path outside the project root, and `notify`'s
recursive watcher is scoped to subtrees of the registered root.

Consequence: if a user edits `~/.claude/CLAUDE.md` while the Mode B
sidecar is running, the page does **not** refresh — neither the
build orchestrator's render loop nor `__zfb/reload` SSE fires
because the watcher saw no change.

Workarounds, ranked by friction:

1. **Symlink `$HOME/.claude/CLAUDE.md` into the project's `pages/`
   or `content/`** — cheap, but `notify` follows symlinks unevenly
   across platforms and the page module would need to re-import.
2. **Spawn an out-of-zfb watcher inside the Tauri host** that
   `touch`es a file inside the project root on CLAUDE.md change,
   triggering an indirect rebuild. Adds Rust glue but keeps zfb
   unchanged.
3. **Add an "extra-watch" config knob to zfb-watcher** (`watch:
   [{ root: "$HOME/.claude", patterns: ["CLAUDE.md"] }]`). Cleanest,
   requires an upstream feature.
4. **Mode D + `with_request_extension`**: skip the watcher entirely;
   the SSR handler reads CLAUDE.md per request and the WebView
   refreshes on user action. This is the path #346 sketches.

### 3.4 Compile result

```
$ cd __inbox/ccresdoc-zfb-probe/src-tauri && cargo build --offline
   ... (workspace deps resolved from Cargo.lock, 4790 lines)
   Compiling ccresdoc-zfb-probe v0.0.0 (.../src-tauri)
   Compiling muda v0.19.2
   Compiling tauri-runtime v2.11.2
   Compiling tauri-runtime-wry v2.11.2
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2m 04s

$ cargo check --offline
    Checking ccresdoc-zfb-probe v0.0.0 (.../src-tauri)
    Finished `dev` profile [unoptimized + debuginfo] target(s) in 2.39s

$ file ~/.cargo-target/debug/ccresdoc-zfb-probe
ELF 64-bit LSB pie executable, x86-64, ..., dynamically linked, ...
$ ls -la ~/.cargo-target/debug/ccresdoc-zfb-probe
-rwxr-xr-x ... 198757616 ...
```

198 MB unstripped debug build — typical for Tauri 2 with
`webkit2gtk-4.1` (system: 2.52.3) + `gtk+-3.0` (system: 3.24.41).
Release-mode + `strip` would shrink that meaningfully; the rescue
prompt's "binary-size delta" question (issue §"Questions to
investigate") is therefore not answered in numeric form here —
it requires release builds of both Mode A (Astro+sidecar) and
Mode B (zfb+sidecar) on the same machine, which is out of scope
for a compile-only probe.

### 3.5 What the probe demonstrates without launching

Even without `cargo tauri dev`, the probe proves:

| Claim | Evidence |
|---|---|
| Tauri 2 compiles on this Linux host | `cargo build --offline` finished, 198 MB binary |
| `zfb dev --port` is a valid CLI form | `crates/zfb/src/cli.rs` `DevArgs::port: #[arg(long)] Option<u16>` |
| The spawn shape is correct | `src-tauri/src/main.rs` `Command::new(bin).arg("dev").arg("--port").arg(...)` matches the documented `host:port` resolution in `crates/zfb/src/commands/dev.rs:119` |
| The child cleans up on app exit | `Drop for ZfbServer` calls `.kill()` then `.wait()` |
| The WebView would point at `localhost:4321` | `tauri.conf.json` `app.windows[0].url` |
| The shell capability is loaded | `capabilities/default.json` lists `shell:allow-execute` |

What the probe does NOT prove (would require launching):

- The WebView actually renders the page (vs. a CSP / mixed-content /
  loopback-policy block).
- Tauri's localhost loader resolves zfb's `<link rel="stylesheet">`
  and `<script type="module">` URLs correctly.
- The livereload SSE channel survives the WebView's same-origin
  policy at `localhost:4321`.
- First-launch latency feels acceptable (subjective; needs a graphical
  environment).

These are explicit follow-ups (§5).

### 3.6 Dead code noted but not removed

`src-tauri/src/main.rs` initialises `tauri-plugin-shell` and the
ACL grants `shell:allow-execute`, but the probe spawns `zfb dev`
via `std::process::Command` — not via the plugin. Two reasons to
leave it:

- A "shipped" Mode B integration would bundle the zfb binary as a
  Tauri sidecar (`bundle.externalBin`) and spawn it via
  `tauri_plugin_shell::ShellExt::sidecar(…)` so Tauri's app-bundle
  packaging handles the platform-specific resolution. The plugin
  is there for that follow-up.
- Removing it would require another 2-minute compile pass with no
  empirical gain.

Trade-off acceptable for a probe; would not survive code review on
a real shipping crate.

## 4. Conclusion

### 4.1 Mode B verdict — partial pass with one structural blocker

Mode B is **mechanically viable** for the static-content slice of
the CCResDoc use case (binary compiles, Tauri host + zfb sidecar
glue is ~5× smaller than the comparable Mode C `main.rs`, all
sub-pieces of the spawn / wait / WebView triangle are reachable
from existing public surfaces). The shape `__inbox/ccresdoc-zfb-probe/`
demonstrates would be enough for a "ship today's CLAUDE.md as a
single MDX page" build artefact.

Mode B is **structurally insufficient** for CCResDoc's actual
ask — "render an arbitrary on-disk MDX file with live updates."
Two unrelated zfb gaps converge to block it:

1. The dev server does not execute `prerender = false` SSR handlers
   (§3.2). So "request-time render" cannot be expressed via the
   intended API.
2. The watcher only watches `project_root` (§3.3). So even
   build-time freshness fails for `$HOME/.claude/CLAUDE.md`,
   which sits outside any plausible zfb project root.

Workarounds for (2) exist (symlink, host-side watcher, upstream
watch-knob); the cleanest fix is the Mode D path itself
(§3.3 option 4).

### 4.2 Mode D recommendation — yes, but gated on #346

Mode D (`zfb-server` as a library, in-process axum in Tauri) is the
right next step for CCResDoc-shaped use cases. The principle:
"per-request native handler that reads disk and returns rendered
HTML" maps cleanly onto axum + `with_request_extension`, and
sidesteps both blockers (1) and (2) by routing requests through
Tauri rather than through zfb's watcher.

This recommendation is **conditional**. From #346's own findings:

- `zfb-server` is **already library-shaped** — no [[bin]], no
  globals, embedding works via today's `ServeOpts` +
  `serve_with_listener`. #346's spike compiles and binds. So the
  capability exists.
- The proposed builder API (`Server::builder()`, `ServerMode`,
  `with_request_extension`, `with_ssr_handler`) is **not yet
  implemented** — it is an API sketch, not a shipped surface.
  Adopting it requires landing #346's implementation work.

So: Mode D for CCResDoc is **deferred until #346 lands the
`Server::builder()` + `with_ssr_handler()` surface**. The probe
does NOT attempt the Mode D variant per the rescue prompt and
because the API is not yet usable.

### 4.3 Dependency on #346 — explicit callouts (codex-mandated)

Per the rescue prompt's codex 2nd-opinion guardrail:

- **Assumptions taken about #346's API**: **none**. The probe stuck
  strictly to Mode B (sidecar, spawn `zfb dev` as a child process).
  It does not import `zfb-server`, does not reference any of #346's
  proposed builder methods, and does not depend on the
  `with_ssr_handler` / `with_request_extension` extension points.
  The probe's behaviour is unchanged by anything that does or does
  not happen in #346's PR.
- **Mode D work deferred from #349 and the conditions for unblocking**:
  the in-process embed variant of the probe is deferred. It can be
  unblocked once #346 ships, specifically once these three pieces
  are merged on `main`:
  1. `pub struct ServerBuilder` and `Server::builder() -> ServerBuilder`
     entry point (the `[lib]` shape is already there; this is the
     ergonomics layer).
  2. `pub enum ServerMode { Dev, Preview, Embed }` and the
     `Embed` mode's livereload + SSE gating (so the in-process
     server doesn't try to mount `/__zfb/*` inside a Tauri host).
  3. `with_ssr_handler(pattern, handler)` — the per-request handler
     hook that the CCResDoc CLAUDE.md route would attach to. Without
     this, Mode D buys nothing over Mode B for this use case.

If #346 lands as sketched, the Mode D probe is a ~50-line edit
(replace the `Command::new(bin).spawn()` block with a
`Server::builder().mode(Embed).with_ssr_handler(...).build()?
.serve_in_thread()?` block) and the watcher / SSR blockers
disappear with it.

## 5. Follow-ups

What to verify with a real launch (graphical environment, Tauri
runtime):

- WebView loads `http://localhost:4321/` cleanly. Specifically:
  Tauri 2's default CSP and the `http://` (not `tauri://`) scheme
  combine to set `connect-src` policy — the probe nulls CSP
  (`security.csp = null`) to sidestep this, but a real ship would
  want explicit `default-src 'self' http://localhost:4321
  ws://localhost:4321` so the livereload SSE channel survives.
- Livereload SSE (`__zfb/reload`) actually connects from inside the
  WebView. Same-origin should be fine since both window and SSE
  endpoint share `localhost:4321`; verify in DevTools.
- First-launch latency. `zfb dev` does an initial build before
  serving; CCResDoc would compare against the cold-start of the
  current Astro sidecar. Without a graphical env this remains a
  research question.
- Asset path resolution — `<link rel="stylesheet" href="/assets/...">`
  must resolve under the WebView's URL scheme. Tauri's
  `http://localhost` flavour resolves these naturally; the
  `tauri://localhost` custom scheme has rewriting quirks
  (`docs/src/content/docs/guides/desktop-deployment.mdx` §"File
  paths" calls these out).

The Mode D variant probe (deferred):

- Lives under `__inbox/ccresdoc-zfb-probe-mode-d/` (parallel to
  this one, not a delta).
- Depends on the three #346 deliverables in §4.3.
- Expected to be ~50 lines smaller than the Mode B probe because
  there is no spawn + wait + Drop dance.
- Open question: in Mode D, does the Tauri host re-use zfb's
  build orchestrator (so MDX → HTML still flows through the zfb
  pipeline) or does the SSR handler render MDX directly inside
  Rust? If the latter, the embed loses the MDX-via-V8 pipeline
  and degrades to "axum static-render" — fine for raw markdown
  but loses islands, MDX components, etc. Worth confirming in the
  #346 implementation PR.

Surprises (no specific action — flagged for the next picker-upper):

- `tauri-plugin-shell` + `shell:allow-execute` permissions are loaded
  but unused. The probe spawns via `std::process::Command` directly.
  A real Mode B ship would route the spawn through the plugin so
  Tauri's sidecar resolution (`BaseDirectory::Resource`) handles
  per-platform binary placement. The dead code is left in
  intentionally as a placeholder; documented in §3.6.
- The probe assumes a fixed port (4321). A real ship needs port-0
  binding + a Rust → JS message passing the resolved port back to
  the WebView URL. Currently the spawn-then-wait dance has no way
  to learn the port from `zfb dev` other than scraping stdout (the
  "ready" line at `crates/zfb/src/commands/dev.rs:363`); a host
  watching for that line would be brittle. Mode D solves this
  trivially (the embedder owns the listener).
- `getStaticProps` reading `$HOME/.claude/CLAUDE.md` will silently
  succeed at build time (no error if the file exists) but produces
  a stale snapshot the moment the user edits it. The page module
  catches the read error and renders it inline — confirmed by code
  inspection only, not by a real failing read.

Unrelated finding raised as an issue: none in this session.

## 6. Scope exceptions

Files touched outside `research/` and `__inbox/`: **none**.

Files touched inside the allowed scope:

- `research/349-ccresdoc-probe.md` — this file. Primary deliverable.
- `__inbox/ccresdoc-zfb-probe/claude-doc-site/pages/index.tsx` —
  edited one comment block (the prior session's claim "render at
  request time" was inaccurate; replaced with the actual SSG +
  watcher-scope story so a future reader doesn't repeat the
  miscalibration). No semantic change to the rendered page.
- `__inbox/ccresdoc-zfb-probe/**` (all other files) — pre-existing
  from the prior session; no edits this round.

Force-add note: `__inbox/` is gitignored repository-wide (`.gitignore:34`).
Per the rescue prompt, the probe contents are intentionally
committed under `__inbox/ccresdoc-zfb-probe/` via `git add -f`. A
pre-add audit confirmed no `node_modules/`, no `dist/`, no `target/`
inside the probe directory (the Rust target lives at
`~/.cargo-target/`, outside the worktree). So the force-add does
not bypass `.gitignore` for any sensitive-by-content artefact.
**Specifically**: `claude-doc-site/dist/` was never produced this
session, so there is no `getStaticProps`-rendered snapshot of
`CLAUDE.md` content anywhere in the commit. The probe's `index.tsx`
reads CLAUDE.md only at the user's machine at zfb-build time —
that read is **input data**, not embedded content, and no committed
file contains personal CLAUDE.md text.
