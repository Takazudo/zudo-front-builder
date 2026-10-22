# cargo-removal test fixtures

Synthetic inputs for the `--reason cargo-removal` tests in
`rebaseline-island-size-{linux,darwin}.test.mjs` (#4388). They are **test
fixtures, not native evidence**: the locks, manifests, declaration and closure
receipts are built by `island-size-cargo-removal.mjs` and never stand for a real
transition or a live contract.

- `old.lock` / `new.lock`: an eleven-record lock reduced from the Deno exit. The
  external `deno_core` and `serde_v8` records leave the lock, and the internal
  `zfb-js-host` and `zfb-render` records each drop their `deno_core` edge.
  Every other record and the header are byte-identical. Two `itoa` versions
  keep `serde_json`'s `"itoa 1.0.15"` spelling and `zudo-wind`'s
  `"itoa 0.4.8"` spelling unambiguous, so one negative control can remove
  `itoa 0.4.8` and show the resulting `"itoa 1.0.15"` to `"itoa"` rewrite of
  the external `serde_json` record being refused.
- The producer closure (`zfb` at its default feature) is the same at both
  locks: `deno_core` and `serde_v8` were never in it, and `serde_json` /
  `itoa 1.0.15` are reached only over a dev edge, so they are outside it too.
