# Captured built anchor inventory

`wind-built-anchors.v1.json` records the 116 zudo-wind routes and their emitted
HTML ids and heading ids. `wind-adjacent-built-anchors.v1.json` records the
emitted ids for 42 nearby CSS guidance routes. These are snapshots from the
captured parent, based on actual static HTML rather than slugs inferred from
source Markdown.

The parent build was cross-checked against the successful docs deploy baseline
at [`4b6b45478a8d63c735f9be7a8ce144fe861da366`](https://github.com/Takazudo/zudo-front-builder/commit/4b6b45478a8d63c735f9be7a8ce144fe861da366),
from [docs deploy run 37207983158](https://github.com/Takazudo/zudo-front-builder/actions/runs/37207983158).
The captured parent source SHA is `70699acdf807b85cc19d050ae30f9adf0d904f57`.

The audit compares each baseline route with its current built `index.html`,
checks all old ids and separately requires every old heading id to remain on an
actual heading element. It also rejects duplicate ids in the current output.
