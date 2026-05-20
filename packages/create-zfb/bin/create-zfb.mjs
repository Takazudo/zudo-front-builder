#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";

const require = createRequire(import.meta.url);

const zfbPkgJson = require.resolve("@takazudo/zfb/package.json");
const zfbBin = join(dirname(zfbPkgJson), "bin", "zfb.mjs");

const args = ["new", ...process.argv.slice(2)];
const result = spawnSync(process.execPath, [zfbBin, ...args], { stdio: "inherit" });
process.exit(result.status ?? 1);
