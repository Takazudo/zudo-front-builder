#!/usr/bin/env node
// Linux CI provisioning for the locked Playwright browsers (see the CI section
// of research/development-queue.md, 2026-11-07 checkpoint).
//
// Phases, each recorded in a JSON receipt that is rewritten after every phase:
//   identity  locked Playwright version, host platform, runner/source identity
//   probe     `playwright install-deps --dry-run <engines>` with the current
//             APT state; when it proves every package present, APT is skipped
//   acquire   bounded by elapsed time per attempt: `apt-get update`, a fresh
//             dry-run that names the missing packages, and
//             `apt-get install --download-only` of exactly those packages
//   install   `apt-get install --no-download` from the acquired archives,
//             with the same APT configuration as the acquisition; never retried
//   validate  the dry-run again, which must now report nothing missing
//   browsers  `playwright install <engines>` (the revisions locked by this
//             Playwright version; no system browser or alias is substituted)
//   verify    each registry executable for the engines exists
//   launch    each engine launches headless and renders a page
//
// --total-timeout caps the whole invocation (every phase budget is clipped to
// it), so a workflow can prove the helper exits inside its job deadline.
//
// One acquisition retry is allowed, only after a transport failure or the
// elapsed deadline, only after every apt-get process and method has exited,
// and only against the official Ubuntu archive. That route reuses the runner's
// own source stanzas with just the URIs replaced, in an APT_CONFIG owned by
// this invocation; nothing under /etc/apt is modified.

import { spawn, spawnSync } from "node:child_process";
import fs from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

export const ENGINES = Object.freeze(["chromium", "firefox", "webkit"]);
export const RECEIPT_SCHEMA = "zfb-playwright-linux-provisioning/v1";
export const OFFICIAL_ARCHIVE = "https://archive.ubuntu.com/ubuntu/";
export const OFFICIAL_SECURITY = "https://security.ubuntu.com/ubuntu/";
// actions/runner-images configure-apt-sources.sh rewrites ubuntu.sources to this
// list: azure.archive.ubuntu.com first, then the official archive and security hosts.
export const RUNNER_MIRROR_LIST = "/etc/apt/apt-mirrors.txt";
const OFFICIAL_URI = /^https?:\/\/(?:azure\.archive|archive|security)\.ubuntu\.com\/ubuntu\/?$/;
const PACKAGE_NAME = /^[a-z0-9][a-z0-9+.-]+(?::[a-z0-9]+)?$/;
const INSECURE_OPTIONS = ["trusted", "allow-insecure", "allow-weak", "allow-downgrade-to-insecure"];
// apt's boolean spellings (apt-pkg/contrib/strutl.cc StringToBool).
const APT_TRUE = /^(?:yes|true|with|on|enable)$/i;
const APT_FALSE = /^(?:no|false|without|off|disable)$/i;
const RUNNER_MIRROR_URIS = new Set([
  "mirror+file:/etc/apt/apt-mirrors.txt",
  "mirror+file:///etc/apt/apt-mirrors.txt",
]);
// apt's own wording for failures a different endpoint can plausibly fix.
const TRANSPORT_OUTPUT =
  /Failed to fetch|Could not (?:resolve|connect)|Connection (?:timed out|failed|reset)|Unable to fetch some archives|Temporary failure resolving|Unable to connect|Network is unreachable|No route to host|Error reading from server|Hash Sum mismatch|File has unexpected size|Some index files failed to download/i;
const DRY_RUN_SATISFIED = "All system dependencies are installed.";
const DRY_RUN_MISSING = /^Missing system dependencies \((\d+)\):$/;
const APT_PROCESS = /(?:^|\/)apt-get(?:\s|$)|\/usr\/lib\/apt\/methods\//;

export class StageError extends Error {
  constructor(stage, message, { transport = false } = {}) {
    super(message);
    this.stage = stage;
    this.transport = transport;
  }
}

// ── arguments ─────────────────────────────────────────────────────────────

function positiveSeconds(name, value, max) {
  if (!/^\d+$/.test(String(value ?? "")))
    throw new Error(`${name} must be a whole number of seconds`);
  const seconds = Number(value);
  if (seconds < 1 || seconds > max) throw new Error(`${name} must be between 1 and ${max} seconds`);
  return seconds;
}

