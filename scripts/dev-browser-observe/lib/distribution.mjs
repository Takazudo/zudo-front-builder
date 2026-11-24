import { spawnSync } from "node:child_process";
import {
  constants as fsConstants,
  existsSync,
  lstatSync,
  mkdirSync,
  readdirSync,
  readFileSync,
  readlinkSync,
  realpathSync,
  writeFileSync,
} from "node:fs";
import { delimiter, dirname, join, relative } from "node:path";
import { createModel } from "./fieldnotes-model.mjs";
import {
  datasetSha256,
  datasetSha256Of,
  fnAdapterPath,
  fnRoute,
  fnScale,
  isBuildOutputPath,
  loadFieldNotesFixture,
} from "./fieldnotes.mjs";
import {
  cpSyncWritable,
  isoNow,
  readJson,
  run,
  sha256,
  sha256File,
  toPosix,
  treeHashDir,
  treeHashFromEntries,
} from "./util.mjs";

export const DISTRIBUTIONS_SCHEMA = "field-notes-distributions.v1";
export const INSTALL_RECEIPT_SCHEMA = "dev-browser-observe.install-receipt.v1";
export const FRAMEWORKS = ["zfb", "astro", "next"];
export const LAUNCH_KINDS = ["direct", "npm-script"];

export function loadDistributions(fixtureDir, manifest = loadFieldNotesFixture(fixtureDir)) {
  if (!manifest.distributions)
    throw new Error("field-notes fixture manifest declares no distributions");
  const d = readJson(join(fixtureDir, manifest.distributions));
  if (d.schema !== DISTRIBUTIONS_SCHEMA)
    throw new Error(`distributions schema ${d.schema} is not ${DISTRIBUTIONS_SCHEMA}`);
  return d;
}

/** One framework's declaration, plus the sha256 of the declarations file it came from. */
export function distributionOf(
  fixtureDir,
  framework,
  manifest = loadFieldNotesFixture(fixtureDir),
) {
  const all = loadDistributions(fixtureDir, manifest);
  const entry = all.frameworks[framework];
  if (!entry)
    throw new Error(
      `unknown framework ${framework}; declared: ${Object.keys(all.frameworks).join(", ")}`,
    );
  return {
    framework,
    ...entry,
    manifestSha256: sha256File(join(fixtureDir, manifest.distributions)),
  };
}

export const platformKey = (platform = process.platform, arch = process.arch) =>
  `${platform}-${arch}`;

/** App source tree: everything except node_modules, framework caches, and the declared excludes. */
export function appTreeHash(appDir, entry) {
  const exclude = new Set(entry.appTree.exclude ?? []);
  const caches = new Set(entry.cacheDirs ?? []);
  return treeHashDir(appDir, {
    filter: (rel, kind, name) => {
      if (kind === "dir" && (name === "node_modules" || caches.has(rel))) return false;
      return !exclude.has(rel);
    },
  });
}

/** node_modules as installed: every regular file's sha256 and every symlink's target (never followed). */
export function nodeModulesTree(dir) {
  const rows = [];
  let files = 0;
  let symlinks = 0;
  const visit = (abs) => {
    for (const name of readdirSync(abs).sort()) {
      const p = join(abs, name);
      const rel = toPosix(relative(dir, p));
      const st = lstatSync(p);
      if (st.isSymbolicLink()) {
        symlinks += 1;
        rows.push({ path: rel, sha256: `symlink:${readlinkSync(p)}` });
      } else if (st.isDirectory()) visit(p);
      else if (st.isFile()) {
        files += 1;
        rows.push({ path: rel, sha256: sha256File(p) });
      }
    }
  };
  if (!existsSync(dir)) return { sha256: null, files: 0, symlinks: 0, missing: true };
  visit(dir);
  return { sha256: treeHashFromEntries(rows).sha256, files, symlinks };
}

/**
 * Compare the installed tree with the archive lockfile: npm's hidden lockfile
 * (node_modules/.package-lock.json) must list exactly the lockfile's packages with
 * the same version and integrity, and each package.json on disk must carry that
 * version. Optional lockfile entries that npm skipped for this platform are allowed
 * and listed. The tarball integrity itself is npm's verification at install time;
 * this check cannot recompute it from extracted files.
 */
