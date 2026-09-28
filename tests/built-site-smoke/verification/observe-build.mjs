/** @verification W-A08: positive child-process observation for a dependency-free basic-blog build.
 * Run: node tests/built-site-smoke/verification/observe-build.mjs target/release/zfb
 * Linux traces execve with strace; macOS polls the process tree and can miss short-lived children.
 * A record without esbuild is invalid rather than a passing no-Tailwind result.
 */
import { spawn, execFileSync } from "node:child_process";
import { cpSync, mkdtempSync, readFileSync, readdirSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { basename, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const binary = resolve(process.argv[2] ?? "target/release/zfb");
const root = fileURLToPath(new URL("../../../", import.meta.url));
const scratch = mkdtempSync(join(tmpdir(), "zfb-w-a08-"));
const site = join(scratch, "basic-blog");
cpSync(join(root, "crates/zfb/templates/basic-blog"), site, {
  recursive: true,
  filter: (path) => !["node_modules", "dist", ".zfb-build"].includes(basename(path)),
});
const trace = join(scratch, "execve.log");
const linux = process.platform === "linux";
const command = linux ? "strace" : binary;
const args = linux ? ["-f", "-e", "trace=execve", "-o", trace, binary, "build"] : ["build"];
const child = spawn(command, args, { cwd: site, stdio: ["ignore", "pipe", "pipe"] });
let output = "";
child.stdout.on("data", (chunk) => {
  output += chunk;
});
child.stderr.on("data", (chunk) => {
  output += chunk;
});

const seen = new Set();
let poll;
if (!linux) {
  // A successful positive esbuild sighting makes the polling limit explicit.
  poll = setInterval(() => {
    const rows = execFileSync("ps", ["-axo", "pid=,ppid=,comm="], { encoding: "utf8" })
      .trim()
      .split("\n")
      .map((row) => row.trim().split(/\s+/, 3));
    const parents = new Set([String(child.pid)]);
    for (let pass = 0; pass < rows.length; pass++) {
      for (const [pid, ppid, executable] of rows) {
        if (parents.has(ppid)) {
          parents.add(pid);
          seen.add(executable);
        }
      }
    }
  }, 5);
}
const status = await new Promise((resolveStatus, reject) => {
  child.on("error", reject);
  child.on("close", resolveStatus);
});
if (poll) clearInterval(poll);
if (linux) {
  const records = readFileSync(trace, "utf8");
  for (const match of records.matchAll(/execve\("([^"]+)"[^\n]*= 0/g)) seen.add(match[1]);
}
const executables = [...seen].sort();
function cssFiles(dir) {
  return readdirSync(dir, { withFileTypes: true }).flatMap((entry) => {
    const path = join(dir, entry.name);
    return entry.isDirectory() ? cssFiles(path) : entry.name.endsWith(".css") ? [path] : [];
  });
}
const stylesheets = status === 0 ? cssFiles(join(site, "dist")) : [];
const nonemptyCss = stylesheets.some((path) => statSync(path).size > 0);
const esbuildSeen = executables.some((path) => /esbuild/i.test(basename(path)));
const tailwindSeen = executables.some((path) => /tailwind/i.test(basename(path)));
const executableReasons = executables.map((path) => ({
  path,
  reason: /esbuild/i.test(basename(path))
    ? "embedded JavaScript bundler"
    : basename(path) === basename(binary)
      ? "requested zfb build process"
      : "inspect this child against the build log before accepting the record",
}));
const record = {
  method: linux
    ? "strace execve"
    : "macOS process-tree polling (short-lived children may be missed)",
  scratch,
  status,
  executables,
  executableReasons,
  esbuildSeen,
  tailwindSeen,
  stylesheets,
  nonemptyCss,
  output,
};
console.log(JSON.stringify(record, null, 2));
if (status !== 0 || !esbuildSeen || tailwindSeen || !nonemptyCss) process.exitCode = 1;
