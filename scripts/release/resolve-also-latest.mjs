#!/usr/bin/env node
// resolve-also-latest.mjs — the prerelease dual-tag decision (#481) for
// release.yml's publish and finalize jobs (#4690).
//
//   node scripts/release/resolve-also-latest.mjs --dist-tag <next|latest> [--registry URL]
//
// Prints `1` when a `next` publish must also advance `latest`, otherwise an
// empty line. `latest` is only advanced while the registry is still in the
// prerelease phase: @takazudo/zfb has no `latest` at all (first publish, or the
// package was never published — a definitive 404) or its `latest` is itself a
// prerelease. Any other read failure (5xx, auth, network, malformed body) exits
// 2: it must never be mistaken for "first publish", which would let a
// prerelease clobber a real stable `latest`.
import { isMain } from "./gh.mjs";
import { DEFAULT_REGISTRY, defaultRequest } from "./npm-package-state.mjs";

export const SIGNAL_PACKAGE = "@takazudo/zfb";

export async function readLatest({ registry = DEFAULT_REGISTRY, request = defaultRequest } = {}) {
  const url = `${registry}/-/package/${encodeURIComponent(SIGNAL_PACKAGE).replace("%40", "@")}/dist-tags`;
  const response = await request(url);
  if (response.status === 404) return { latest: "", basis: "package never published (E404)" };
  if (response.status !== 200) {
    throw new Error(
      `GET ${url} answered HTTP ${response.status}: ${response.body.toString("utf8").slice(0, 200)}`,
    );
  }
  let tags;
  try {
    tags = JSON.parse(response.body.toString("utf8"));
  } catch (error) {
    throw new Error(`GET ${url}: malformed JSON: ${error.message}`);
  }
  if (tags === null || typeof tags !== "object")
    throw new Error(`GET ${url}: not a dist-tags object`);
  if (tags.latest !== undefined && typeof tags.latest !== "string") {
    throw new Error(`GET ${url}: latest is not a string`);
  }
  return { latest: tags.latest ?? "", basis: "dist-tags read" };
}

/** Pure: the decision from the dist-tag being published and the current latest. */
export function alsoLatest(distTag, currentLatest) {
  if (distTag !== "next") return false;
  return currentLatest === "" || currentLatest.includes("-");
}

export async function main(argv, { request, out = process.stdout, err = process.stderr } = {}) {
  const opts = {};
  for (let i = 0; i < argv.length; i += 2) {
    if (!["--dist-tag", "--registry"].includes(argv[i]) || argv[i + 1] === undefined) {
      err.write("usage: resolve-also-latest.mjs --dist-tag <next|latest> [--registry URL]\n");
      return 2;
    }
    opts[argv[i].slice(2)] = argv[i + 1];
  }
  if (!["next", "latest"].includes(opts["dist-tag"])) {
    err.write("usage: resolve-also-latest.mjs --dist-tag <next|latest> [--registry URL]\n");
    return 2;
  }
  if (opts["dist-tag"] !== "next") {
    err.write(`dist-tag is ${opts["dist-tag"]} — dual-tag not applicable.\n`);
    out.write("\n");
    return 0;
  }
  let current;
  try {
    current = await readLatest({
      registry: opts.registry ?? process.env.ZFB_NPM_REGISTRY ?? DEFAULT_REGISTRY,
      ...(request ? { request } : {}),
    });
  } catch (error) {
    err.write(
      `::error::cannot read ${SIGNAL_PACKAGE} dist-tags — failing closed: ${error.message}\n`,
    );
    return 2;
  }
  const decision = alsoLatest("next", current.latest);
  err.write(
    `${SIGNAL_PACKAGE} latest='${current.latest}' (${current.basis}) → ${decision ? "advance latest alongside next" : "latest is stable; leave it"}\n`,
  );
  out.write(decision ? "1\n" : "\n");
  return 0;
}

if (isMain(import.meta.url)) {
  process.exitCode = await main(process.argv.slice(2));
}