export function compareLockfile(appDir) {
  const lock = readJson(join(appDir, "package-lock.json"));
  const hiddenPath = join(appDir, "node_modules", ".package-lock.json");
  const problems = [];
  if (!existsSync(hiddenPath))
    return { ok: false, problems: [{ kind: "hidden-lockfile-missing", path: hiddenPath }] };
  const hidden = readJson(hiddenPath);
  const installed = [];
  const skippedOptional = [];
  for (const [key, e] of Object.entries(lock.packages ?? {})) {
    if (key === "") continue;
    const h = hidden.packages?.[key];
    if (!h) {
      if (e.optional === true) skippedOptional.push(key);
      else problems.push({ kind: "missing", key, version: e.version });
      continue;
    }
    if (h.version !== e.version)
      problems.push({ kind: "version", key, lock: e.version, hidden: h.version });
    if ((e.integrity ?? null) !== (h.integrity ?? null))
      problems.push({
        kind: "integrity",
        key,
        lock: e.integrity ?? null,
        hidden: h.integrity ?? null,
      });
    if (e.link) {
      installed.push(key);
      continue;
    }
    const pj = join(appDir, key, "package.json");
    let onDisk = null;
    try {
      onDisk = readJson(pj).version ?? null;
    } catch {
      onDisk = null;
    }
    if (onDisk !== e.version)
      problems.push({ kind: "on-disk-version", key, lock: e.version, onDisk });
    installed.push(key);
  }
  for (const key of Object.keys(hidden.packages ?? {}))
    if (key !== "" && !(key in (lock.packages ?? {}))) problems.push({ kind: "extraneous", key });
  return {
    ok: problems.length === 0,
    lockfileVersion: lock.lockfileVersion ?? null,
    hiddenLockfileSha256: sha256File(hiddenPath),
    installed: installed.length,
    skippedOptional,
    problems,
  };
}

/** Pinned packages: node_modules/<pkg>/package.json and the lockfile entry both carry the pinned version. */
export function pinnedVersions(appDir, pinned) {
  let lock = null;
  try {
    lock = readJson(join(appDir, "package-lock.json"));
  } catch {
    lock = null;
  }
  return Object.entries(pinned).map(([name, version]) => {
    let installed = null;
    try {
      installed = readJson(join(appDir, "node_modules", name, "package.json")).version ?? null;
    } catch {
      installed = null;
    }
    const locked = lock?.packages?.[`node_modules/${name}`] ?? null;
    return {
      name,
      pinned: version,
      installed,
      locked: locked?.version ?? null,
      integrity: locked?.integrity ?? null,
      ok: installed === version && locked?.version === version,
    };
  });
}

/** The framework's executables for this platform: declared path, realpath and sha256. */
export function resolveExecutables(appDir, entry, key = platformKey()) {
  return entry.executables.map((x) => {
    const rel = typeof x.path === "string" ? x.path : (x.path[key] ?? null);
    if (!rel) return { role: x.role, path: null, ok: false, reason: `no ${x.role} for ${key}` };
    const abs = join(appDir, rel);
    if (!existsSync(abs)) return { role: x.role, path: rel, ok: false, reason: "missing" };
    const real = realpathSync(abs);
    return {
      role: x.role,
      path: rel,
      realpath: real,
      sha256: sha256File(real),
      ok: x.sha256 ? sha256File(real) === x.sha256 : true,
      ...(x.sha256 ? { expectedSha256: x.sha256 } : {}),
    };
  });
}

/** `npm` on PATH resolved to its JavaScript CLI (spawned with the same node), or null. */
export function resolveNpmCli(pathEnv = process.env.PATH ?? "") {
  for (const dir of pathEnv.split(delimiter).filter(Boolean)) {
    const p = join(dir, "npm");
    if (!existsSync(p)) continue;
    const real = realpathSync(p);
    if (/\.(c|m)?js$/.test(real)) return real;
    const sibling = join(dirname(dirname(p)), "lib", "node_modules", "npm", "bin", "npm-cli.js");
    if (existsSync(sibling)) return realpathSync(sibling);
  }
  return null;
}

export function npmIdentity(npmCli) {
  if (!npmCli) return { cli: null, sha256: null, version: null };
  let version = null;
  try {
    version = readJson(join(dirname(dirname(npmCli)), "package.json")).version ?? null;
  } catch {
    version = null;
  }
  return { cli: npmCli, sha256: existsSync(npmCli) ? sha256File(npmCli) : null, version };
}

const nodeShaCache = new Map();

