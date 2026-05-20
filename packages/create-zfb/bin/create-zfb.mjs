#!/usr/bin/env node
import { spawnSync } from "node:child_process";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);

const zfbBin = require.resolve("@takazudo/zfb/bin/zfb.mjs");

const args = ["new", ...process.argv.slice(2)];
const result = spawnSync(process.execPath, [zfbBin, ...args], { stdio: "inherit" });
process.exit(result.status ?? 1);
