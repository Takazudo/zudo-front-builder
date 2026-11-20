#!/usr/bin/env node
import { existsSync } from "node:fs";
import { resolve } from "node:path";
import { OBSERVER_PATH } from "./lib/browser.mjs";
import { fixtureKind, kindFor } from "./lib/fixture-kinds.mjs";
import { sourceTreeHashes } from "./lib/fieldnotes.mjs";
import { currentPins, recordBuild } from "./lib/identity.mjs";
import { holdLock, lockContentionCheck } from "./lib/lock-check.mjs";
import { probeMac } from "./lib/mac-process-adapter.mjs";
import { loadProfile } from "./lib/profile.mjs";
import { EXIT, execute, notImplementedResult, verifyIdentityOnly } from "./lib/run.mjs";
import { SCHEMA_FILES, loadSchema, validateAs } from "./lib/schema.mjs";
import { HARNESS_VERSION, writeJsonSync } from "./lib/util.mjs";

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
                               is recorded in records.jsonl and receipt.json.

Stop: SIGINT to the server group, SIGTERM after timeouts.stopTermMs, SIGKILL after
timeouts.stopKillMs; the signal that ended each owned process is recorded.
outDir must be outside every git work tree.

Outputs (run/calibrate): <outDir>/<label>/records.jsonl (append + fsync per record),
receipt.json, summary.json, server.log. A label is never reused.

Exit codes: 0 ok (a batch stopped only by a declared negative control, or by an
expect:"classify" timeout recorded as timeout, counts as ok),
1 batch stopped / calibration failure, 2 usage, 3 identity rejected,
4 not implemented (fixed-distribution), 5 lock busy, 6 owned server still alive,
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
      const pins = currentPins({
        fixtureDir: resolved.fixtureDir,
        observerPath: OBSERVER_PATH,
        adapterPath: kind.adapterPath(resolved.fixtureDir, manifest, profile.fixture.adapter),
      });
      if (fixtureKind(profile) === "field-notes") {
        print({
          pins,
          source: {
            dir: profile.fixture.source.dir,
            ...sourceTreeHashes(resolved.sourceDir, manifest),
          },
        });
      } else print(pins);
      return EXIT.ok;
    }
    case "verify-identity": {
      const { profile } = loadProfile(need(args, "profile"));
      if (profile.mode === "fixed-distribution") {
        print(notImplementedResult(profile));
        return EXIT.notImplemented;
      }
      const verdict = verifyIdentityOnly(args.profile);
      print({
        ok: verdict.ok,
        checks: verdict.checks,
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
      return execute({
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