/** The node binary's identity; its sha256 is cached per realpath, size and mtime within one process. */
export function nodeIdentity(nodePath = process.execPath) {
  const real = realpathSync(nodePath);
  const st = lstatSync(real);
  const key = `${real}\0${st.size}\0${st.mtimeMs}`;
  if (!nodeShaCache.has(key)) nodeShaCache.set(key, sha256File(real));
  return {
    execPath: nodePath,
    realpath: real,
    sha256: nodeShaCache.get(key),
    version: process.version,
  };
}

const fill = (args, vars) =>
  args.map((a) => a.replace(/\{(\w+)\}/g, (_m, k) => String(vars[k] ?? `{${k}}`)));

/** The exact command a session spawns for this framework and launch kind. */
export function launchCommand({ entry, appDir, launch = "direct", host, port, nodePath, npmCli }) {
  const vars = { host, port };
  if (launch === "direct")
    return {
      kind: "direct",
      command: nodePath,
      args: [join(appDir, entry.launch.direct.script), ...fill(entry.launch.direct.args, vars)],
    };
  if (launch === "npm-script") {
    const s = entry.launch.npmScript;
    if (!s) throw new Error(`framework ${entry.framework} has no npm dev script`);
    if (!npmCli) throw new Error("npm-script launch needs npm on PATH (an npm-cli.js)");
    return {
      kind: "npm-script",
      command: nodePath,
      args: [npmCli, "run", s.script, "--", ...fill(s.args, vars)],
      script: s.script,
    };
  }
  throw new Error(`unknown launch ${launch}`);
}

/**
 * Text a readiness rule sees for one output line: the line itself, or, for a JSON log
 * line with a string `message` (Astro's non-TTY logger), that message with its newlines.
 */
export function logicalText(line) {
  const t = line.trim();
  if (t.startsWith("{") && t.endsWith("}")) {
    try {
      const j = JSON.parse(t);
      if (typeof j.message === "string") return j.message;
    } catch {
      return line;
    }
  }
  return line;
}

/**
 * Readiness from server output: every `require` pattern seen and the `url` pattern captured.
 * `port` (when given) must equal the URL's port; a different port is a failure, not readiness.
 */
export function createReadiness(spec, { port = null } = {}) {
  const url = new RegExp(spec.url);
  const required = (spec.require ?? []).map((r) => ({ source: r, re: new RegExp(r) }));
  const state = { seen: new Set(), url: null, lines: 0, failure: null };
  return {
    state,
    push(line) {
      state.lines += 1;
      const text = logicalText(line);
      for (const r of required) if (r.re.test(text)) state.seen.add(r.source);
      if (state.url === null) {
        const m = text.match(url);
        if (m) {
          const found = m[1].replace(/\/$/, "");
          let p;
          try {
            p = new URL(found).port;
          } catch {
            state.failure = { reason: "ready-url-unparsable", printed: found };
            state.url = found;
            return this.result();
          }
          if (port !== null && String(port) !== p)
            state.failure = {
              reason: "ready-port-mismatch",
              expected: String(port),
              printed: found,
            };
          state.url = found;
        }
      }
      return this.result();
    },
    result() {
      if (state.failure) return { failed: state.failure };
      if (state.url !== null && required.every((r) => state.seen.has(r.source)))
        return { url: state.url, seen: [...state.seen] };
      return null;
    },
  };
}

/** Role of each owned process, from the framework's declared args patterns (first match). */
export function classifyRoles(entry, processes) {
  const rules = (entry.roles ?? []).map((r) => ({ role: r.role, re: new RegExp(r.args) }));
  return processes.map((p) => ({
    pid: p.pid,
    ppid: p.ppid,
    comm: p.comm,
    role: rules.find((r) => r.re.test(p.args ?? ""))?.role ?? "unclassified",
  }));
}

/**
 * The executable a live process runs. macOS and Linux lsof list it as the first `txt` entry;
 * `ps -o comm=` is the fallback when it prints an absolute path. Null when neither works.
 */
export function processExecutable(pid, runImpl = run) {
  const env = { ...process.env, LC_ALL: "C" };
  try {
    const out = runImpl("lsof", ["-a", "-p", String(pid), "-d", "txt", "-Fn"], { env });
    const line = out.split("\n").find((l) => l.startsWith("n"));
    if (line) return { path: line.slice(1), method: "lsof-txt" };
  } catch {
    // fall through to ps
  }
  try {
    const comm = runImpl("ps", ["-o", "comm=", "-p", String(pid)], { env }).trim();
    if (comm.startsWith("/")) return { path: comm, method: "ps-comm" };
  } catch {
    // unreadable
  }
  return { path: null, method: "unavailable" };
}