export function parseArgs(argv) {
  const values = { engines: [] };
  for (let index = 0; index < argv.length; index += 1) {
    const flag = argv[index];
    const value = argv[index + 1];
    const take = () => {
      if (value === undefined || value.startsWith("--")) throw new Error(`${flag} needs a value`);
      index += 1;
      return value;
    };
    switch (flag) {
      case "--label":
        values.label = take();
        break;
      case "--engines":
        values.engines.push(...take().split(","));
        break;
      case "--acquire-timeout":
        values.acquireTimeout = take();
        break;
      case "--install-timeout":
        values.installTimeout = take();
        break;
      case "--browser-timeout":
        values.browserTimeout = take();
        break;
      case "--total-timeout":
        values.totalTimeout = take();
        break;
      case "--receipt-dir":
        values.receiptDir = take();
        break;
      default:
        throw new Error(`unknown argument: ${flag}`);
    }
  }
  if (!/^[a-z0-9][a-z0-9-]*$/.test(values.label ?? "")) {
    throw new Error("--label is required (lowercase letters, digits and dashes)");
  }
  const engines = values.engines.map((engine) => engine.trim()).filter(Boolean);
  if (engines.length === 0) throw new Error("--engines is required");
  for (const engine of engines) {
    if (!ENGINES.includes(engine)) throw new Error(`unknown engine: ${engine}`);
  }
  if (new Set(engines).size !== engines.length) throw new Error("--engines lists an engine twice");
  return {
    label: values.label,
    engines: ENGINES.filter((engine) => engines.includes(engine)),
    acquireTimeoutMs: positiveSeconds("--acquire-timeout", values.acquireTimeout, 3600) * 1000,
    installTimeoutMs:
      positiveSeconds("--install-timeout", values.installTimeout ?? "300", 3600) * 1000,
    browserTimeoutMs: positiveSeconds("--browser-timeout", values.browserTimeout, 3600) * 1000,
    totalTimeoutMs: positiveSeconds("--total-timeout", values.totalTimeout, 7200) * 1000,
    receiptDir: values.receiptDir,
  };
}

// ── Playwright dry-run output ─────────────────────────────────────────────

export function parseDryRun({ exitCode, stdout, stderr = "" }) {
  const lines = String(stdout ?? "")
    .split("\n")
    .map((line) => line.replace(/\r$/, ""))
    .filter((line) => line.trim() !== "");
  const detail = `exit ${exitCode}; stdout: ${lines.join(" | ").slice(0, 2000)}; stderr: ${String(stderr).trim().slice(-2000)}`;
  if (exitCode === 0 && lines.length === 1 && lines[0].trim() === DRY_RUN_SATISFIED) {
    return { satisfied: true, packages: [] };
  }
  const header = lines[0]?.match(DRY_RUN_MISSING);
  if (exitCode === 1 && header) {
    const count = Number(header[1]);
    const packages = lines.slice(1).map((line) => line.match(/^ {2}(\S+)$/)?.[1]);
    if (
      count > 0 &&
      packages.length === count &&
      packages.every((name) => name && PACKAGE_NAME.test(name)) &&
      new Set(packages).size === count
    ) {
      return { satisfied: false, packages };
    }
  }
  throw new Error(`unrecognised Playwright dependency report (${detail})`);
}

// ── official-archive route ────────────────────────────────────────────────

function officialFor(suite) {
  return /-security$/.test(suite) ? OFFICIAL_SECURITY : OFFICIAL_ARCHIVE;
}

export function parseMirrorList(text) {
  const uris = [];
  for (const raw of String(text).split("\n")) {
    const line = raw.trim();
    if (line === "" || line.startsWith("#")) continue;
    uris.push(line.split(/\s+/)[0]);
  }
  return uris;
}

function makeUriClassifier(mirrorList) {
  const mirrorUris = mirrorList == null ? null : parseMirrorList(mirrorList);
  return (uri) => {
    if (OFFICIAL_URI.test(uri)) return true;
    if (RUNNER_MIRROR_URIS.has(uri)) {
      if (
        !mirrorUris ||
        mirrorUris.length === 0 ||
        !mirrorUris.every((entry) => OFFICIAL_URI.test(entry))
      ) {
        throw new Error(`${RUNNER_MIRROR_LIST} is missing or names a non-official mirror`);
      }
      return true;
    }
    return false;
  };
}

function rewriteOneLine(text, isOfficial, file) {
  let rewritten = 0;
  const out = [];
  for (const line of String(text).split("\n")) {
    const match = line.match(/^(\s*)(deb|deb-src)(\s+\[[^\]]*\])?\s+(\S+)\s+(\S+)(.*)$/);
    if (!match || line.trim().startsWith("#")) continue;
    const [, indent, type, options = "", uri, suite, rest] = match;
    if (!isOfficial(uri)) continue;
    const lowered = options.toLowerCase();
    if (
      INSECURE_OPTIONS.some((option) =>
        new RegExp(`(?:^|[\\s\\[])${option}=(?:yes|true|with|on|enable)(?:[\\s\\]]|$)`).test(
          lowered,
        ),
      )
    ) {
      throw new Error(`${file}: refusing to reroute a source that disables authentication`);
    }
    out.push(`${indent}${type}${options} ${officialFor(suite)} ${suite}${rest}`);
    rewritten += 1;
  }
  return { content: out.length ? `${out.join("\n")}\n` : "", rewritten };
}

