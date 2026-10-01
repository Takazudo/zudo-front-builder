import { randomUUID } from "node:crypto";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { basename, dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const REPO_ROOT = resolve(dirname(fileURLToPath(import.meta.url)), "..", "..");
const SOURCE_FIXTURE = join(REPO_ROOT, "crates/zfb/tests/fixtures/embedded-host-request-time");
const STARTUP_TIMEOUT_MS = 25 * 60_000;

function sharedTargetDir(repoRoot) {
  for (let directory = repoRoot; ; directory = dirname(directory)) {
    if (basename(directory) === "worktrees") {
      const sharedRoot = dirname(directory);
      if (existsSync(join(sharedRoot, "Cargo.toml")) && existsSync(join(sharedRoot, "crates"))) {
        return join(sharedRoot, "target");
      }
    }
    const parent = dirname(directory);
    if (parent === directory) return join(repoRoot, "target");
  }
}

const delay = (ms) => new Promise((resolveDelay) => setTimeout(resolveDelay, ms));

async function waitForChildExit(child, timeoutMs) {
  if (child.closed) return true;
  return new Promise((resolveExit) => {
    const timer = setTimeout(() => {
      child.off("close", onClose);
      resolveExit(false);
    }, timeoutMs);
    function onClose() {
      clearTimeout(timer);
      resolveExit(true);
    }
    child.once("close", onClose);
  });
}

function parseReadyPort(log) {
  const readyLines = log
    .split(/\r?\n/)
    .map((line) => line.replace(/\u001b\[[0-9;]*m/g, ""))
    .filter((line) => /\bready\b|\bLocal:/i.test(line));
  for (const line of readyLines) {
    for (const match of line.matchAll(/https?:\/\/[^\s]+/g)) {
      try {
        const port = new URL(match[0]).port;
        const parsedPort = Number.parseInt(port, 10);
        if (parsedPort > 0) return parsedPort;
      } catch {
        // Continue until the zfb ready banner's URL parses.
      }
    }
  }
  return undefined;
}

export async function startWindRawDevServer() {
  const temporaryRoot = await mkdtemp(join(tmpdir(), "zfb-wind-raw-route-"));
  const projectRoot = join(temporaryRoot, "project");
  const marker = `WIND_RAW_ROUTE_OK_${randomUUID()}`;
  try {
    await cp(SOURCE_FIXTURE, projectRoot, {
      recursive: true,
      filter: (source) => {
        const name = basename(source);
        return ![".zfb", "dist", "node_modules"].includes(name) && !name.startsWith(".zfb-dev-");
      },
    });
    const routePath = join(projectRoot, "pages/wind-raw.tsx");
    const routeSource = await readFile(routePath, "utf8");
    const routeWithMarker = routeSource.replace("WIND_RAW_ROUTE_OK", marker);
    if (routeWithMarker === routeSource) {
      throw new Error(
        `the SSR fixture page does not contain the expected route marker: ${routePath}`,
      );
    }
    await writeFile(routePath, routeWithMarker);
  } catch (error) {
    await rm(temporaryRoot, { recursive: true, force: true });
    throw error;
  }

  const child = spawn(
    "cargo",
    [
      "run",
      "--manifest-path",
      join(REPO_ROOT, "Cargo.toml"),
      "-p",
      "zfb",
      "--",
      "dev",
      "--port",
      "0",
    ],
    {
      cwd: projectRoot,
      detached: process.platform !== "win32",
      env: {
        ...process.env,
        CARGO_TARGET_DIR: process.env.CARGO_TARGET_DIR
          ? resolve(REPO_ROOT, process.env.CARGO_TARGET_DIR)
          : sharedTargetDir(REPO_ROOT),
        CARGO_BUILD_JOBS: "1",
        CARGO_INCREMENTAL: "0",
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );

  let childResult;
  let childError;
  let stdoutLog = "";
  let stderrLog = "";
  child.stdout?.on("data", (chunk) => {
    const text = chunk.toString();
    stdoutLog = `${stdoutLog}${text}`.slice(-20000);
    process.stdout.write(text);
  });
  child.stderr?.on("data", (chunk) => {
    const text = chunk.toString();
    stderrLog = `${stderrLog}${text}`.slice(-20000);
    process.stderr.write(text);
  });
  const childExit = new Promise((resolveExit) => {
    child.once("error", (error) => {
      childError = error;
      resolveExit();
    });
    child.once("close", (code, signal) => {
      childResult = { code, signal };
      resolveExit();
    });
  });
  const startupLogs = () => `stdout:\n${stdoutLog}\nstderr:\n${stderrLog}`;

  let stopPromise;
  function stop() {
    if (stopPromise) return stopPromise;
    stopPromise = (async () => {
      if (!childResult && !childError) {
        try {
          if (process.platform === "win32") child.kill("SIGTERM");
          else process.kill(-child.pid, "SIGTERM");
        } catch (error) {
          if (error.code !== "ESRCH") throw error;
        }
        if (!(await waitForChildExit(child, 5000))) {
          try {
            if (process.platform === "win32") child.kill("SIGKILL");
            else process.kill(-child.pid, "SIGKILL");
          } catch (error) {
            if (error.code !== "ESRCH") throw error;
          }
        }
      }
      await childExit;
      await rm(temporaryRoot, { recursive: true, force: true });
    })();
    return stopPromise;
  }

  try {
    const deadline = Date.now() + STARTUP_TIMEOUT_MS;
    let lastObservation = "the dev server has not responded";
    let lastUrl = "the zfb ready banner has not announced an ephemeral URL";
    while (Date.now() < deadline) {
      if (childError) {
        throw new Error(`could not start cargo: ${childError}\n${startupLogs()}`);
      }
      if (childResult) {
        throw new Error(
          `zfb dev exited before serving /wind-raw (code ${childResult.code}, signal ${childResult.signal})\n${startupLogs()}`,
        );
      }
      const port = parseReadyPort(`${stdoutLog}\n${stderrLog}`);
      if (port === undefined) {
        await delay(250);
        continue;
      }
      const url = `http://localhost:${port}/wind-raw`;
      lastUrl = url;
      try {
        const response = await fetch(url, { signal: AbortSignal.timeout(1000) });
        const body = await response.text();
        if (response.status === 200 && body.includes(marker)) {
          return { url, marker, stop };
        }
        lastObservation = `HTTP ${response.status}: ${body.slice(0, 2000)}`;
      } catch (error) {
        lastObservation = String(error);
      }
      await delay(250);
    }
    throw new Error(
      `timed out waiting for ${lastUrl}; last observation: ${lastObservation}\n${startupLogs()}`,
    );
  } catch (error) {
    await stop();
    throw error;
  }
}
