#!/usr/bin/env node
import { runProductionBuild } from "./production-build.mjs";

if (process.argv.length !== 3)
  throw Error("Usage: build-production.mjs <outside-checkout-output-directory>");
process.stdout.write(`${await runProductionBuild(process.argv[2])}\n`);
