import { existsSync, mkdirSync, readFileSync } from "node:fs";
import { dirname, isAbsolute, join, resolve } from "node:path";
import { DECLARABLE_FACETS, PROFILE_VARIANT_KEYS } from "./aggregate.mjs";
import { LABEL_RE, loadProfile } from "./profile.mjs";
import { EXIT, execute, verifyIdentityOnly } from "./run.mjs";
import {
  PROTOCOLS,
  RESOURCE_STUDY_PROTOCOL,
  TIMING_ABBA_PROTOCOL,
  expandSchedule,
} from "./schedule.mjs";
import { validateAs } from "./schema.mjs";
import { isoNow, readJson, sha256, writeJsonSync } from "./util.mjs";

export const SERIES_SCHEMA = "dev-browser-observe.series.v1";
export const SERIES_RECORD_SCHEMA = "dev-browser-observe.series-record.v1";

const canon = (v) =>
  JSON.stringify(v, (_k, x) =>
    x && typeof x === "object" && !Array.isArray(x)
      ? Object.fromEntries(
          Object.keys(x)
            .sort()
            .map((k) => [k, x[k]]),
        )
      : x,
  );

export function sessionLabel(plan, index, variant) {
  return `${plan.labelPrefix}-s${index + 1}-${variant}`;
}

/** True when order is X,Y,Y,X with two distinct variants. */
export function isAbba(order) {
  return (
    order.length === 4 && order[0] === order[3] && order[1] === order[2] && order[0] !== order[1]
  );
}

/**
 * Load a series plan and everything it names, and check it without running anything:
 * schema, protocol constants, profiles equal except the build they run, one outDir and lock,
 * and declared differences. Returns { plan, path, sha256, variants, sessions, errors }.
 */
export function loadSeriesPlan(planPath) {
  const path = resolve(planPath);
  const raw = readFileSync(path);
  const plan = JSON.parse(raw.toString("utf8"));
  const errors = validateAs("series", plan);
  const out = { plan, path, sha256: sha256(raw), variants: {}, sessions: [], errors };
  if (errors.length) return out;
  const base = dirname(path);
  const names = Object.keys(plan.variants);
  for (const v of plan.order) if (!names.includes(v)) errors.push(`$.order: unknown variant ${v}`);
  for (const v of names)
    if (!plan.order.includes(v)) errors.push(`$.variants.${v}: never scheduled in order`);
  const declared = plan.declaredDifferences ?? [];
  for (const d of declared)
    if (!DECLARABLE_FACETS.includes(d))
      errors.push(
        `$.declaredDifferences: ${d} cannot differ (declarable: ${DECLARABLE_FACETS.join(", ")})`,
      );
  for (const [v, spec] of Object.entries(plan.variants)) {
    const p = isAbsolute(spec.profile) ? spec.profile : resolve(base, spec.profile);
    try {
      const loaded = loadProfile(p);
      const selected = plan.scenarios
        ? {
            ...loaded.profile,
            scenarios: loaded.profile.scenarios.filter((s) => plan.scenarios.includes(s.id)),
          }
        : loaded.profile;
      const missing = (plan.scenarios ?? []).filter(
        (id) => !loaded.profile.scenarios.some((s) => s.id === id),
      );
      if (missing.length)
        errors.push(`$.variants.${v}: profile lacks scenarios ${missing.join(", ")}`);
      out.variants[v] = {
        profilePath: loaded.path,
        profileSha256: loaded.sha256,
        loaded,
        selected,
      };
    } catch (error) {
      errors.push(`$.variants.${v}: ${error?.message ?? error}`);
    }
  }
  if (errors.length) return out;
  const vs = Object.values(out.variants);
  const strip = (prof) =>
    Object.fromEntries(Object.entries(prof).filter(([k]) => !PROFILE_VARIANT_KEYS.includes(k)));
  const ref = canon(strip(vs[0].loaded.profile));
  for (const [v, x] of Object.entries(out.variants)) {
    if (canon(strip(x.loaded.profile)) !== ref)
      errors.push(
        `$.variants.${v}: profile differs from ${names[0]} outside ${PROFILE_VARIANT_KEYS.join(", ")}`,
      );
    if (x.loaded.resolved.outDir !== vs[0].loaded.resolved.outDir)
      errors.push(`$.variants.${v}: every variant must use the same outDir`);
    if (x.loaded.resolved.lockPath !== vs[0].loaded.resolved.lockPath)
      errors.push(`$.variants.${v}: every variant must use the same lock`);
  }
  const builds = Object.fromEntries(
    Object.entries(out.variants).map(([v, x]) => {
      try {
        return [v, readJson(x.loaded.resolved.buildReceipt)];
      } catch {
        return [v, null];
      }
    }),
  );
  const facet = {
    build: (b) =>
      b && { exe: b.exe?.sha256, source: b.source?.sha, dirty: b.source?.dirtyIdentitySha256 },
    toolchain: (b) => b?.toolchain ?? null,
  };
  for (const [name, get] of Object.entries(facet)) {
    if (declared.includes(name)) continue;
    const vals = new Set(Object.values(builds).map((b) => canon(get(b))));
    if (vals.size > 1)
      errors.push(
        `build receipts differ in ${name}, which the plan does not declare as a difference`,
      );
  }
  const proto = PROTOCOLS[plan.protocol];
  for (const [v, x] of Object.entries(out.variants)) {
    const prof = x.selected;
    if ((prof.fixture.kind ?? "tiny") !== "field-notes")
      errors.push(`$.variants.${v}: ${plan.protocol} needs a field-notes profile`);
    if (plan.protocol === TIMING_ABBA_PROTOCOL) {
      if (prof.resourceStudy)
        errors.push(`$.variants.${v}: a timing series cannot run a resource study`);
      for (const s of prof.scenarios)
        for (const op of s.ops) {
          const at = `$.variants.${v}: scenario ${s.id} op ${op.id ?? op.op}`;
          if (!proto.ops.includes(op.op))
            errors.push(`${at}: ${op.op} is not a ${plan.protocol} op (${proto.ops.join(", ")})`);
          if (op.repeat?.warmups !== proto.warmups || op.repeat?.measured !== proto.measured)
            errors.push(
              `${at}: repeat must be { warmups: ${proto.warmups}, measured: ${proto.measured} }`,
            );
          if ((op.expect ?? "success") !== "success")
            errors.push(`${at}: timing ops expect success`);
        }
    } else if (plan.protocol === RESOURCE_STUDY_PROTOCOL) {
      if (!prof.resourceStudy) errors.push(`$.variants.${v}: needs resourceStudy`);
      try {
        expandSchedule(prof);
      } catch (error) {
        errors.push(`$.variants.${v}: ${error?.message ?? error}`);
      }
    }
  }
  if (plan.protocol === TIMING_ABBA_PROTOCOL && !isAbba(plan.order))
    errors.push(`$.order: ${plan.protocol} needs an X,Y,Y,X order of two variants`);
  out.sessions = plan.order.map((variant, index) => ({
    index,
    variant,
    label: sessionLabel(plan, index, variant),
  }));
  for (const s of out.sessions)
    if (!LABEL_RE.test(s.label)) errors.push(`session label ${s.label} does not match ${LABEL_RE}`);
  out.outDir = vs[0].loaded.resolved.outDir;
  out.recordPath = join(out.outDir, `${plan.labelPrefix}.series.json`);
  return out;
}

