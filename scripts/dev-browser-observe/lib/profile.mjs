import { readFileSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { validateAs } from "./schema.mjs";
import { sha256 } from "./util.mjs";

export const LABEL_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

const NEEDS_ARTICLE = new Set([
  "edit-title",
  "edit-body",
  "edit-category",
  "delete-article",
  "stale-predicate",
]);

export function semanticErrors(profile) {
  const errors = [];
  if (profile.server?.port?.mode === "fixed" && !profile.server.port.port)
    errors.push("$.server.port.port required for fixed mode");
  for (const s of profile.scenarios ?? []) {
    for (const [i, op] of (s.ops ?? []).entries()) {
      if (NEEDS_ARTICLE.has(op.op) && !op.article)
        errors.push(`$.scenarios[${s.id}].ops[${i}]: ${op.op} needs article`);
      if (op.op === "stale-predicate" && op.expect !== "timeout")
        errors.push(
          `$.scenarios[${s.id}].ops[${i}]: stale-predicate must declare expect: "timeout"`,
        );
    }
  }
  return errors;
}

export function loadProfile(path) {
  const abs = resolve(path);
  const raw = readFileSync(abs);
  const profile = JSON.parse(raw.toString("utf8"));
  const errors = [...validateAs("profile", profile), ...semanticErrors(profile)];
  if (errors.length) {
    const e = new Error(`invalid profile ${abs}:\n  ${errors.join("\n  ")}`);
    e.code = "PROFILE_INVALID";
    e.errors = errors;
    throw e;
  }
  const base = dirname(abs);
  const r = (p) => (isAbsolute(p) ? p : resolve(base, p));
  const outDir = r(profile.outDir);
  return {
    profile,
    path: abs,
    sha256: sha256(raw),
    resolved: {
      buildReceipt: r(profile.buildReceipt),
      fixtureDir: r(profile.fixture.dir),
      outDir,
      lockPath: profile.lockPath ? r(profile.lockPath) : join(outDir, ".dev-browser-observe.lock"),
      registryPath: join(outDir, ".dev-browser-observe-owned.jsonl"),
    },
  };
}
