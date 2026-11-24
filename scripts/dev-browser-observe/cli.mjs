#!/usr/bin/env node
import { existsSync } from "node:fs";
import { join, resolve } from "node:path";
import { AGGREGATE_SCHEMA, DECLARABLE_FACETS, aggregate } from "./lib/aggregate.mjs";
import { OBSERVER_PATH } from "./lib/browser.mjs";
import {
  FRAMEWORKS,
  distributionChecks,
  distributionOf,
  fixedHelperPath,
  installDistribution,
} from "./lib/distribution.mjs";
import { fixtureKind, kindFor } from "./lib/fixture-kinds.mjs";
import {
  FN_SCALE_COUNTS,
  fnScale,
  makeFieldNotesSource,
  sourceTreeHashes,
} from "./lib/fieldnotes.mjs";
import { freezeHarness, verifyFrozen } from "./lib/frozen.mjs";
import { currentPins, recordBuild } from "./lib/identity.mjs";
import { holdLock, lockContentionCheck } from "./lib/lock-check.mjs";
import { probeMac } from "./lib/mac-process-adapter.mjs";
import { loadProfile } from "./lib/profile.mjs";
import { EXIT, findGitWorkTree, verifyIdentityOnly } from "./lib/run.mjs";
import { superviseSession } from "./lib/session-supervisor.mjs";
import { PROTOCOLS } from "./lib/schedule.mjs";
import { aggregateInputFromRecord, runSeries } from "./lib/series.mjs";
import { SCHEMA_FILES, loadSchema, validateAs } from "./lib/schema.mjs";
import { HARNESS_DIR, HARNESS_VERSION, writeJsonSync } from "./lib/util.mjs";