/** Shells an npm `run` (or a launcher's `sh -c`) may legitimately use, compared after realpath. */
export const SYSTEM_SHELLS = ["/bin/sh", "/bin/bash", "/bin/zsh", "/bin/dash"];

const realOrSelf = (p) => {
  try {
    return realpathSync(p);
  } catch {
    return p;
  }
};

/**
 * Every owned process at readiness must run an executable of the install: node-role processes
 * the install's node binary (a PATH `node`, e.g. via npm's `.bin` shebang, can differ); the zfb
 * native process the installed platform binary; a `sh -c` a system shell (SYSTEM_SHELLS after
 * realpath); any other process an executable inside the app's node_modules. A process that
 * exited between the snapshot and this check is recorded as gone.
 */
export function processIdentityChecks({
  processes,
  roles,
  nodeRealpath,
  nodeSha256 = null,
  systemShells = SYSTEM_SHELLS,
  executables = [],
  appDir,
  exeOf = processExecutable,
  alive = (pid) => {
    try {
      process.kill(pid, 0);
      return true;
    } catch {
      return false;
    }
  },
}) {
  const nativeSha = executables.find((x) => x.role === "zfb-native")?.sha256 ?? null;
  const shells = new Set(systemShells.filter((p) => existsSync(p)).map(realOrSelf));
  const nm = join(realOrSelf(appDir), "node_modules");
  return processes.map((p) => {
    const role = roles.find((r) => r.pid === p.pid)?.role ?? "unclassified";
    const exe = exeOf(p.pid);
    const real = exe.path ? realOrSelf(exe.path) : null;
    const base = {
      name: `process-executable:${p.pid}:${role}`,
      pid: p.pid,
      role,
      method: exe.method,
    };
    if (real === null)
      return alive(p.pid)
        ? { ...base, ok: false, expected: "a readable executable", actual: null }
        : { ...base, ok: true, expected: null, actual: null, note: "exited before the check" };
    if (role === "shell")
      return { ...base, ok: shells.has(real), expected: [...shells], actual: real };
    if (role === "zfb-native") {
      const sha = existsSync(real) ? sha256File(real) : null;
      return {
        ...base,
        ok: sha !== null && sha === nativeSha,
        expected: nativeSha,
        actual: { path: real, sha256: sha },
      };
    }
    const isNode = role !== "unclassified" || /(^|\/)node$/.test(real);
    if (isNode) {
      if (real === nodeRealpath)
        return { ...base, ok: true, expected: nodeRealpath, actual: real, matchedBy: "realpath" };
      // A byte-identical node binary at another path is the same node.
      const sha = nodeSha256 && existsSync(real) ? sha256File(real) : null;
      return {
        ...base,
        ok: sha !== null && sha === nodeSha256,
        expected: { realpath: nodeRealpath, sha256: nodeSha256 },
        actual: sha === null ? real : { path: real, sha256: sha },
        ...(sha !== null && sha === nodeSha256 ? { matchedBy: "sha256" } : {}),
      };
    }
    return { ...base, ok: real.startsWith(`${nm}/`), expected: `${nm}/…`, actual: real };
  });
}

function check(name, ok, expected, actual, extra = {}) {
  return { name, ok, expected, actual, ...extra };
}

/**
 * Identity of an installed framework dir (no edits, no network): app tree, package.json,
 * lockfile, pinned versions, hidden-lockfile comparison, executables, no framework caches,
 * and the node_modules tree hash.
 */
