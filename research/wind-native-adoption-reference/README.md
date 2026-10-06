# Independent reference compiler probes for #3833

Collected 2026-10-06 using Node v24.14.0 at Wind decision source 3cfe07aa60046e642a3128b25cd93ad0203b5be5. These are test-only public compiler results, not Wind output or browser evidence.

Each file records the complete explicit candidate list, exact compiler CSS, npm version, tarball SHA-256/SRI and observed source identity. Input CSS was exactly `@theme { --*: initial; } @tailwind utilities;`: no preset theme, preflight, source scanner or authored CSS. The compiler module was loaded from the artifact verified by `reference-cli.mjs probe`; tag-to-artifact identity remains unverified.

| File | Candidates | File SHA-256 |
| --- | --- | --- |
| positive.json | 37 | `f6ed6edeb9dd135e8cd20be95b35577a36947ce5213bedcb221f1ac0a7336efd` |
| boundaries.json | 19 | `b8a4af3f90b2c3f7d7b0ba13768bb46104b525dac4b59110256cf99c6bad73b0` |
| accessibility.json | 3 | `e554a3912ab46139d5079f2b2f665cbbf6faf7b656e0fc7c99ad44a496fb7aed` |

To reproduce a row, run `node scripts/wind-compatibility/reference-cli.mjs probe --cache /tmp/zfb-wind-reference-cache --candidates '<comma-joined candidates from that JSON>'`. The cache stays outside Git. A generated rule is compiler evidence only; missing rules and compiler-accepted invalid browser values are also material to the bounded decision.

Positive contains proposed adoptions plus selected intrinsic/numeric omissions. Boundaries deliberately contains both reference-rejected and reference-accepted forms which Wind will reject, including zero-denominator fractions, large integers and SVG opacity. Accessibility retains the full upstream sr-only and reversal declaration model, including its focus variant. Do not normalize these outputs into Wind expectations or overwrite them from Wind results.