export const HELP = `dev-browser-observe (${HARNESS_VERSION}) - measure whether \`zfb dev\` updates an
open Chromium page after a content file edit, with fail-stop semantics.

Usage: node scripts/dev-browser-observe/cli.mjs <command> [options]

Commands:
  help, --help                 Show this help.
  schema [name]                Print a JSON schema (${Object.keys(SCHEMA_FILES).join(", ")}).
                               Without a name, list the schema names.
  record-build                 Write a build receipt for a just-built zfb binary.
      --exe <path>             The executable runs will use (required); may be a
                               frozen copy outside the target dir.
      --vendor-dir <path>      Embedded vendor copy to bind (default: the newest
                               target-dir build-script out/vendor).
      --build-exe <path>       Original build output (default: <target-dir>/
                               [<triple>/]<profile-dir>/zfb when present).
      --profile <name>         Cargo profile used (required: dev, release, ...).
      --out <path>             Receipt path; never overwritten (required).
      --repo <path>            Source checkout (default: this harness's checkout).
      --target-dir <path>      Cargo target dir (default: <repo>/target).
      --triple <triple>        Explicit --target triple, if one was passed to cargo.
      --features <a,b>         Features passed to cargo.
      --no-default-features    Record that default features were disabled.
      --toolchain <name>       RUSTUP_TOOLCHAIN used for the build.
      --build-command <text>   The exact cargo command, recorded verbatim.
  pins --profile <p.json>      Print the current harness/fixture/observer/adapter
                               hashes to pin in the profile's "pins".
  verify-identity --profile <p.json>
                               Check identity only (no lock, server, browser or edits).
  probe-mac                    Print which ps fields work on this host (JSON).
  protocols                    Print the series protocol ids and their fixed constants (JSON).
  hold-lock --profile <p.json> [--ms N]
                               Hold the profile's batch lock (prints LOCKED) until
                               N ms pass (default 60000) or stdin closes.
  lock-contention-check --profile <p.json>
                               Negative control: a hold-lock child holds the lock and
                               a second \`run\` must exit 5 without a label dir.
  calibrate --profile <p.json> --label <unique>
                               Run calibration cells through the same observer.
  run --profile <p.json> --label <unique> [--scenario id[,id]]
                               Run the profile's scenarios (fail-stop). --scenario runs
                               only the named scenarios, in profile order; the selection
                               is recorded in records.jsonl and receipt.json. An op with
                               repeat { warmups, measured } runs that many times in a row
                               (warmups recorded, excluded from stats); a profile with
                               resourceStudy runs its one scenario as restore cycles.
                               A profile with mode "fixed-distribution" (schema
                               profile-fixed-distribution) runs a pinned zfb 6.0.1, astro
                               7.4.12 or next 16.4.6 install from install-distribution; its
                               install, lockfile, executables, helper and corpus are checked
                               before any edit (verify-identity runs the same checks).
  make-source --archive <dir> --scale <100|1000|5000> --out <dir> [--fixture-dir <dir>]
                               Build a Field Notes source dir for a declared scale: copy
                               the archive's apps/, shared/ and scripts/ to a new --out
                               (outside any git work tree), check generate.mjs's sha256,
                               run it at that count and compare the tree/dataset hashes
                               with fixtures/field-notes/scales.json. Prints the pins.
  series --plan <series.json>  Run a series (schema "series"): preflight identity of every
                               variant, then one fresh run (server + browser) per session
                               in plan order; the first non-zero session stops the series
                               and the rest are not-attempted. Writes
                               <outDir>/<labelPrefix>.series.json.
  aggregate (--series <record.json> | --sessions V=<labelDir>[,V=<labelDir>...])
            [--declared-differences build[,toolchain]] [--out <report.json>]
                               Per-session block stats and pooled per-variant/op stats
                               (planned/attempted/success/stale/timeout/error/notAttempted,
                               n/min/median/nearest-rank p95/max, conditional on success,
                               warmups excluded) plus the identity comparison (schema
                               "aggregate"). Exit 3 identity mismatch, 1 a session
                               incomplete, 0 otherwise. --out is never overwritten.
  install-distribution --framework <zfb|astro|next> --source <scale source dir> --out <dir>
            [--npm-cli <npm-cli.js>] [--dry-run] [--fixture-dir <dir>]
                               fixed-distribution install: copy <source>/apps/<fw> (checked
                               against fixtures/field-notes/distributions.json) to a new --out
                               outside any git work tree, run \`npm ci --no-audit --no-fund\`
                               there with this node, verify the result and write
                               <out>/install-receipt.json. --dry-run prints the plan only.
                               Exit 0 ok, 3 installed but not the declared tree, 1 npm failed.
  verify-distribution --framework <fw> --install-dir <dir> [--fixture-dir <dir>]
                               Recheck an install against its receipt and the declarations
                               (lockfile, pinned versions, hidden-lockfile versions and
                               integrity, executables, node binary, node_modules tree).
  freeze --out <dir> [--allow-dirty]
                               Copy this harness to <dir>/dev-browser-observe with a
                               .frozen.json stamp (tree hashes, source sha, the linked
                               @playwright/test), read-only. Run series from its cli.mjs.
  verify-frozen [--dir <frozen harness dir>]
                               Recheck a frozen copy (default: this harness) against its stamp.

Stop: SIGINT to the server group, SIGTERM after timeouts.stopTermMs, SIGKILL after
timeouts.stopKillMs; the signal that ended each owned process is recorded.
outDir must be outside every git work tree.

Protocols: ${Object.keys(PROTOCOLS).join(", ")}; \`protocols\` prints their constants.

Outputs (run/calibrate): <outDir>/<label>/records.jsonl (append + fsync per record),
receipt.json, summary.json, server.log. A label is never reused.

Exit codes: 0 ok (a batch stopped only by a declared negative control, or by an
expect:"classify" timeout recorded as timeout, counts as ok),
1 batch stopped / calibration failure, 2 usage, 3 identity rejected,
4 not implemented (unused since fixed-distribution was implemented), 5 lock busy, 6 owned server still alive,
7 server not ready, 8 controller error.

Timing endpoint: "predicate confirmed + 2 rAF + binding delivery", measured on one
controller clock (process.hrtime.bigint): t0 immediately before the write syscall,
t1 at binding receipt. No paint claim; browser clocks are never subtracted.
`;

export function parseArgs(argv) {
  const out = { _: [] };
  for (let i = 0; i < argv.length; i += 1) {
    const a = argv[i];
    if (a.startsWith("--")) {
      const key = a.slice(2);
      const next = argv[i + 1];
      if (next === undefined || next.startsWith("--")) out[key] = true;
      else {
        out[key] = next;
        i += 1;
      }
    } else out._.push(a);
  }
  return out;
}

function print(value) {
  process.stdout.write(`${typeof value === "string" ? value : JSON.stringify(value, null, 2)}\n`);
}

