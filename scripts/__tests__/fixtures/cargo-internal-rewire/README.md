# cargo-internal-rewire test fixtures

Synthetic inputs for the `--reason cargo-internal-rewire` tests in
`rebaseline-island-size-{linux,darwin}.test.mjs` (#4258). They are **test
fixtures, not native evidence**: the locks, manifests, declaration and closure
receipts are built by `island-size-cargo-rewire.mjs` and never stand for a real
transition or a live contract.

- `old.lock` / `new.lock`: a five-record lock whose `zfb-render` record drops
  its direct `sha2` edge and gains a `zfb-js-host` edge while the new
  unpublished `zfb-js-host` record takes `anyhow` and `sha2`; every other
  record, the header and the external `sha2` record are byte-identical. This
  is the shape of the real #4242 extraction, reduced to five records.
