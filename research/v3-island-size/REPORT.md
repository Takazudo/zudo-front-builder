# V3 island size decision input

Measurement pending the manager's serial guarded run on the merged base. Use the exact commands in [README.md](README.md), then commit the generated findings here. Do not substitute modeled esbuild sidecar totals for real `zfb build` shipped JS totals.

| Provenance | Value |
| --- | --- |
| Merged source SHA | pending |
| `@takazudo/zfb` package version and packed tarball SHA-256 | pending |
| esbuild version and executable SHA-256 | pending |
| Node and zlib versions, gzip flags | pending |
| Real workspace / packed report paths | pending |
| Modeled workspace / packed metafile paths | pending |
| Repeated identical build verdict | pending |

## Actual shipped JS

Paste the real workspace and packed tables from their generated `report.md` files, including no-island, initial/later, all shipped, and per-chunk raw/gzip. Explain any workspace-versus-packed difference. Record the multi-island incremental cost with shared chunks counted once.

## Retained inputs and reachability

Use the modeled metafiles to identify retained inputs and `bytesInOutput` for event-only, scalar signal, Show/For, model, blog/theme, and JSON API. State whether optional paths are reached, with paths and byte counts. Report the global keep-names probe separately. Do not claim independent tree shaking from a single input row.

## Limits

These frozen recipes are controlled current-baseline measurements. Exact historical v2/v3 recipe inputs have not been recovered, so no historical numbers are reproduced. The modeled sidecar matches the production export-selection and registration protocol but uses a fixed identity and relative temporary imports instead of the CLI's real build identity and staged paths. It is an attribution aid; real CLI output controls shipped totals. No arbitrary byte ceiling is applied.