function stanzaFields(lines) {
  const fields = [];
  for (const line of lines) {
    if (line.startsWith("#")) {
      fields.push({ key: null, lines: [line] });
    } else if (/^\s/.test(line) && fields.length > 0 && fields.at(-1).key) {
      fields.at(-1).lines.push(line);
    } else {
      const key = line.match(/^([^:\s]+):/)?.[1];
      if (!key) throw new Error(`unparseable deb822 line: ${line}`);
      fields.push({ key, lines: [line] });
    }
  }
  return fields;
}

function fieldValue(field) {
  return field.lines
    .map((line, index) => (index === 0 ? line.slice(line.indexOf(":") + 1) : line))
    .join(" ")
    .trim();
}

function rewriteDeb822(text, isOfficial, file) {
  let rewritten = 0;
  const blocks = String(text).split(/\n[ \t]*\n/);
  const out = blocks.map((block) => {
    const lines = block.split("\n").filter((line) => line !== "");
    const fields = stanzaFields(lines);
    const get = (key) => fields.find((field) => field.key?.toLowerCase() === key.toLowerCase());
    const uris = get("URIs");
    if (!uris) return null;
    if (get("Enabled") && APT_FALSE.test(fieldValue(get("Enabled")))) return null;
    const values = fieldValue(uris).split(/\s+/);
    const official = values.map((uri) => isOfficial(uri));
    if (!official.some(Boolean)) return null;
    if (!official.every(Boolean)) throw new Error(`${file}: stanza mixes official and other URIs`);
    for (const option of INSECURE_OPTIONS) {
      const field = get(option);
      if (field && APT_TRUE.test(fieldValue(field))) {
        throw new Error(`${file}: refusing to reroute a source that disables authentication`);
      }
    }
    const suitesField = get("Suites");
    if (!suitesField) throw new Error(`${file}: stanza has URIs but no Suites`);
    const suites = fieldValue(suitesField).split(/\s+/);
    const groups = new Map();
    for (const suite of suites) {
      const target = officialFor(suite);
      groups.set(target, [...(groups.get(target) ?? []), suite]);
    }
    rewritten += 1;
    return [...groups]
      .map(([target, groupSuites]) =>
        fields
          .flatMap((field) => {
            if (field === uris) return [`URIs: ${target}`];
            if (field === suitesField) return [`Suites: ${groupSuites.join(" ")}`];
            return field.lines;
          })
          .join("\n"),
      )
      .join("\n\n");
  });
  const kept = out.filter(Boolean);
  return { content: kept.length ? `${kept.join("\n\n")}\n` : "", rewritten };
}

// Returns only the runner's own enabled Ubuntu stanzas, with their URIs
// replaced by the official archive/security hosts. Suites, components,
// architectures, Signed-By and every other field are kept verbatim. Other
// sources are left out of the retry so it contacts no other endpoint;
// Playwright's packages all come from Ubuntu's own components.
export function buildOfficialRoute({ sourcesList, parts, mirrorList }) {
  const isOfficial = makeUriClassifier(mirrorList);
  let rewritten = 0;
  const main = sourcesList == null ? "" : rewriteOneLine(sourcesList, isOfficial, "sources.list");
  rewritten += main.rewritten ?? 0;
  const outParts = [];
  for (const part of parts) {
    let result;
    if (part.name.endsWith(".sources")) result = rewriteDeb822(part.content, isOfficial, part.name);
    else if (part.name.endsWith(".list"))
      result = rewriteOneLine(part.content, isOfficial, part.name);
    else continue;
    rewritten += result.rewritten;
    if (result.content) outParts.push({ name: part.name, content: result.content });
  }
  if (rewritten === 0) throw new Error("no allowlisted Ubuntu source stanza to reroute");
  return { sourcesList: main.content ?? "", parts: outParts, rewritten };
}

// ── bounded subprocesses ──────────────────────────────────────────────────

const activeChildren = new Set();

function killGroup(child, signal) {
  try {
    process.kill(-child.pid, signal);
  } catch {
    try {
      child.kill(signal);
    } catch {
      // already gone
    }
  }
}

// Signal 0 probes the group without delivering anything; EPERM means members
// exist that this process may not signal. Unreaped zombies still count.
function groupHasMembers(child) {
  try {
    process.kill(-child.pid, 0);
    return true;
  } catch (error) {
    return error?.code === "EPERM";
  }
}

