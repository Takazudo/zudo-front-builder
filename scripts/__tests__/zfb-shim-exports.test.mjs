import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vite-plus/test";

const root = join(dirname(fileURLToPath(import.meta.url)), "..", "..");

describe("basic-blog bare zfb type shim", () => {
  it("re-exports exactly the public TypeScript entry points", () => {
    const manifest = JSON.parse(readFileSync(join(root, "packages/zfb/package.json"), "utf8"));
    const expected = Object.entries(manifest.exports)
      .filter(([, target]) => typeof target === "object" && target.types)
      .map(([subpath]) => (subpath === "." ? "zfb" : `zfb/${subpath.slice(2)}`))
      .sort();

    const shim = readFileSync(
      join(root, "crates/zfb/templates/basic-blog/components/zfb-shim.d.ts"),
      "utf8",
    );
    const declarations = [...shim.matchAll(/declare module "([^"]+)"\s*\{([^}]*)\}/g)];
    const actual = declarations.map((match) => match[1]).sort();

    expect(actual).toEqual(expected);
    for (const [, alias, body] of declarations) {
      const packageName = alias.replace(/^zfb(?=\/|$)/, "@takazudo/zfb");
      expect(body).toContain(`export * from "${packageName}";`);
    }
  });
});