export function verifyInstall({ installDir, entry, key = platformKey() }) {
  const appDir = join(installDir, entry.app);
  const checks = [];
  if (!existsSync(appDir))
    return {
      ok: false,
      checks: [check("install-app-dir", false, "existing dir", appDir)],
      facts: {},
    };
  const tree = appTreeHash(appDir, entry);
  checks.push(
    check("app-tree", tree.sha256 === entry.appTree.sha256, entry.appTree.sha256, tree.sha256),
  );
  const pj = sha256File(join(appDir, "package.json"));
  checks.push(check("package-json", pj === entry.packageJsonSha256, entry.packageJsonSha256, pj));
  const lockPath = join(appDir, entry.lockfile.path);
  const lockSha = existsSync(lockPath) ? sha256File(lockPath) : null;
  checks.push(check("lockfile", lockSha === entry.lockfile.sha256, entry.lockfile.sha256, lockSha));
  const pinned = pinnedVersions(appDir, entry.pinned);
  for (const p of pinned)
    checks.push(
      check(`pinned:${p.name}`, p.ok, p.pinned, { installed: p.installed, locked: p.locked }),
    );
  let lock;
  try {
    lock = compareLockfile(appDir);
  } catch (error) {
    lock = {
      ok: false,
      problems: [{ kind: "unreadable", error: String(error?.message ?? error) }],
    };
  }
  checks.push(check("lockfile-installed-match", lock.ok, [], lock.problems));
  const execs = resolveExecutables(appDir, entry, key);
  for (const x of execs)
    checks.push(check(`executable:${x.role}`, x.ok, x.expectedSha256 ?? "present", x));
  const caches = (entry.cacheDirs ?? []).filter((d) => existsSync(join(appDir, d)));
  checks.push(check("no-framework-caches", caches.length === 0, [], caches));
  const nm = nodeModulesTree(join(appDir, "node_modules"));
  checks.push(
    check("node-modules-present", !nm.missing, "node_modules", nm.missing ? null : nm.sha256),
  );
  return {
    ok: checks.every((c) => c.ok),
    checks,
    facts: {
      appDir,
      appTreeSha256: tree.sha256,
      packageJsonSha256: pj,
      lockfileSha256: lockSha,
      pinned,
      lockComparison: {
        installed: lock.installed ?? null,
        skippedOptional: lock.skippedOptional ?? null,
        hiddenLockfileSha256: lock.hiddenLockfileSha256 ?? null,
      },
      executables: execs,
      nodeModules: nm,
      platform: key,
    },
  };
}

const NPM_CI_ARGS = ["ci", "--no-audit", "--no-fund"];

/**
 * Install one framework from the archive lockfile: copy <sourceDir>/<app> (no node_modules or
 * caches) to <outDir>/<app>, check its declared hashes, run `npm ci` there with the same node,
 * then verify the result and write <outDir>/install-receipt.json. dryRun returns the plan only.
 */
export function installDistribution({
  fixtureDir,
  framework,
  sourceDir,
  outDir,
  npmCli = resolveNpmCli(),
  nodePath = process.execPath,
  dryRun = false,
  spawnImpl = spawnSync,
}) {
  const manifest = loadFieldNotesFixture(fixtureDir);
  const entry = distributionOf(fixtureDir, framework, manifest);
  const srcApp = join(sourceDir, entry.app);
  const appDir = join(outDir, entry.app);
  const command = [nodePath, npmCli ?? "<npm not found>", ...NPM_CI_ARGS];
  const plan = { framework, sourceApp: srcApp, installDir: outDir, appDir, cwd: appDir, command };
  if (dryRun) return { dryRun: true, plan };
  if (!npmCli) throw new Error("npm not found on PATH; pass --npm-cli <path to npm-cli.js>");
  if (existsSync(outDir)) throw new Error(`install dir already exists: ${outDir}`);
  const srcTree = appTreeHash(srcApp, entry);
  if (srcTree.sha256 !== entry.appTree.sha256)
    throw new Error(
      `source app ${srcApp} tree ${srcTree.sha256} is not the declared ${entry.appTree.sha256}; nothing copied`,
    );
  mkdirSync(dirname(appDir), { recursive: true });
  const caches = new Set(entry.cacheDirs ?? []);
  cpSyncWritable(srcApp, appDir, {
    recursive: true,
    filter: (p) => {
      const rel = toPosix(relative(srcApp, p));
      return (
        !rel.split("/").includes("node_modules") && !caches.has(rel) && !isBuildOutputPath(rel)
      );
    },
  });
  const started = Date.now();
  const r = spawnImpl(command[0], command.slice(1), {
    cwd: appDir,
    encoding: "utf8",
    maxBuffer: 256 * 1024 * 1024,
    env: { ...process.env, npm_config_update_notifier: "false" },
  });
  const tail = (s) =>
    String(s ?? "")
      .split("\n")
      .slice(-60)
      .join("\n");
  const verification = r.status === 0 ? verifyInstall({ installDir: outDir, entry }) : null;
  const receipt = {
    schema: INSTALL_RECEIPT_SCHEMA,
    framework,
    createdAt: isoNow(),
    durationMs: Date.now() - started,
    distributionsSha256: entry.manifestSha256,
    entry: { app: entry.app, pinned: entry.pinned, lockfileSha256: entry.lockfile.sha256 },
    source: { dir: sourceDir, app: srcApp, appTreeSha256: srcTree.sha256 },
    installDir: outDir,
    appDir,
    node: nodeIdentity(nodePath),
    npm: npmIdentity(npmCli),
    command,
    cwd: appDir,
    exitCode: r.status,
    signal: r.signal ?? null,
    spawnError: r.error ? String(r.error.message ?? r.error) : null,
    stdoutTail: tail(r.stdout),
    stderrTail: tail(r.stderr),
    verification,
    ok: r.status === 0 && verification?.ok === true,
  };
  writeFileSync(join(outDir, "install-receipt.json"), `${JSON.stringify(receipt, null, 2)}\n`);
  return receipt;
}