// Runs a command in its own process group. At the deadline the whole group
// gets SIGTERM, then SIGKILL after the grace period; the promise settles only
// once the child has exited and every holder of its output pipes has closed.
// After a deadline, a group that still has members when the pipes close is
// re-probed until it empties; members left at the end of the grace are
// SIGKILLed before the promise settles.
export function runBounded({
  command,
  args = [],
  env,
  timeoutMs,
  killGraceMs = 15_000,
  echo = true,
}) {
  const started = Date.now();
  return new Promise((resolve) => {
    let tail = "";
    let timedOut = false;
    let killTimer;
    let emptyProbe;
    const child = spawn(command, args, {
      env: env ?? process.env,
      detached: true,
      stdio: ["ignore", "pipe", "pipe"],
    });
    activeChildren.add(child);
    let stdout = "";
    const collect = (chunk, stream) => {
      const text = chunk.toString();
      if (stream === "stdout") stdout += text;
      tail = (tail + text).slice(-8000);
      if (echo) (stream === "stdout" ? process.stdout : process.stderr).write(text);
    };
    child.stdout.on("data", (chunk) => collect(chunk, "stdout"));
    child.stderr.on("data", (chunk) => collect(chunk, "stderr"));
    let killed = false;
    let closed = null;
    const settle = ({ exitCode, signal }) => {
      clearTimeout(deadline);
      clearTimeout(killTimer);
      clearInterval(emptyProbe);
      activeChildren.delete(child);
      resolve({
        exitCode: spawnError ? null : exitCode,
        signal,
        timedOut,
        durationMs: Date.now() - started,
        stdout,
        tail: spawnError ? `${tail}\n${spawnError.message}` : tail,
      });
    };
    const deadline = setTimeout(() => {
      timedOut = true;
      killGroup(child, "SIGTERM");
      killTimer = setTimeout(() => {
        killed = true;
        // Re-probe first: an emptied group's pgid may already belong to another group.
        if (groupHasMembers(child)) killGroup(child, "SIGKILL");
        if (closed) settle(closed);
      }, killGraceMs);
    }, timeoutMs);
    let spawnError = null;
    child.on("error", (error) => {
      spawnError = error;
    });
    child.on("close", (exitCode, signal) => {
      clearTimeout(deadline);
      // A descendant that ignored SIGTERM and released the pipes still belongs
      // to the group: keep its SIGKILL instead of settling past it (#4614).
      // Members still exiting from SIGTERM (or awaiting a reaper) empty the
      // group on their own, so settle as soon as a short re-probe sees it empty.
      if (timedOut && !killed && groupHasMembers(child)) {
        closed = { exitCode, signal };
        emptyProbe = setInterval(() => {
          if (!groupHasMembers(child)) settle(closed);
        }, 25);
      } else settle({ exitCode, signal });
    });
  });
}

export function listProcesses() {
  const result = spawnSync("ps", ["-eo", "pid=,args="], { encoding: "utf8" });
  if (result.status !== 0) throw new Error(`ps failed: ${result.stderr}`);
  return result.stdout
    .split("\n")
    .map((line) => line.trim())
    .filter(Boolean)
    .map((line) => {
      const [pid, ...rest] = line.split(/\s+/);
      return { pid: Number(pid), args: rest.join(" ") };
    });
}

// ── provisioning ──────────────────────────────────────────────────────────

function nodeDeps() {
  const require = createRequire(path.join(process.cwd(), "package.json"));
  const readJson = (file) => JSON.parse(fs.readFileSync(file, "utf8"));
  const lockPath = path.join(process.cwd(), "pnpm-lock.yaml");
  return {
    now: () => Date.now(),
    env: process.env,
    isRoot: typeof process.getuid === "function" && process.getuid() === 0,
    run: runBounded,
    listProcesses,
    sleep: (ms) => new Promise((resolve) => setTimeout(resolve, ms)),
    readText: (file) => (fs.existsSync(file) ? fs.readFileSync(file, "utf8") : null),
    listDir: (dir) => (fs.existsSync(dir) ? fs.readdirSync(dir).sort() : []),
    exists: (file) => fs.existsSync(file),
    makeTempDir: () => {
      const dir = fs.mkdtempSync(
        path.join(process.env.RUNNER_TEMP || os.tmpdir(), "zfb-apt-route-"),
      );
      fs.chmodSync(dir, 0o755);
      return dir;
    },
    writeText: (file, text) => {
      fs.mkdirSync(path.dirname(file), { recursive: true });
      fs.writeFileSync(`${file}.tmp`, text);
      fs.renameSync(`${file}.tmp`, file);
    },
    appendText: (file, text) => fs.appendFileSync(file, text),
    mkdir: (dir) => fs.mkdirSync(dir, { recursive: true, mode: 0o755 }),
    nodePath: process.execPath,
    playwright: () => {
      // Resolved here, inside provision()'s try, so a broken install still
      // produces an identity-stage receipt.
      const testManifestPath = require.resolve("@playwright/test/package.json");
      const playwrightManifestPath =
        createRequire(testManifestPath).resolve("playwright/package.json");
      const coreManifestPath = createRequire(playwrightManifestPath).resolve(
        "playwright-core/package.json",
      );
      const coreDir = path.dirname(coreManifestPath);
      const core = require(coreManifestPath);
      const bundle = createRequire(coreManifestPath)("playwright-core/lib/coreBundle");
      const lockVersions = [
        ...new Set(
          [
            ...fs
              .readFileSync(lockPath, "utf8")
              .matchAll(/^ {2}playwright-core@(\d+\.\d+\.\d+):/gm),
          ].map((match) => match[1]),
        ),
      ];
      return {
        coreVersion: core.version,
        testVersion: readJson(testManifestPath).version,
        playwrightVersion: readJson(playwrightManifestPath).version,
        lockVersions,
        hostPlatform: bundle.utils.hostPlatform,
        officiallySupported: bundle.utils.isOfficiallySupportedPlatform,
        cli: path.join(coreDir, "cli.js"),
        coreDir,
        executables: (engine) =>
          bundle.registry.registry
            .resolveBrowsers([engine], {})
            .filter((executable) => executable.executablePath)
            .map((executable) => ({
              name: executable.name,
              path: executable.executablePath("javascript"),
              browserVersion: executable.browserVersion ?? null,
            })),
      };
    },
    gitHead: () => {
      const result = spawnSync("git", ["rev-parse", "HEAD"], { encoding: "utf8" });
      return result.status === 0 ? result.stdout.trim() : null;
    },
  };
}

