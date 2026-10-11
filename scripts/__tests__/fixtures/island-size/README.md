# Real island-size artifact bytes (#4131)

Verbatim copies of two `pass-1` dist trees from the retained `island-size-linux-x64` artifact of
[health run 38075998971](https://github.com/Takazudo/zudo-front-builder/actions/runs/38075998971)
(job 114282961692, workspace mode, PR merge source `3aaad9b4c403a3c8e920384885c541d5864d9c00`,
probe head `d3d78455daa4f632a0e9d3d76ed51ad9be45875b`, `@takazudo/zfb` 4.3.1, Linux x64, Node
v24.14.0, zlib 1.3.1-e00f703):

- `multi-island/` — `index.html`, `__zfb/routes.json`, and the islands entry
  `assets/islands-dca258ba.js` (SHA-256 `dca258ba…4cff09`, 67072 raw bytes, build token
  `d1cbd3e6cdd9d75e`).
- `no-island/` — the zero-JavaScript dist.

`island-size-identity-normalization.test.mjs` recomputes every number from these bytes and ties
`multi-island` to the `workspace` ceiling in `research/v3-island-size/decision-linux-x64.json`.
The directory is excluded from `vp fmt` in `vite.config.mjs` so the minified entry stays
byte-exact; do not reformat or hand-edit these files.