/**
 * Run a series: identity-check every variant first (nothing runs if any fails), then one fresh
 * `run` per session in plan order. The first session that does not exit 0 stops the series; the
 * remaining sessions are recorded not-attempted. The series record is written next to the label dirs.
 */
export async function runSeries({
  planPath,
  stderr = (m) => process.stderr.write(`${m}\n`),
  executeImpl = execute,
  verifyImpl = verifyIdentityOnly,
}) {
  const s = loadSeriesPlan(planPath);
  if (s.errors.length) {
    stderr(`invalid series plan ${s.path}:\n  ${s.errors.join("\n  ")}`);
    return { exitCode: EXIT.usage, record: null };
  }
  if (existsSync(s.recordPath)) {
    stderr(`series record already exists: ${s.recordPath} (pick a new labelPrefix)`);
    return { exitCode: EXIT.usage, record: null };
  }
  const taken = s.sessions.filter((x) => existsSync(join(s.outDir, x.label)));
  if (taken.length) {
    stderr(`label already used: ${taken.map((x) => x.label).join(", ")}`);
    return { exitCode: EXIT.usage, record: null };
  }
  const record = {
    schema: SERIES_RECORD_SCHEMA,
    planPath: s.path,
    planSha256: s.sha256,
    plan: s.plan,
    protocol: s.plan.protocol,
    constants: PROTOCOLS[s.plan.protocol],
    variants: Object.fromEntries(
      Object.entries(s.variants).map(([v, x]) => [
        v,
        { profile: x.profilePath, profileSha256: x.profileSha256 },
      ]),
    ),
    outDir: s.outDir,
    createdAt: isoNow(),
    preflight: {},
    sessions: s.sessions.map((x) => ({ ...x, status: "planned", exitCode: null })),
    stoppedAt: null,
    exitCode: null,
  };
  mkdirSync(s.outDir, { recursive: true });
  for (const [v, x] of Object.entries(s.variants)) {
    const verdict = verifyImpl(x.profilePath);
    record.preflight[v] = {
      ok: verdict.ok,
      failed: verdict.checks.filter((c) => !c.ok).map((c) => c.name),
    };
  }
  const preflightOk = Object.values(record.preflight).every((p) => p.ok);
  if (!preflightOk) {
    for (const x of record.sessions) x.status = "not-attempted";
    record.exitCode = EXIT.identityRejected;
    record.stoppedAt = { reason: "preflight-identity-rejected" };
    writeJsonSync(s.recordPath, record);
    stderr("series preflight identity rejected; no session ran");
    return { exitCode: EXIT.identityRejected, record };
  }
  writeJsonSync(s.recordPath, record);
  let exitCode = EXIT.ok;
  for (const x of record.sessions) {
    if (exitCode !== EXIT.ok) {
      x.status = "not-attempted";
      continue;
    }
    x.status = "running";
    x.startedAt = isoNow();
    writeJsonSync(s.recordPath, record);
    let code;
    try {
      code = await executeImpl({
        profilePath: s.variants[x.variant].profilePath,
        label: x.label,
        command: "run",
        stderr,
        scenarios: s.plan.scenarios ?? null,
      });
    } catch (error) {
      x.error = String(error?.stack ?? error);
      code = EXIT.controllerError;
    }
    x.exitCode = code;
    x.endedAt = isoNow();
    x.status = code === EXIT.ok ? "completed" : "stopped";
    if (code !== EXIT.ok) {
      exitCode = code;
      record.stoppedAt = { session: x.label, exitCode: code };
    }
    writeJsonSync(s.recordPath, record);
  }
  record.exitCode = exitCode;
  writeJsonSync(s.recordPath, record);
  return { exitCode, record };
}

/** The sessions of a series record, as aggregate() inputs (planned-but-unrun sessions included). */
export function sessionsFromRecord(recordPath) {
  const record = readJson(recordPath);
  return {
    record,
    sessions: record.sessions.map((x) => ({
      index: x.index,
      label: x.label,
      variant: x.variant,
      dir: join(record.outDir, x.label),
    })),
  };
}
