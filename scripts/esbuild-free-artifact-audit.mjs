#!/usr/bin/env node
// Pure checks behind scripts/smoke-esbuild-free-artifact.sh (#4636): zfb's
// packed packages must not ship esbuild, an installed project may reach
// esbuild only through third-party packages (reported, never hidden), and a
// traced build must not exec esbuild, a downloader, or open a network socket.
//
//   node scripts/esbuild-free-artifact-audit.mjs manifest <package.json> <tar-listing.txt>
//   node scripts/esbuild-free-artifact-audit.mjs closure <pnpm-ls.json>
//   node scripts/esbuild-free-artifact-audit.mjs strace <strace.log>
//
// Each command prints a JSON report on stdout and exits 1 when it holds a
// finding (closure: only first-party findings), 0 otherwise, 2 on bad usage.

import { readFileSync } from "node:fs";
import { basename, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const DEPENDENCY_FIELDS = [
  "dependencies",
  "optionalDependencies",
  "peerDependencies",
  "devDependencies",
];
const BUNDLED_FIELDS = ["bundleDependencies", "bundledDependencies"];

export function isEsbuildPackageName(name) {
  return name === "esbuild" || name.startsWith("@esbuild/");
}

export function isFirstPartyPackageName(name) {
  return name.startsWith("@takazudo/") || name === "create-zfb";
}

/** Tarball entries that are an esbuild binary or an esbuild package file. */
export function esbuildTarEntries(entries) {
  return entries.filter((entry) => {
    const path = entry.trim().replace(/\\/g, "/");
    if (!path) return false;
    const file = path.split("/").pop();
    return (
      /^esbuild(\.exe)?$/i.test(file) ||
      /(^|\/)node_modules\/esbuild(\/|$)/.test(path) ||
      /(^|\/)@esbuild\//.test(path)
    );
  });
}

/** Findings for one packed package: declared esbuild deps and shipped files. */
export function auditManifest(manifest, entries) {
  const findings = [];
  for (const field of DEPENDENCY_FIELDS) {
    // devDependencies are not installed for consumers; they are still listed
    // so a reviewer sees them, but only the runtime fields are findings.
    for (const name of Object.keys(manifest[field] ?? {})) {
      if (isEsbuildPackageName(name) && field !== "devDependencies") {
        findings.push(`${manifest.name} declares ${name} in ${field}`);
      }
    }
  }
  for (const field of BUNDLED_FIELDS) {
    const bundled = manifest[field];
    const names = Array.isArray(bundled)
      ? bundled
      : bundled === true
        ? Object.keys(manifest.dependencies ?? {})
        : [];
    for (const name of names) {
      if (isEsbuildPackageName(name)) {
        findings.push(`${manifest.name} bundles ${name} via ${field}`);
      }
    }
  }
  for (const entry of esbuildTarEntries(entries)) {
    findings.push(`${manifest.name} tarball ships ${entry.trim()}`);
  }
  return { package: manifest.name, version: manifest.version, findings };
}

// pnpm lists devDependencies only for the project itself, where they are a
// real part of the install, so they are walked too.
function childrenOf(node) {
  return Object.entries({
    ...(node.dependencies ?? {}),
    ...(node.optionalDependencies ?? {}),
    ...(node.devDependencies ?? {}),
  });
}

/**
 * Every esbuild package reachable in `pnpm ls --json --depth Infinity` output,
 * with the chain that reaches it. A chain is first-party when esbuild is a
 * direct dependency of the (zfb-scaffolded) project or any ancestor below the
 * project is a zfb package; otherwise it is third-party.
 */
export function classifyEsbuildClosure(projects) {
  const firstParty = [];
  const thirdParty = [];
  const walk = (node, chain, seen) => {
    for (const [name, child] of childrenOf(node)) {
      const key = `${name}@${child.version ?? "?"}`;
      const next = [...chain, key];
      if (isEsbuildPackageName(name)) {
        const viaFirstParty =
          chain.length === 1 ||
          chain.slice(1).some((link) => isFirstPartyPackageName(nameOf(link)));
        (viaFirstParty ? firstParty : thirdParty).push(next.join(" > "));
      }
      if (!seen.has(key)) {
        walk(child, next, new Set([...seen, key]));
      }
    }
  };
  for (const project of Array.isArray(projects) ? projects : [projects]) {
    walk(project, [project.name ?? "(project)"], new Set());
  }
  return {
    firstParty: [...new Set(firstParty)].sort(),
    thirdParty: [...new Set(thirdParty)].sort(),
  };
}

function nameOf(link) {
  const at = link.lastIndexOf("@");
  return at > 0 ? link.slice(0, at) : link;
}

const DOWNLOADERS = new Set(["curl", "wget"]);
// A package-manager exec is recorded, not failed: zfb legitimately runs
// `pnpm exec` for an adapter, and any download it made would still show up
// as a network connect.
const PACKAGE_MANAGERS = new Set(["npm", "npx", "pnpm", "yarn", "corepack"]);

function isLoopback(address) {
  return /^127\./.test(address) || address === "::1" || /^::ffff:127\./i.test(address);
}

/** Exec and connect findings from `strace -f -e trace=execve,connect` output. */
export function auditStrace(text) {
  const execs = [];
  const esbuildExecs = [];
  const downloaderExecs = [];
  const packageManagerExecs = [];
  const networkConnects = [];
  const loopbackConnects = [];
  for (const line of text.split("\n")) {
    const exec = line.match(/execve\("([^"]*)"/);
    if (exec) {
      const file = exec[1];
      const program = basename(file);
      execs.push(file);
      if (
        /^esbuild(\.exe)?$/i.test(program) ||
        /\/@esbuild\//.test(file) ||
        /\/node_modules\/esbuild\//.test(file)
      ) {
        esbuildExecs.push(line.trim());
      } else if (DOWNLOADERS.has(program)) {
        downloaderExecs.push(line.trim());
      } else if (PACKAGE_MANAGERS.has(program)) {
        packageManagerExecs.push(line.trim());
      }
      continue;
    }
    if (!/connect\(/.test(line)) continue;
    const family = line.match(/sa_family=(AF_INET6?)\b/)?.[1];
    if (!family) continue;
    const address =
      family === "AF_INET"
        ? line.match(/inet_addr\("([^"]+)"\)/)?.[1]
        : line.match(/inet_pton\(AF_INET6,\s*"([^"]+)"/)?.[1];
    const port = Number(line.match(/sin6?_port=htons\((\d+)\)/)?.[1] ?? NaN);
    if (address && isLoopback(address) && port !== 53) {
      loopbackConnects.push(line.trim());
    } else {
      networkConnects.push(line.trim());
    }
  }
  return {
    execCount: execs.length,
    programs: [...new Set(execs.map((file) => basename(file)))].sort(),
    esbuildExecs,
    downloaderExecs,
    packageManagerExecs,
    networkConnects,
    loopbackConnects,
    findings: esbuildExecs.length + downloaderExecs.length + networkConnects.length,
  };
}

function main(argv) {
  const [command, ...args] = argv;
  let report;
  let failed;
  if (command === "manifest" && args.length === 2) {
    const manifest = JSON.parse(readFileSync(args[0], "utf8"));
    const entries = readFileSync(args[1], "utf8").split("\n");
    report = auditManifest(manifest, entries);
    failed = report.findings.length > 0;
  } else if (command === "closure" && args.length === 1) {
    report = classifyEsbuildClosure(JSON.parse(readFileSync(args[0], "utf8")));
    failed = report.firstParty.length > 0;
  } else if (command === "strace" && args.length === 1) {
    report = auditStrace(readFileSync(args[0], "utf8"));
    if (report.execCount === 0) {
      report.error = "the trace recorded no execve at all; it did not observe the build";
    }
    failed = report.findings > 0 || report.execCount === 0;
  } else {
    console.error(
      "usage: esbuild-free-artifact-audit.mjs manifest <package.json> <tar-listing> | closure <pnpm-ls.json> | strace <log>",
    );
    return 2;
  }
  console.log(JSON.stringify(report, null, 2));
  return failed ? 1 : 0;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === resolve(process.argv[1])) {
  process.exitCode = main(process.argv.slice(2));
}
