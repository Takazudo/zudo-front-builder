# Installed registry browser verification (#3879)

Standalone opt-in verification; no automatic promotion to a repository gate.
Linux x64 glibc is the frozen native platform. The manager owns all execution
that builds or launches Chromium, under the machine-wide heavy guard.

```sh
# Optional browser provisioning when playwright 1.56.1 is not already cached:
bash "$HOME/.codex/scripts/heavy-guard.sh" -- npm exec --yes --package=playwright@1.56.1 -- playwright install chromium
# From the checkout, use a fresh empty directory OUTSIDE the repository:
bash "$HOME/.codex/scripts/heavy-guard.sh" -- node tests/published-browser-verification/run.mjs /tmp/zfb-published-browser-261008
```

Each subprocess has a 180-second liveness guard; expiry fails the harness and
cannot satisfy a negative control. Child groups receive TERM then KILL after
five seconds. Browser launch failures also close the owned static server.

An existing compatible Chromium may be selected with
`CHROMIUM_EXECUTABLE_PATH=/absolute/path/to/chromium`. Evidence records the actual
browser version; using an external executable must be stated in the run report.

`pins.json` captures the exact public registry SDK, router runtime, and native
carrier tuple for 4.1.0, 4.0.0 and 3.2.0, including integrity, tarball URLs and
provenance attestation metadata. Installation compares lockfile resolution and
integrity with those pins, checks actual package versions and non-symlink paths,
and records the installed native binary SHA-256 and `--version`. npm verifies
tarball integrity. Attestation metadata is recorded, not independently verified.
No workspace package, packed workspace tarball, local engine, or mutable dist-tag
is substituted. No `zfb` npm alias is added to repair the old CLI's resolution.

The release docs under `docs/` are verbatim Git-tag snapshots, stored as `.txt` to prevent documentation
formatters from rewriting their source bytes. Their tag commit,
original source path and SHA-256 are pinned. Recipe extraction retains the SDK
imports, component fences and Content call exactly; only relative local imports
are adjusted because the executable fixture uses `/` rather than a dynamic blog
route. The root page and content collection supply the surrounding runnable site.

Assertions:

- 4.1.0 mount AND hydrate an initially false root Show, observe the late branch,
  unmount, assert disposed and zero owned child nodes, and reject diagnostics.
- 3.2.0 runs the identical lifecycle and must reproduce the concrete `<p>later</p>`
  leak for both root methods. Both versions' intrinsic-wrapper controls must
  clean up. Idempotent unmount and post-disposal mutation must not change DOM.
- 4.1.0 builds the exact release Fragment MDX recipe with bare `zfb` imports,
  serves its production `dist`, observes both registered AND mounted islands,
  and clicks each real counter from 0 to 1. All browser errors/warnings fail.
- Removing only the root map's Fragment delimiters must reject its opaque map
  with a boundary/opaque diagnostic in 4.1.0.
- 4.0.0 builds its own exact release docs recipe and must report both no SDK boundary
  targets/skipped islands bundle and `ZR_ISLAND_IDENTITY: Counter is not registered
  by the scanner`. This is [#3917 variant A](https://github.com/Takazudo/zudo-front-builder/issues/3917).
  The unresolved client import belongs to variant C, which changes only the
  `Island` import; this harness preserves both bare imports. A generic unrelated
  build failure cannot satisfy the control.

3.2.0 scoped-import Fragment behavior is deliberately not a negative control:
that shape historically passed. Downstream consumer migration, WebKit, pixel
appearance and other platforms remain outside this verification.

The output directory retains every isolated consumer, installed package lock,
bundle, production output and full command stdout/stderr. `evidence.json` records
OS/Node/browser, all command arguments and outcomes, diagnostics and measured
DOM/registration/click results. Overall failures exit nonzero and retain evidence;
they are not reclassified as old-version controls. Read the case name and matched
diagnostic for each named control. Browser/build execution is pending until the
manager records the tested commit and the evidence artifact.