function need(args, key) {
  if (typeof args[key] !== "string") {
    process.stderr.write(`missing --${key}\n`);
    process.exit(EXIT.usage);
  }
  return args[key];
}

export async function main(argv) {
  const args = parseArgs(argv);
  const cmd = args._[0] ?? (args.help ? "help" : undefined);
  switch (cmd) {
    case undefined:
    case "help":
      print(HELP);
      return EXIT.ok;
    case "schema": {
      const name = args._[1];
      if (!name) {
        print(Object.keys(SCHEMA_FILES));
        return EXIT.ok;
      }
      if (!SCHEMA_FILES[name]) {
        process.stderr.write(`unknown schema ${name}\n`);
        return EXIT.usage;
      }
      print(loadSchema(name));
      return EXIT.ok;
    }
    case "record-build": {
      const out = resolve(need(args, "out"));
      if (existsSync(out)) {
        process.stderr.write(`refusing to overwrite ${out}\n`);
        return EXIT.usage;
      }
      const receipt = recordBuild({
        exe: need(args, "exe"),
        profile: need(args, "profile"),
        repo: args.repo ?? process.cwd(),
        targetDir: args["target-dir"],
        vendorDir: args["vendor-dir"],
        buildExe: args["build-exe"],
        triple: args.triple,
        features: typeof args.features === "string" ? args.features.split(",").filter(Boolean) : [],
        noDefaultFeatures: Boolean(args["no-default-features"]),
        toolchain: args.toolchain,
        buildCommand: args["build-command"],
      });
      const errors = validateAs("build-receipt", receipt);
      writeJsonSync(out, receipt);
      print({
        out,
        exeSha256: receipt.exe.sha256,
        sourceSha: receipt.source.sha,
        dirty: receipt.source.dirty,
        vendorMatchesExpected: receipt.vendor.matchesExpected,
        embeddedMissing: receipt.embeddedBytes.missing.length,
        schemaErrors: errors,
      });
      return receipt.vendor.matchesExpected &&
        receipt.embeddedBytes.missing.length === 0 &&
        errors.length === 0
        ? EXIT.ok
        : EXIT.identityRejected;
    }
    case "pins": {
      const { profile, resolved } = loadProfile(need(args, "profile"));
      const kind = kindFor(profile);
      const manifest = kind.loadManifest(resolved.fixtureDir);
      const entry =
        profile.mode === "fixed-distribution"
          ? distributionOf(resolved.fixtureDir, profile.framework, manifest)
          : null;
      const pins = currentPins({
        fixtureDir: resolved.fixtureDir,
        observerPath: OBSERVER_PATH,
        adapterPath: entry
          ? fixedHelperPath({
              fixtureDir: resolved.fixtureDir,
              manifest,
              entry,
              sourceDir: resolved.sourceDir,
              adapter: profile.fixture.adapter,
            })
          : kind.adapterPath(resolved.fixtureDir, manifest, profile.fixture.adapter),
      });
      if (fixtureKind(profile) === "field-notes") {
        const scale = fnScale(resolved.fixtureDir, manifest, profile.fixture.scale ?? null);
        print({
          pins,
          source: {
            dir: profile.fixture.source.dir,
            ...sourceTreeHashes(resolved.sourceDir, manifest),
          },
          scale: {
            scale: scale.scale,
            id: scale.id,
            articleCount: scale.articleCount,
            declared: Object.fromEntries(
              ["appsZfbTreeSha256", "contentTreeSha256", "sharedTreeSha256", "datasetSha256"].map(
                (k) => [k, scale[k]],
              ),
            ),
          },
        });
      } else print(pins);
      return EXIT.ok;
    }
    case "verify-identity": {
      const verdict = verifyIdentityOnly(need(args, "profile"));
      print({
        ok: verdict.ok,
        checks: verdict.checks,
        ...(verdict.facts?.framework ? { distribution: verdict.facts } : {}),
        receiptError: verdict.receiptError,
        harnessCapability: verdict.harnessCapability,
      });
      return verdict.ok ? EXIT.ok : EXIT.identityRejected;
    }
    case "hold-lock": {
      const ms =
        args.ms === undefined
          ? 60000
          : typeof args.ms === "string" && args.ms.trim() !== ""
            ? Number(args.ms)
            : NaN;
      // setTimeout caps delays at 2^31-1 ms; larger values would fire immediately.
      if (!Number.isFinite(ms) || ms < 0 || ms > 2147483647) {
        process.stderr.write(
          "--ms needs a finite number of milliseconds between 0 and 2147483647\n",
        );
        return EXIT.usage;
      }
      return holdLock(need(args, "profile"), ms);
    }
    case "lock-contention-check": {
      const r = await lockContentionCheck(need(args, "profile"));
      print(r);
      return r.ok ? EXIT.ok : EXIT.stopped;
    }
    case "make-source": {
      const scale = Number(need(args, "scale"));
      if (!FN_SCALE_COUNTS.includes(scale)) {
        process.stderr.write(`--scale must be one of ${FN_SCALE_COUNTS.join(", ")}\n`);
        return EXIT.usage;
      }
      const out = resolve(need(args, "out"));
      const git = findGitWorkTree(out);
      if (git) {
        process.stderr.write(`--out ${out} is inside the git work tree ${git}\n`);
        return EXIT.usage;
      }
      if (existsSync(out)) {
        process.stderr.write(`refusing to overwrite ${out}\n`);
        return EXIT.usage;
      }
      const r = await makeFieldNotesSource({
        fixtureDir: resolve(args["fixture-dir"] ?? join(HARNESS_DIR, "fixtures", "field-notes")),
        archiveDir: resolve(need(args, "archive")),
        scale,
        outDir: out,
      });
      print({ ...r, generator: { ...r.generator, stdout: r.generator.stdout.trim() } });
      return r.ok ? EXIT.ok : EXIT.identityRejected;
    }
    case "series": {
      const { exitCode, record } = await runSeries({ planPath: need(args, "plan") });
      if (record)
        print({
          record: join(record.outDir, `${record.plan.labelPrefix}.series.json`),
          exitCode,
          sessions: record.sessions.map((x) => ({
            label: x.label,
            variant: x.variant,
            status: x.status,
            exitCode: x.exitCode,
          })),
        });
      return exitCode;
    }
    case "aggregate": {
      const out = typeof args.out === "string" ? resolve(args.out) : null;
      if (out && existsSync(out)) {
        process.stderr.write(`refusing to overwrite ${out}\n`);
        return EXIT.usage;
      }
      let input;
      if (typeof args.series === "string") {
        input = aggregateInputFromRecord(resolve(args.series));
      } else if (typeof args.sessions === "string") {
        const sessions = args.sessions.split(",").map((pair, index) => {
          const at = pair.indexOf("=");
          const variant = pair.slice(0, at);
          const dir = resolve(pair.slice(at + 1));
          return { index, variant, dir, label: dir.split("/").filter(Boolean).at(-1) };
        });
        if (sessions.some((x) => !/^[A-Za-z0-9]+$/.test(x.variant))) {
          process.stderr.write("--sessions needs V=<labelDir> pairs (variant: [A-Za-z0-9]+)\n");
          return EXIT.usage;
        }
        const declared =
          typeof args["declared-differences"] === "string"
            ? args["declared-differences"].split(",").filter(Boolean)
            : [];
        const bad = declared.filter((d) => !DECLARABLE_FACETS.includes(d));
        if (bad.length) {
          process.stderr.write(
            `--declared-differences: ${bad.join(", ")} cannot differ (declarable: ${DECLARABLE_FACETS.join(", ")})\n`,
          );
          return EXIT.usage;
        }
        input = { sessions, declaredDifferences: declared, protocol: null, series: null };
      } else {
        process.stderr.write("aggregate needs --series <record.json> or --sessions V=<dir>,...\n");
        return EXIT.usage;
      }
      const { report, exitCode } = aggregate(input);
      const errors = validateAs("aggregate", report);
      if (out) writeJsonSync(out, report);
      print({
        schema: AGGREGATE_SCHEMA,
        out,
        exitCode,
        identityOk: report.identity.ok,
        failedChecks: report.identity.checks.filter((c) => !c.ok).map((c) => c.name),
        complete: report.complete,
        incompleteSessions: report.incompleteSessions,
        schemaErrors: errors,
        ...(out ? {} : { report }),
      });
      return exitCode;
    }
    case "install-distribution": {
      const framework = need(args, "framework");
      if (!FRAMEWORKS.includes(framework)) {
        process.stderr.write(`--framework must be one of ${FRAMEWORKS.join(", ")}\n`);
        return EXIT.usage;
      }
      const out = resolve(need(args, "out"));
      const git = findGitWorkTree(out);
      if (git) {
        process.stderr.write(`--out ${out} is inside the git work tree ${git}\n`);
        return EXIT.usage;
      }
      if (!args["dry-run"] && existsSync(out)) {
        process.stderr.write(`refusing to overwrite ${out}\n`);
        return EXIT.usage;
      }
      const r = installDistribution({
        fixtureDir: resolve(args["fixture-dir"] ?? join(HARNESS_DIR, "fixtures", "field-notes")),
        framework,
        sourceDir: resolve(need(args, "source")),
        outDir: out,
        ...(typeof args["npm-cli"] === "string" ? { npmCli: resolve(args["npm-cli"]) } : {}),
        dryRun: Boolean(args["dry-run"]),
      });
      if (r.dryRun) {
        print(r);
        return EXIT.ok;
      }
      print({
        receipt: join(out, "install-receipt.json"),
        ok: r.ok,
        exitCode: r.exitCode,
        node: r.node,
        npm: r.npm,
        failed: r.verification?.checks.filter((c) => !c.ok).map((c) => c.name) ?? null,
        stderrTail: r.ok ? undefined : r.stderrTail,
      });
      return r.ok ? EXIT.ok : r.exitCode === 0 ? EXIT.identityRejected : EXIT.stopped;
    }
    case "verify-distribution": {
      const framework = need(args, "framework");
      if (!FRAMEWORKS.includes(framework)) {
        process.stderr.write(`--framework must be one of ${FRAMEWORKS.join(", ")}\n`);
        return EXIT.usage;
      }
      const fixtureDir = resolve(
        args["fixture-dir"] ?? join(HARNESS_DIR, "fixtures", "field-notes"),
      );
      const r = distributionChecks({
        installDir: resolve(need(args, "install-dir")),
        entry: distributionOf(fixtureDir, framework),
      });
      print({
        ok: r.ok,
        failed: r.checks.filter((c) => !c.ok).map((c) => c.name),
        checks: r.checks,
        facts: r.verification?.facts ?? null,
      });
      return r.ok ? EXIT.ok : EXIT.identityRejected;
    }
    case "freeze": {
      const r = freezeHarness({
        outDir: need(args, "out"),
        allowDirty: Boolean(args["allow-dirty"]),
      });
      print({ dir: r.dir, cli: r.cli, stamp: r.stamp });
      return EXIT.ok;
    }
    case "verify-frozen": {
      const r = verifyFrozen(resolve(typeof args.dir === "string" ? args.dir : HARNESS_DIR));
      print({ ok: r.ok, dir: r.dir, checks: r.checks });
      return r.ok ? EXIT.ok : EXIT.identityRejected;
    }
    case "protocols":
      print(PROTOCOLS);
      return EXIT.ok;
    case "probe-mac":
      print(probeMac());
      return EXIT.ok;
    case "run":
    case "calibrate":
      if (
        args.scenario === true ||
        (typeof args.scenario === "string" && args.scenario.trim() === "")
      ) {
        process.stderr.write("--scenario needs one or more scenario ids (comma-separated)\n");
        return EXIT.usage;
      }
      return superviseSession({
        profilePath: need(args, "profile"),
        label: need(args, "label"),
        command: cmd,
        scenarios:
          typeof args.scenario === "string"
            ? args.scenario
                .split(",")
                .map((x) => x.trim())
                .filter(Boolean)
            : null,
      });
    default:
      process.stderr.write(`unknown command ${cmd}\n${HELP}`);
      return EXIT.usage;
  }
}

if (
  import.meta.url === `file://${process.argv[1]}` ||
  process.argv[1]?.endsWith("dev-browser-observe/cli.mjs")
) {
  main(process.argv.slice(2)).then(
    (code) => {
      process.exitCode = code;
    },
    (error) => {
      process.stderr.write(`${error?.stack ?? error}\n`);
      process.exitCode = error?.code === "PROFILE_INVALID" ? EXIT.usage : EXIT.controllerError;
    },
  );
}
