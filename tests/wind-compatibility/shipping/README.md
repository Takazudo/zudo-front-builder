# Current-source shipping evidence

This suite is the production counterpart to the pure `zudo-wind` example comparison. It executes the current checkout's Cargo-built `zfb` binary directly. The comparison's `windBuild` remains a shared reference binding; `productionBuild` separately identifies the real CLI artifact, its bytes, `--version` under an empty `PATH`, Cargo JSON artifact message, source SHA, complete tested input closure, and lockfile. A globally installed or older binary fails validation.

The checked manifest describes the mandatory cases. `run.mjs` repeats `zfb css` across source add, edit, remove, rename, token change, and token removal, with a fresh same-mode CSS run after every phase. It checks the standalone source plan's configured source root and package root. It then reads a proven `zfb build` dist, checks actual HTTP CSS bytes against the generated file, verifies an expected missing asset remains 404, exercises a synthetic page's navigation, card, form, typography, responsive geometry, and nested theme, and retains bounded screenshots. A separate `zfb dev` process performs the same warm transitions and compares its final live CSS bytes to a clean dev process. The request-time `prerender = false` route is rendered through that dev process. Production SSG is represented by the build dist; production request-time deployment is outside this fixture's supported local mode.

The utility reference is the exact SRI-verified Tailwind tarball already used by the comparison. It receives explicit equivalent tokens and is rendered on a separate origin. Its utility screenshot must match the Wind utility screenshot byte for byte. Full page authored CSS and reset/cascade expectations are asserted separately through the production composition case; the isolated utility check does not claim historical page pixel identity.

All paths below are examples; choose an **outside-checkout** evidence directory and current comparison/plan files. Run the Cargo build and Rust dist export through the repository heavy guard in the manager lane, after all shipping source changes are integrated:

```sh
bash "$HOME/.codex/scripts/heavy-guard.sh" -- \
  node scripts/wind-compatibility/build-production.mjs "$EVIDENCE_DIR"
ZFB_WIND_REAL_BUILD_DIST="$EVIDENCE_DIR/dist" \
  ZFB_WIND_REAL_BUILD_EXECUTION="$EVIDENCE_DIR/build-execution.json" \
  bash "$HOME/.codex/scripts/heavy-guard.sh" -- cargo test -p zfb --test wind_real_build_confirm_build -- --nocapture
node tests/wind-real-build/dist-proof.mjs "$EVIDENCE_DIR/production-build.json" "$EVIDENCE_DIR/dist" "$EVIDENCE_DIR/build-execution.json" "$EVIDENCE_DIR/dist-proof.json"
node tests/wind-compatibility/shipping/run.mjs \
  --production-build "$EVIDENCE_DIR/production-build.json" \
  --comparison "$COMPARISON_JSON" --plan "$PLAN_JSON" \
  --dist "$EVIDENCE_DIR/dist" --dist-proof "$EVIDENCE_DIR/dist-proof.json" \
  --reference-cache "$REFERENCE_CACHE" --output "$EVIDENCE_DIR/shipping"
ZFB_WIND_REAL_BUILD_DIST="$EVIDENCE_DIR/dist" \
  ZFB_WIND_REAL_BUILD_PROOF="$EVIDENCE_DIR/dist-proof.json" \
  bash "$HOME/.codex/scripts/heavy-guard.sh" -- \
  pnpm exec playwright test --config tests/wind-real-build/playwright.config.mjs
```

`run.mjs` writes `report.json` only after all mandatory cases pass and the admission validator re-reads raw CSS, observations, screenshots, source plan, browser executable, reference, dist, and production executable provenance. A failed or unavailable browser/platform leaves no passing report. Keep the external evidence directory until promotion validation finishes. No reference compiler is loaded by the production binary or served dist.