/**
 * Identity checks of a fixed-distribution profile's install against its receipt and the
 * declarations, for this process (same node binary as the install). No edits.
 */
export function distributionChecks({ installDir, entry, nodePath = process.execPath }) {
  const checks = [];
  const receiptPath = join(installDir, "install-receipt.json");
  let receipt = null;
  try {
    receipt = readJson(receiptPath);
  } catch (error) {
    checks.push(
      check("install-receipt-readable", false, receiptPath, String(error?.message ?? error)),
    );
    return { ok: false, checks, receipt: null, verification: null };
  }
  checks.push(check("install-receipt-ok", receipt.ok === true, true, receipt.ok ?? null));
  checks.push(
    check(
      "install-receipt-framework",
      receipt.framework === entry.framework,
      entry.framework,
      receipt.framework,
    ),
  );
  checks.push(
    check(
      "install-receipt-declarations",
      receipt.distributionsSha256 === entry.manifestSha256,
      entry.manifestSha256,
      receipt.distributionsSha256,
    ),
  );
  const node = nodeIdentity(nodePath);
  checks.push(
    check(
      "node-version-matches-install",
      receipt.node?.version === node.version,
      receipt.node?.version ?? null,
      node.version,
    ),
  );
  checks.push(
    check(
      "node-binary-matches-install",
      receipt.node?.sha256 === node.sha256,
      receipt.node?.sha256 ?? null,
      node.sha256,
    ),
  );
  const verification = verifyInstall({ installDir, entry });
  checks.push(...verification.checks.map((c) => ({ ...c, name: `install:${c.name}` })));
  const recorded = receipt.verification?.facts ?? {};
  checks.push(
    check(
      "node-modules-unchanged-since-install",
      verification.facts.nodeModules?.sha256 != null &&
        verification.facts.nodeModules?.sha256 === recorded.nodeModules?.sha256,
      recorded.nodeModules?.sha256 ?? null,
      verification.facts.nodeModules?.sha256 ?? null,
    ),
  );
  return { ok: checks.every((c) => c.ok), checks, receipt, verification, node };
}

/** Field Notes helper a fixed-distribution profile runs, as a path in the source app (for pins). */
export function fixedHelperPath({ fixtureDir, manifest, entry, sourceDir, adapter }) {
  if (entry.helper.adapters) return fnAdapterPath(fixtureDir, manifest, adapter);
  return join(sourceDir, entry.app, entry.helper.path);
}

/**
 * Materialize a fixed-distribution work dir with the external Field Notes layout:
 * content/ and shared/ from the scale source, apps/<fw> (with node_modules, cloned when the
 * volume supports it) from the install dir, the declared helper, and the archive's
 * imported-CSS variant files when importedCss. Returns the fixtureOps model `m`.
 */
