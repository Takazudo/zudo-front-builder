import { readFileSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import {
  FN_CALIBRATION_ARTICLE,
  FN_CLEANUP_DETAIL,
  FN_EXPECTED_NEGATIVE_OPS,
  FN_NEEDS_ARTICLE,
  FN_OP_WRITE_METHODS,
  fnWarmPaths,
} from "./fieldnotes.mjs";
import { OP_WRITE_METHODS } from "./fixture.mjs";
import { validateAs } from "./schema.mjs";
import { DIAGNOSTIC_ENV } from "./server.mjs";
import { sha256 } from "./util.mjs";

export const LABEL_RE = /^[A-Za-z0-9][A-Za-z0-9._-]{0,127}$/;

const UNSPECIFIED_HOSTS = new Set(["", "0.0.0.0", "::", "[::]"]);

const NEEDS_ARTICLE = new Set([
  "edit-title",
  "edit-body",
  "edit-category",
  "delete-article",
  "create-article",
  "rename-article",
  "stale-predicate",
]);

const TINY_OPS = new Set(Object.keys(OP_WRITE_METHODS));

/** Field-notes-only rules: op set per kind, expected-negative placement, managed CSS, public CSS classification, hidden-edit isolation. */
function fieldNotesErrors(profile) {
  const errors = [];
  const at = (s, i) => `$.scenarios[${s.id}].ops[${i}]`;
  if (!profile.fixture.source)
    errors.push("$.fixture.source is required for kind field-notes (dir + pinned tree hashes)");
  // Paths the run may load before a hidden-edit, known statically: cleanup's default live-check detail page,
  // the calibration article, and every warm path, target and article of earlier ops in profile order
  // (any scenario may be selected into the same run). Planned supplementals depend on the dataset, so the
  // run-time request-log guard (hidden-precondition-visited) is the complete check.
  const visited = new Set([
    `/articles/${FN_CLEANUP_DETAIL}/`,
    `/articles/${FN_CALIBRATION_ARTICLE}/`,
  ]);
  for (const s of profile.scenarios ?? []) {
    for (const p of fnWarmPaths(s)) visited.add(p);
    for (const [i, op] of (s.ops ?? []).entries()) {
      const allowed = FN_OP_WRITE_METHODS[op.op];
      if (!allowed) {
        errors.push(`${at(s, i)}: ${op.op} is not a field-notes op`);
        continue;
      }
      if (op.writeMethod && !allowed.includes(op.writeMethod))
        errors.push(
          `${at(s, i)}: ${op.op} does not allow writeMethod ${op.writeMethod} (allowed: ${allowed.join(", ")})`,
        );
      if (FN_NEEDS_ARTICLE.has(op.op) && !op.article)
        errors.push(`${at(s, i)}: ${op.op} needs article`);
      const negative = FN_EXPECTED_NEGATIVE_OPS.has(op.op);
      if (negative !== (s.set === "expected-negative"))
        errors.push(
          `${at(s, i)}: ${op.op} ${negative ? "is expected-invalid and must be in" : "must not be in"} a scenario with set "expected-negative"`,
        );
      if (op.op === "edit-managed-css" && profile.fixture.managedCss !== true)
        errors.push(`${at(s, i)}: edit-managed-css needs fixture.managedCss: true`);
      if (op.op === "edit-public-css" && op.expect !== "classify")
        errors.push(
          `${at(s, i)}: edit-public-css must declare expect: "classify" (public CSS may have no auto-update contract)`,
        );
      if (op.op === "hidden-edit") {
        const hidden = `/articles/${op.article}/`;
        if (visited.has(hidden) || op.targetPath === hidden)
          errors.push(
            `${at(s, i)}: hidden-edit article ${op.article} must not be visited earlier in its scenario (warm order or target)`,
          );
      }
      if (
        op.op === "rapid-sequence" &&
        (op.variant ?? "title-then-category") === "add-then-delete" &&
        op.writeMethod &&
        op.writeMethod !== "create"
      )
        errors.push(
          `${at(s, i)}: rapid-sequence add-then-delete uses writeMethod "create" (then unlink)`,
        );
      if (op.inject) errors.push(`${at(s, i)}: inject is not supported for field-notes ops`);
      if (op.targetPath) visited.add(op.targetPath);
      if (op.article) visited.add(`/articles/${op.article}/`);
      if (op.article && (op.op === "rename-slug-change" || op.op === "rename-filename-only"))
        visited.add(`/articles/${op.to ?? `${op.article}-renamed`}/`);
    }
  }
  return errors;
}

export function semanticErrors(profile) {
  const errors = [];
  const kind = profile.fixture?.kind ?? "tiny";
  if (kind === "field-notes") errors.push(...fieldNotesErrors(profile));
  else if (!profile.fixture?.placement)
    errors.push("$.fixture.placement is required for kind tiny");
  if (kind !== "field-notes")
    for (const s of profile.scenarios ?? [])
      for (const [i, op] of (s.ops ?? []).entries())
        if (!TINY_OPS.has(op.op))
          errors.push(
            `$.scenarios[${s.id}].ops[${i}]: ${op.op} is a field-notes op, not a tiny op`,
          );
  if (profile.server?.port?.mode === "fixed" && !profile.server.port.port)
    errors.push("$.server.port.port required for fixed mode");
  if (UNSPECIFIED_HOSTS.has(String(profile.server?.host ?? "").trim()))
    errors.push(
      `$.server.host must be an explicit address (got ${JSON.stringify(profile.server?.host)}): zfb prints a multi-line Local:/Network: banner for unspecified hosts, which the ready parser does not read`,
    );
  for (const k of DIAGNOSTIC_ENV) {
    if (profile.server?.env && k in profile.server.env && profile.diagnostic !== true)
      errors.push(`$.server.env.${k} is only allowed in a profile with diagnostic: true`);
  }
  for (const s of kind === "field-notes" ? [] : (profile.scenarios ?? [])) {
    for (const [i, op] of (s.ops ?? []).entries()) {
      if (NEEDS_ARTICLE.has(op.op) && !op.article)
        errors.push(`$.scenarios[${s.id}].ops[${i}]: ${op.op} needs article`);
      if (op.inject && s.set !== "negative-controls")
        errors.push(
          `$.scenarios[${s.id}].ops[${i}]: inject "${op.inject}" is only allowed in a scenario with set "negative-controls"`,
        );
      if (op.inject === "restore-blocked" && profile.restoration?.writeMethod !== "in-place")
        errors.push(
          `$.scenarios[${s.id}].ops[${i}]: inject "restore-blocked" requires restoration.writeMethod "in-place" (a rename restore is not blocked by the target's mode)`,
        );
      if (
        op.inject === "write-error" &&
        !["in-place", "atomic-rename"].includes(op.writeMethod ?? OP_WRITE_METHODS[op.op]?.[0])
      )
        errors.push(
          `$.scenarios[${s.id}].ops[${i}]: inject "write-error" requires writeMethod "in-place" or "atomic-rename" (${op.writeMethod ?? "default"} would create the redirected path)`,
        );
      const allowed = OP_WRITE_METHODS[op.op];
      if (allowed && op.writeMethod && !allowed.includes(op.writeMethod))
        errors.push(
          `$.scenarios[${s.id}].ops[${i}]: ${op.op} does not allow writeMethod ${op.writeMethod} (allowed: ${allowed.join(", ")})`,
        );
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
      sourceDir: profile.fixture.source?.dir ? r(profile.fixture.source.dir) : null,
      outDir,
      lockPath: profile.lockPath ? r(profile.lockPath) : join(outDir, ".dev-browser-observe.lock"),
      registryPath: join(outDir, ".dev-browser-observe-owned.jsonl"),
    },
  };
}
