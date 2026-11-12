// The complete GitHub Release asset set for one version (#4690):
//   ci      — the three hosted-runner archives + their .sha256 (release-assets job)
//   darwin  — the ten owner-Mac inputs (scripts/build-darwin-local.sh --upload)
//   mdWasm  — the published MD/WASM digest manifest (publish/finalize job)
import { DARWIN_TARGETS, assetNames } from "../darwin-artifact-contract.mjs";

export const CI_TARGETS = Object.freeze([
  { target: "x86_64-unknown-linux-gnu", ext: "tar.gz" },
  { target: "aarch64-unknown-linux-gnu", ext: "tar.gz" },
  { target: "x86_64-pc-windows-msvc", ext: "zip" },
]);

export function expectedReleaseAssets(version) {
  const ci = CI_TARGETS.flatMap(({ target, ext }) => {
    const archive = `zfb-${version}-${target}.${ext}`;
    return [archive, `${archive}.sha256`];
  });
  const darwin = Object.keys(DARWIN_TARGETS).flatMap((target) =>
    Object.values(assetNames(version, target)),
  );
  return { ci, darwin, mdWasm: `zfb-md-wasm-${version}-shipped-artifacts.json` };
}

/** Pure: which expected assets are missing, and which names are ambiguous/not uploaded. */
export function assetInventory(version, assets) {
  const expected = expectedReleaseAssets(version);
  const counts = new Map();
  for (const asset of assets) counts.set(asset.name, (counts.get(asset.name) ?? 0) + 1);
  const states = new Map(assets.map((asset) => [asset.name, asset.state ?? "uploaded"]));
  const check = (names) => ({
    present: names.filter((name) => counts.has(name)),
    missing: names.filter((name) => !counts.has(name)),
  });
  const all = [...expected.ci, ...expected.darwin, expected.mdWasm];
  return {
    expected,
    ci: check(expected.ci),
    darwin: check(expected.darwin),
    mdWasm: check([expected.mdWasm]),
    conflicting: all.filter(
      (name) =>
        (counts.get(name) ?? 0) > 1 || (counts.has(name) && states.get(name) !== "uploaded"),
    ),
  };
}
