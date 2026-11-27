import assert from "node:assert/strict";
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";
import { test } from "vitest";
test.skipIf(process.platform !== "linux")(
  "a persistently failing outer journal still reaps and preserves the prior durable ownership claim",
  () => {
    const path = fileURLToPath(new URL("./next-outer-journal.py", import.meta.url));
    assert.match(
      execFileSync("python3", ["-I", "-B", path], { encoding: "utf8", timeout: 5000 }),
      /persistent-EIO cleanup/,
    );
  },
);
