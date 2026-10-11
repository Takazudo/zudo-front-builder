import assert from "node:assert/strict";
import fs from "node:fs";
const trace = fs.readFileSync(process.argv[2], "utf8");
const executions = trace.split("\n").filter((line) => /\bexecve(?:at)?\(/.test(line));
assert.equal(executions.length, 1, `Unexpected native subprocess: ${executions.join("\n")}`);
assert(!/\b(?:fork|vfork)\(/.test(trace), "Native fork/vfork");
for (const line of trace.split("\n")) {
  if (/\bclone(?:3)?\(/.test(line))
    assert(line.includes("CLONE_THREAD"), `Non-thread clone: ${line}`);
  assert(
    !/\b(?:open|openat|openat2)\(.*\bO_(?:WRONLY|RDWR|CREAT|TRUNC)\b/.test(line),
    `Filesystem write attempt: ${line}`,
  );
  assert(
    !/\b(?:mkdir|mkdirat|rename|renameat|renameat2|unlink|unlinkat|link|linkat|symlink|symlinkat)\(/.test(
      line,
    ),
    `Filesystem mutation: ${line}`,
  );
}
console.log(
  "PASS: native syscall trace has one initial exec, thread-only clones, no filesystem writes",
);