function launchScript(coreDir, engine) {
  return [
    `const { ${engine} } = require(${JSON.stringify(coreDir)});`,
    `(async () => {`,
    `  const browser = await ${engine}.launch({ headless: true });`,
    `  const page = await browser.newPage();`,
    `  await page.setContent("<p id=ready>ready</p>");`,
    `  const text = await page.textContent("#ready");`,
    `  await browser.close();`,
    `  if (text !== "ready") throw new Error("unexpected page content: " + text);`,
    `  console.log(${JSON.stringify(engine)} + " " + browser.version() + " launched");`,
    `})().catch((error) => { console.error(error); process.exit(1); });`,
  ].join("\n");
}

export async function provision(options, deps = nodeDeps()) {
  const env = deps.env;
  const receiptDir =
    options.receiptDir ?? path.join(env.RUNNER_TEMP || os.tmpdir(), "playwright-provisioning");
  const receiptPath = path.join(receiptDir, `${options.label}.json`);
  const killGraceMs = options.killGraceMs ?? 15_000;
  const quiescenceMs = options.quiescenceMs ?? 30_000;
  const startedAt = deps.now();
  // Every bounded command may overrun its budget by the kill grace plus the
  // backstop margin, so stop issuing work early enough to still save receipts.
  const totalDeadline = startedAt + options.totalTimeoutMs - killGraceMs - 5_000;
  const within = (stage, ms) => {
    const left = Math.min(ms, totalDeadline - deps.now());
    if (left < 1000) throw new StageError(stage, "total provisioning deadline reached");
    return left;
  };
  const receipt = {
    schema: RECEIPT_SCHEMA,
    label: options.label,
    engines: options.engines,
    limits: {
      acquireTimeoutMs: options.acquireTimeoutMs,
      installTimeoutMs: options.installTimeoutMs,
      browserTimeoutMs: options.browserTimeoutMs,
      totalTimeoutMs: options.totalTimeoutMs,
      maxAcquisitionAttempts: 2,
    },
    source: {
      githubSha: env.GITHUB_SHA ?? null,
      checkoutHead: deps.gitHead(),
      repository: env.GITHUB_REPOSITORY ?? null,
      runId: env.GITHUB_RUN_ID ?? null,
      runAttempt: env.GITHUB_RUN_ATTEMPT ?? null,
      job: env.GITHUB_JOB ?? null,
    },
    runner: {
      imageOs: env.ImageOS ?? null,
      imageVersion: env.ImageVersion ?? null,
      os:
        (deps.readText("/etc/os-release") ?? "").match(/^PRETTY_NAME="?([^"\n]*)"?$/m)?.[1] ?? null,
      arch: process.arch,
    },
    playwright: null,
    dependencies: { reused: false, packages: [], route: null },
    phases: [],
    firstTransportFailure: null,
    result: "running",
    failedStage: null,
    error: null,
    startedAt: new Date(startedAt).toISOString(),
    durationMs: 0,
  };
  const save = () => {
    receipt.durationMs = deps.now() - startedAt;
    deps.writeText(receiptPath, `${JSON.stringify(receipt, null, 2)}\n`);
  };
  const record = (stage, attempt, route, outcome) => {
    const phase = {
      stage,
      attempt,
      route,
      exitCode: outcome.exitCode ?? null,
      signal: outcome.signal ?? null,
      timedOut: Boolean(outcome.timedOut),
      durationMs: outcome.durationMs ?? 0,
      ...(outcome.note ? { note: outcome.note } : {}),
      ...(outcome.exitCode === 0 && !outcome.timedOut
        ? {}
        : { outputTail: (outcome.tail ?? "").slice(-4000) }),
    };
    receipt.phases.push(phase);
    save();
    return phase;
  };
  const sudo = (args, aptConfig, timeoutMs) => {
    const seconds = Math.max(1, Math.ceil(timeoutMs / 1000));
    const command = [
      "env",
      ...(aptConfig ? [`APT_CONFIG=${aptConfig}`] : []),
      "DEBIAN_FRONTEND=noninteractive",
      "timeout",
      `--kill-after=${Math.ceil(killGraceMs / 1000)}s`,
      `${seconds}s`,
      "apt-get",
      ...args,
    ];
    return {
      command: deps.isRoot ? command[0] : "sudo",
      args: deps.isRoot ? command.slice(1) : ["-n", "--", ...command],
      // Backstop only: the root-side timeout above owns the deadline.
      timeoutMs: timeoutMs + killGraceMs + 5_000,
      killGraceMs,
    };
  };
  const aptTimedOut = (outcome, budgetMs) =>
    outcome.timedOut ||
    outcome.exitCode === 124 ||
    (outcome.exitCode === 137 && outcome.durationMs >= budgetMs);
  const ok = (outcome) => outcome.exitCode === 0 && !outcome.timedOut;

  let playwright;
  const dryRun = async (stage, attempt, route, aptConfig, timeoutMs) => {
    const outcome = await deps.run({
      command: deps.nodePath,
      args: [playwright.cli, "install-deps", "--dry-run", ...options.engines],
      env: { ...env, ...(aptConfig ? { APT_CONFIG: aptConfig } : {}) },
      timeoutMs,
      killGraceMs,
      echo: false,
    });
    let parsed = null;
    let note;
    try {
      if (outcome.timedOut) throw new Error("dependency report timed out");
      parsed = parseDryRun(outcome);
      note = parsed.satisfied ? "satisfied" : `${parsed.packages.length} missing`;
    } catch (error) {
      note = error.message.slice(0, 500);
    }
    // The dry-run's own exit 1 means "missing packages", not a failed command.
    record(stage, attempt, route, { ...outcome, exitCode: parsed ? 0 : outcome.exitCode, note });
    return { parsed, note, timedOut: outcome.timedOut };
  };

  const waitForAptExit = async () => {
    const deadline = deps.now() + Math.min(quiescenceMs, Math.max(0, totalDeadline - deps.now()));
    for (;;) {
      const lingering = deps.listProcesses().filter((entry) => APT_PROCESS.test(entry.args));
      if (lingering.length === 0) return;
      if (deps.now() >= deadline) {
        throw new StageError(
          "acquire",
          `apt processes still running after the attempt; refusing an overlapping retry: ${lingering
            .map((entry) => `${entry.pid} ${entry.args}`)
            .join("; ")}`,
        );
      }
      await deps.sleep(1000);
    }
  };

  const prepareOfficialRoute = () => {
    const parts = deps.listDir("/etc/apt/sources.list.d").map((name) => ({
      name,
      content: deps.readText(path.join("/etc/apt/sources.list.d", name)) ?? "",
    }));
    const route = buildOfficialRoute({
      sourcesList: deps.readText("/etc/apt/sources.list"),
      parts,
      mirrorList: deps.readText(RUNNER_MIRROR_LIST),
    });
    const dir = deps.makeTempDir();
    deps.mkdir(path.join(dir, "sources.list.d"));
    deps.mkdir(path.join(dir, "lists", "partial"));
    deps.writeText(path.join(dir, "sources.list"), route.sourcesList);
    for (const part of route.parts)
      deps.writeText(path.join(dir, "sources.list.d", part.name), part.content);
    const aptConfig = path.join(dir, "apt.conf");
    deps.writeText(
      aptConfig,
      [
        `Dir::Etc::SourceList "${path.join(dir, "sources.list")}";`,
        `Dir::Etc::SourceParts "${path.join(dir, "sources.list.d")}";`,
        `Dir::State::Lists "${path.join(dir, "lists")}";`,
        "",
      ].join("\n"),
    );
    receipt.dependencies.officialRoute = {
      aptConfig,
      rewrittenStanzas: route.rewritten,
      sources: Object.fromEntries(route.parts.map((part) => [part.name, part.content])),
      sourcesList: route.sourcesList,
    };
    return { aptConfig, dir };
  };

  // apt.conf.d is read after APT_CONFIG, so prove the effective directories.
  const verifyRouteConfig = async ({ aptConfig, dir }) => {
    const dumped = await deps.run({
      command: "apt-config",
      args: ["dump"],
      env: { ...env, APT_CONFIG: aptConfig },
      timeoutMs: within("acquire", 30_000),
      killGraceMs,
      echo: false,
    });
    const value = (key) =>
      dumped.stdout?.match(new RegExp(`^${key} "([^"]*)";$`, "mi"))?.[1]?.replace(/\/$/, "");
    const expected = {
      "Dir::Etc::sourcelist": path.join(dir, "sources.list"),
      "Dir::Etc::sourceparts": path.join(dir, "sources.list.d"),
      "Dir::State::lists": path.join(dir, "lists"),
    };
    const wrong = Object.entries(expected).filter(([key, want]) => value(key) !== want);
    record("acquire:route-config", 2, "official-archive", {
      ...dumped,
      exitCode: ok(dumped) && wrong.length === 0 ? 0 : (dumped.exitCode ?? 1) || 1,
      note: wrong.length ? `overridden: ${wrong.map(([key]) => key).join(", ")}` : "effective",
    });
    if (!ok(dumped) || wrong.length > 0) {
      throw new StageError("acquire", "the official-route APT configuration is not in effect");
    }
  };

  // One bounded acquisition attempt. Throws StageError({transport:true}) for
  // failures that a different endpoint can plausibly fix.
  const acquire = async (attempt, route, aptConfig) => {
    const deadline = deps.now() + within("acquire", options.acquireTimeoutMs);
    const remaining = () => deadline - deps.now();
    const update = sudo(["update"], aptConfig, remaining());
    const updated = await deps.run(update);
    record("acquire:metadata", attempt, route, updated);
    if (!ok(updated)) {
      const timedOut = aptTimedOut(updated, update.timeoutMs - killGraceMs - 5_000);
      throw new StageError(
        "acquire",
        timedOut
          ? "apt-get update reached the acquisition deadline"
          : `apt-get update exited ${updated.exitCode}`,
        { transport: timedOut || TRANSPORT_OUTPUT.test(updated.tail ?? "") },
      );
    }
    if (remaining() <= 0)
      throw new StageError("acquire", "acquisition deadline reached", { transport: true });
    const resolved = await dryRun(
      "acquire:resolve",
      attempt,
      route,
      aptConfig,
      Math.max(remaining(), 1000),
    );
    if (!resolved.parsed) {
      if (resolved.timedOut) {
        throw new StageError("acquire", "dependency resolution reached the acquisition deadline", {
          transport: true,
        });
      }
      throw new StageError("acquire:resolve", `dependency resolution failed: ${resolved.note}`);
    }
    if (resolved.parsed.satisfied) return [];
    const packages = resolved.parsed.packages;
    if (remaining() <= 0)
      throw new StageError("acquire", "acquisition deadline reached", { transport: true });
    const download = sudo(
      ["install", "-y", "--download-only", "--no-install-recommends", ...packages],
      aptConfig,
      remaining(),
    );
    const downloadBudget = remaining();
    const downloaded = await deps.run(download);
    record("acquire:download", attempt, route, downloaded);
    if (!ok(downloaded)) {
      const timedOut = aptTimedOut(downloaded, downloadBudget);
      throw new StageError(
        "acquire",
        timedOut
          ? "package download reached the acquisition deadline"
          : `apt-get install --download-only exited ${downloaded.exitCode}`,
        { transport: timedOut || TRANSPORT_OUTPUT.test(downloaded.tail ?? "") },
      );
    }
    return packages;
  };

  const onSignal = (signal) => {
    for (const child of activeChildren) killGroup(child, "SIGTERM");
    receipt.result = "cancelled";
    receipt.error = `received ${signal}`;
    try {
      save();
    } catch {
      // the receipt is best effort once the runner is cancelling
    }
    process.exit(signal === "SIGINT" ? 130 : 143);
  };
  if (options.handleSignals) {
    process.once("SIGTERM", onSignal);
    process.once("SIGINT", onSignal);
  }

  try {
    try {
      playwright = deps.playwright();
    } catch (error) {
      throw new StageError("identity", `cannot load the installed Playwright: ${error.message}`);
    }
    receipt.playwright = {
      version: playwright.coreVersion,
      testVersion: playwright.testVersion,
      playwrightVersion: playwright.playwrightVersion,
      lockVersions: playwright.lockVersions,
      hostPlatform: playwright.hostPlatform,
      officiallySupported: playwright.officiallySupported,
    };
    save();
    const versions = new Set([
      playwright.coreVersion,
      playwright.testVersion,
      playwright.playwrightVersion,
    ]);
    if (
      versions.size !== 1 ||
      playwright.lockVersions.length !== 1 ||
      !versions.has(playwright.lockVersions[0])
    ) {
      throw new StageError(
        "identity",
        `installed Playwright (${[...versions].join(", ")}) does not match the single locked version (${playwright.lockVersions.join(", ") || "none"})`,
      );
    }
    if (
      !playwright.officiallySupported ||
      // x64 only: the official retry route has no ports.ubuntu.com entry.
      !/^ubuntu\d+\.\d+-x64$/.test(playwright.hostPlatform)
    ) {
      throw new StageError(
        "identity",
        `unsupported host platform for this helper: ${playwright.hostPlatform}`,
      );
    }

    const probe = await dryRun("probe", 0, "current", null, within("probe", 60_000));
    if (probe.parsed?.satisfied) {
      receipt.dependencies.reused = true;
    } else {
      let packages;
      let aptConfig = null;
      let route = "runner-default";
      try {
        packages = await acquire(1, route, null);
      } catch (error) {
        if (!(error instanceof StageError) || !error.transport) throw error;
        receipt.firstTransportFailure = { attempt: 1, route, error: error.message };
        save();
        await waitForAptExit();
        route = "official-archive";
        try {
          const officialRoute = prepareOfficialRoute();
          await verifyRouteConfig(officialRoute);
          aptConfig = officialRoute.aptConfig;
        } catch (routeError) {
          throw new StageError(
            "acquire",
            `${error.message}; official route unavailable: ${routeError.message}`,
          );
        }
        try {
          packages = await acquire(2, route, aptConfig);
        } catch (retryError) {
          if (retryError instanceof StageError && retryError.transport) {
            throw new StageError("acquire", `${error.message}; retry: ${retryError.message}`);
          }
          throw retryError;
        }
      }
      receipt.dependencies.route = route;
      receipt.dependencies.packages = packages;
      save();
      if (packages.length > 0) {
        const install = sudo(
          ["install", "-y", "--no-download", "--no-install-recommends", ...packages],
          aptConfig,
          within("install", options.installTimeoutMs),
        );
        const installed = await deps.run(install);
        record("install", 1, route, installed);
        if (!ok(installed))
          throw new StageError(
            "install",
            `apt-get install --no-download exited ${installed.exitCode}`,
          );
      }
      const validated = await dryRun("validate", 1, route, aptConfig, within("validate", 60_000));
      if (!validated.parsed?.satisfied) {
        throw new StageError("validate", `system dependencies still incomplete: ${validated.note}`);
      }
    }

    const browsers = await deps.run({
      command: deps.nodePath,
      args: [playwright.cli, "install", ...options.engines],
      env,
      timeoutMs: within("browsers", options.browserTimeoutMs),
      killGraceMs,
    });
    record("browsers", 1, "playwright-cdn", browsers);
    if (!ok(browsers)) {
      throw new StageError(
        "browsers",
        browsers.timedOut
          ? "browser download reached its deadline"
          : `playwright install exited ${browsers.exitCode}`,
      );
    }

    const executables = [];
    for (const engine of options.engines) {
      const list = playwright.executables(engine);
      if (list.length === 0) throw new StageError("verify", `no registry executable for ${engine}`);
      for (const executable of list) {
        const present = deps.exists(executable.path);
        executables.push({ engine, ...executable, present });
      }
    }
    receipt.executables = executables;
    save();
    const absent = executables.filter((executable) => !executable.present);
    if (absent.length > 0) {
      throw new StageError(
        "verify",
        `missing browser executables: ${absent.map((e) => e.path).join(", ")}`,
      );
    }

    for (const engine of options.engines) {
      const launched = await deps.run({
        command: deps.nodePath,
        args: ["-e", launchScript(playwright.coreDir, engine)],
        env,
        timeoutMs: within("launch", 60_000),
        killGraceMs,
      });
      record(`launch:${engine}`, 1, "local", launched);
      if (!ok(launched)) throw new StageError("launch", `${engine} did not launch`);
    }
    receipt.result = "success";
  } catch (error) {
    receipt.result = "failure";
    receipt.failedStage = error instanceof StageError ? error.stage : "internal";
    receipt.error = error.message;
  } finally {
    if (options.handleSignals) {
      process.removeListener("SIGTERM", onSignal);
      process.removeListener("SIGINT", onSignal);
    }
  }
  save();
  if (env.GITHUB_STEP_SUMMARY) {
    const rows = receipt.phases.map(
      (phase) =>
        `| ${phase.stage} | ${phase.attempt} | ${phase.route} | ${phase.exitCode ?? ""}${phase.timedOut ? " (deadline)" : ""} | ${(phase.durationMs / 1000).toFixed(1)}s |`,
    );
    try {
      deps.appendText(
        env.GITHUB_STEP_SUMMARY,
        [
          `### Playwright provisioning: ${options.label} — ${receipt.result}`,
          "",
          `Engines: ${options.engines.join(", ")}; Playwright ${receipt.playwright?.version ?? "unknown"}; ${receipt.runner.os ?? "unknown OS"}${receipt.failedStage ? `; failed stage: ${receipt.failedStage}` : ""}`,
          receipt.firstTransportFailure
            ? `\nFirst transport failure: ${receipt.firstTransportFailure.error}`
            : "",
          "",
          "| Stage | Attempt | Route | Exit | Duration |",
          "| --- | --- | --- | --- | --- |",
          ...rows,
          "",
        ].join("\n"),
      );
    } catch {
      // the JSON receipt is authoritative
    }
  }
  return { receipt, receiptPath, exitCode: receipt.result === "success" ? 0 : 1 };
}

async function main() {
  let options;
  try {
    options = parseArgs(process.argv.slice(2));
  } catch (error) {
    console.error(`install-playwright-linux: ${error.message}`);
    process.exit(2);
  }
  const { receipt, receiptPath, exitCode } = await provision({ ...options, handleSignals: true });
  console.log(`install-playwright-linux: ${receipt.result} (receipt ${receiptPath})`);
  if (exitCode !== 0)
    console.error(
      `::error::Playwright provisioning failed at ${receipt.failedStage}: ${receipt.error}`,
    );
  process.exit(exitCode);
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  await main();
}
