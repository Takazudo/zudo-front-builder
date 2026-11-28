// #4856: browser executable digests are streamed instead of readFile plus a
// synchronous whole-image sha256. The digest must equal the old one byte for byte.
import { createHash, randomBytes } from "node:crypto";
import { mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { afterAll, describe, expect, it } from "vitest";
import { streamDigest } from "../lib/browser.mjs";
import { sha256 } from "../lib/util.mjs";

describe("streamDigest", () => {
  const dir = mkdtempSync(join(tmpdir(), "dbo-stream-digest-"));
  afterAll(() => rmSync(dir, { recursive: true, force: true }));

  it.each([
    ["empty", 0, 1 << 20],
    ["sub-chunk", 1000, 1 << 20],
    ["exact chunk multiple", 4 * 65536, 65536],
    ["multi-chunk with tail", 5 * 65536 + 17, 65536],
    ["default 1 MiB chunks", 3 * (1 << 20) + 5, undefined],
  ])("equals the old whole-file digest: %s", async (_name, size, chunkBytes) => {
    const path = join(dir, `f-${size}-${chunkBytes}`);
    writeFileSync(path, randomBytes(size));
    const streamed = await streamDigest(path, chunkBytes ? { chunkBytes } : {});
    const bytes = readFileSync(path);
    expect(streamed.digest).toBe(sha256(bytes));
    expect(streamed.length).toBe(bytes.length);
  });

  it("hashes each chunk as it is read, never the whole image at once", async () => {
    const size = 7 * 65536 + 3;
    const path = join(dir, "per-chunk");
    writeFileSync(path, randomBytes(size));
    const updates = [];
    const createHashImpl = (algorithm) => {
      const hash = createHash(algorithm);
      return {
        update(chunk) {
          updates.push(chunk.length);
          hash.update(chunk);
          return this;
        },
        digest: (encoding) => hash.digest(encoding),
      };
    };
    const streamed = await streamDigest(path, { chunkBytes: 65536, createHashImpl });
    expect(updates.length).toBe(Math.ceil(size / 65536));
    expect(Math.max(...updates)).toBeLessThanOrEqual(65536);
    expect(streamed.chunks).toBe(updates.length);
    expect(streamed.digest).toBe(sha256(readFileSync(path)));
  });

  it("rejects on abort and on a missing file", async () => {
    const path = join(dir, "abort");
    writeFileSync(path, randomBytes(8 * 65536));
    const abort = new AbortController();
    abort.abort(new Error("stopped"));
    await expect(streamDigest(path, { signal: abort.signal, chunkBytes: 65536 })).rejects.toThrow();
    await expect(streamDigest(join(dir, "missing"))).rejects.toMatchObject({ code: "ENOENT" });
  });
});
