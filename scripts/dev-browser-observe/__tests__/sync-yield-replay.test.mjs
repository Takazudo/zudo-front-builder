// #4856 r2 replay with a real pipe. Awaits a child process, so the suite is
// listed in SUBPROCESS_SUITES (vitest.config.mjs).
import { spawn } from "node:child_process";
import { expect, test } from "vitest";
import { yieldToLoop } from "../lib/util.mjs";

// Replay of the r2 shape with a real pipe: a child writes its frames and exits
// at once while the parent runs several synchronous blocks. Without yields the
// frame waits behind the whole chain; with a yield between blocks it waits at
// most one block. The 750 ms sampler budget itself is not involved here.
function busy(ms) {
  const end = process.hrtime.bigint() + BigInt(ms) * 1_000_000n;
  while (process.hrtime.bigint() < end);
}

async function replay({ blocks, blockMs, yields }) {
  const child = spawn(
    process.execPath,
    [
      "-e",
      "process.stdout.write(JSON.stringify({ wroteNs: String(process.hrtime.bigint()) }) + '\\n')",
    ],
    { stdio: ["ignore", "pipe", "inherit"] },
  );
  const received = new Promise((resolve) => {
    let text = "";
    child.stdout.on("data", (chunk) => {
      const callbackNs = process.hrtime.bigint();
      text += chunk;
      if (text.includes("\n")) resolve({ callbackNs, wroteNs: BigInt(JSON.parse(text).wroteNs) });
    });
  });
  const exited = new Promise((resolve) => child.once("exit", resolve));
  // Wait until the child is running, then start the chain before its write lands.
  await new Promise((resolve) => child.once("spawn", resolve));
  for (let i = 0; i < blocks; i++) {
    busy(blockMs);
    if (yields) await yieldToLoop();
  }
  const { callbackNs, wroteNs } = await received;
  await exited;
  return Number(callbackNs - wroteNs) / 1e6;
}

test("yields bound the frame residual to one synchronous block", { timeout: 20_000 }, async () => {
  const blocks = 4,
    blockMs = 250;
  const without = await replay({ blocks, blockMs, yields: false });
  const withYields = await replay({ blocks, blockMs, yields: true });
  // Unyielded, the frame waits for most of the 1000 ms chain (it is written
  // during the first block); yielded, for at most the block it landed in.
  expect(without).toBeGreaterThan((blocks - 1) * blockMs);
  // Bound: the block the frame landed in plus scheduling slack, never two blocks.
  expect(withYields).toBeLessThan(1.5 * blockMs);
});