export function materializeFixed({
  fixtureDir,
  sourceDir,
  installDir,
  workDir,
  framework,
  adapter,
  importedCss = false,
  scale = null,
  expectedDatasetSha256 = null,
}) {
  const manifest = loadFieldNotesFixture(fixtureDir);
  const entry = distributionOf(fixtureDir, framework, manifest);
  const scaleDecl = fnScale(fixtureDir, manifest, scale);
  const wantDataset = expectedDatasetSha256 ?? scaleDecl.datasetSha256;
  if (existsSync(workDir)) throw new Error(`work dir already exists: ${workDir}`);
  const src = manifest.source;
  const sourceDataset = datasetSha256(join(sourceDir, src.content));
  if (sourceDataset !== wantDataset)
    throw new Error(
      `source dataset ${sourceDataset} is not the pinned dataset ${wantDataset}; nothing materialized`,
    );
  mkdirSync(workDir, { recursive: true });
  for (const rel of [src.content, src.shared])
    cpSyncWritable(join(sourceDir, rel), join(workDir, rel), {
      recursive: true,
      filter: (p) => !isBuildOutputPath(relative(join(sourceDir, rel), p)),
    });
  const appDir = join(workDir, entry.app);
  mkdirSync(dirname(appDir), { recursive: true });
  cpSyncWritable(join(installDir, entry.app), appDir, {
    recursive: true,
    verbatimSymlinks: true,
    mode: fsConstants.COPYFILE_FICLONE,
  });
  const helperPath = join(appDir, entry.helper.path);
  let helperProvenance;
  if (entry.helper.adapters) {
    if (!entry.helper.adapters.includes(adapter))
      throw new Error(`adapter ${adapter} is not a declared ${framework} helper`);
    writeFileSync(helperPath, readFileSync(fnAdapterPath(fixtureDir, manifest, adapter)));
    helperProvenance = manifest.adapters.variants[adapter].provenance ?? null;
  } else {
    if (adapter !== "archive") throw new Error(`${framework} runs only its archive helper`);
    helperProvenance = entry.helper.provenance;
  }
  const helperSha256 = sha256File(helperPath);
  if (entry.helper.sha256 && helperSha256 !== entry.helper.sha256)
    throw new Error(
      `${framework} helper ${helperSha256} is not the declared ${entry.helper.sha256}`,
    );
  const importedFiles = [];
  if (importedCss) {
    for (const f of entry.importedCss.files) {
      const bytes = readFileSync(join(fixtureDir, f.file));
      if (sha256(bytes) !== f.sha256)
        throw new Error(`imported-CSS file ${f.file} is not the declared ${f.sha256}`);
      const target = join(appDir, f.target);
      mkdirSync(dirname(target), { recursive: true });
      writeFileSync(target, bytes);
      importedFiles.push({ target: f.target, sha256: f.sha256, provenance: f.provenance });
    }
  }
  const contentDir = join(workDir, src.content, "articles");
  const contentManifest = readJson(join(workDir, src.content, "manifest.json"));
  const materializedDatasetSha256 = datasetSha256Of(contentManifest, contentDir);
  if (materializedDatasetSha256 !== wantDataset)
    throw new Error(`materialized dataset ${materializedDatasetSha256} is not ${wantDataset}`);
  const configPath = join(appDir, entry.config.path);
  return {
    kind: "field-notes",
    mode: "fixed-distribution",
    framework,
    manifest,
    entry,
    placement: "external",
    workDir,
    appDir,
    contentDir,
    layout: {
      placement: "external",
      app: entry.app,
      contentDir: relative(workDir, contentDir),
      externalContentCopied: true,
    },
    materializedDatasetSha256,
    scale: scaleDecl.scale,
    scaleId: scaleDecl.id,
    layoutPath: join(appDir, entry.layout),
    publicCssPath: join(appDir, entry.publicCss),
    managedCssPath: importedCss ? join(appDir, entry.importedCss.edit) : null,
    managedCssEdit: importedCss
      ? { mode: "insert-root", customProperty: entry.importedCss.customProperty }
      : null,
    importedCss: importedFiles,
    helperPath,
    helperSha256,
    helperProvenance,
    adapterSha256: helperSha256,
    trailingSlash: true,
    model: createModel(contentManifest, readJson(join(appDir, entry.spec))),
    route: (key, slug) => fnRoute(key, slug),
    configFile: entry.config.path,
    configSha256: existsSync(configPath) ? sha256File(configPath) : null,
    configProvenance: "archive-app",
    configBaseSha256: entry.config.sha256,
    configOverlay: null,
    appTreeSha256: appTreeHash(appDir, {
      ...entry,
      appTree: {
        ...entry.appTree,
        exclude: [...(entry.appTree.exclude ?? []), ...importedFiles.map((f) => f.target)],
      },
    }).sha256,
    nodeModules: nodeModulesTree(join(appDir, "node_modules")),
  };
}
