import assert from "node:assert/strict";
import { compileCss } from "@takazudo/zfb/css";
const result = await compileCss({ cwd: process.argv[2], input: "entry.css" });
assert(result.css.includes(".flex"));
assert(result.css.includes("margin: 0"));
console.log("PASS: return-only compilation in a read-only project");
