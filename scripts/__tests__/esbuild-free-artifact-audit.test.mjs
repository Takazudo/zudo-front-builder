// Offline unit tests for scripts/esbuild-free-artifact-audit.mjs (#4636).
// The strace lines are in the shape `strace -f -e trace=execve,connect`
// prints on Linux; the pnpm ls shape is `pnpm ls --json --depth Infinity`.

import { describe, expect, it } from "vite-plus/test";
import {
  auditManifest,
  auditStrace,
  classifyEsbuildClosure,
  esbuildTarEntries,
} from "../esbuild-free-artifact-audit.mjs";

describe("auditManifest", () => {
  it("accepts a package with no esbuild dependency or file", () => {
    const report = auditManifest(
      { name: "@takazudo/zfb", version: "1.0.0", dependencies: { hono: "4.0.0" } },
      ["package/package.json", "package/dist/index.js", "package/src/esbuild-notes.md"],
    );
    expect(report.findings).toEqual([]);
  });

  it("flags every runtime dependency field and bundled esbuild", () => {
    const report = auditManifest(
      {
        name: "@takazudo/zfb",
        version: "1.0.0",
        dependencies: { esbuild: "0.25.0" },
        optionalDependencies: { "@esbuild/linux-x64": "0.25.0" },
        peerDependencies: { esbuild: "*" },
        devDependencies: { esbuild: "0.25.0" },
        bundleDependencies: ["esbuild"],
      },
      [],
    );
    expect(report.findings).toEqual([
      "@takazudo/zfb declares esbuild in dependencies",
      "@takazudo/zfb declares @esbuild/linux-x64 in optionalDependencies",
      "@takazudo/zfb declares esbuild in peerDependencies",
      "@takazudo/zfb bundles esbuild via bundleDependencies",
    ]);
  });

  it("treats bundledDependencies: true as bundling every dependency", () => {
    const report = auditManifest(
      { name: "x", dependencies: { esbuild: "1" }, bundledDependencies: true },
      [],
    );
    expect(report.findings).toContain("x bundles esbuild via bundledDependencies");
  });

  it("flags shipped esbuild binaries and package files", () => {
    expect(
      esbuildTarEntries([
        "package/bin/esbuild",
        "package/esbuild.exe",
        "package/node_modules/esbuild/lib/main.js",
        "package/vendor/@esbuild/linux-x64/bin/esbuild",
        "package/dist/esbuild-shim.js",
        "",
      ]),
    ).toEqual([
      "package/bin/esbuild",
      "package/esbuild.exe",
      "package/node_modules/esbuild/lib/main.js",
      "package/vendor/@esbuild/linux-x64/bin/esbuild",
    ]);
  });
});

describe("classifyEsbuildClosure", () => {
  const esbuild = {
    version: "0.25.0",
    dependencies: { "@esbuild/linux-x64": { version: "0.25.0" } },
  };

  it("reports third-party esbuild separately from zfb packages", () => {
    const report = classifyEsbuildClosure([
      {
        name: "site",
        dependencies: {
          "@takazudo/zfb": {
            version: "1.0.0",
            dependencies: { "@takazudo/zfb-linux-x64-gnu": { version: "1.0.0" } },
          },
          wrangler: { version: "4.85.0", dependencies: { esbuild } },
        },
      },
    ]);
    expect(report.firstParty).toEqual([]);
    expect(report.thirdParty).toEqual([
      "site > wrangler@4.85.0 > esbuild@0.25.0",
      "site > wrangler@4.85.0 > esbuild@0.25.0 > @esbuild/linux-x64@0.25.0",
    ]);
  });

  it("charges esbuild reached through a zfb package to first party", () => {
    const report = classifyEsbuildClosure([
      {
        name: "site",
        dependencies: {
          "@takazudo/zfb": {
            version: "1.0.0",
            optionalDependencies: { helper: { version: "2.0.0", dependencies: { esbuild } } },
          },
        },
      },
    ]);
    expect(report.firstParty).toContain(
      "site > @takazudo/zfb@1.0.0 > helper@2.0.0 > esbuild@0.25.0",
    );
    expect(report.thirdParty).toEqual([]);
  });

  it("charges a direct scaffold dependency on esbuild to first party", () => {
    const report = classifyEsbuildClosure({ name: "site", dependencies: { esbuild } });
    expect(report.firstParty).toContain("site > esbuild@0.25.0");
    const dev = classifyEsbuildClosure({ name: "site", devDependencies: { esbuild } });
    expect(dev.firstParty).toContain("site > esbuild@0.25.0");
  });
});

describe("auditStrace", () => {
  const benign = [
    '4100 execve("/usr/bin/node", ["node", "zfb.mjs", "build"], 0x7ffd /* 30 vars */) = 0',
    '4101 execve("/clean/node_modules/@takazudo/zfb-linux-x64-gnu/zfb", ["zfb", "build"], 0x1 /* 30 vars */) = 0',
    '4102 connect(5, {sa_family=AF_UNIX, sun_path="/var/run/nscd/socket"}, 110) = -1 ENOENT (No such file or directory)',
    '4102 connect(6, {sa_family=AF_INET, sin_port=htons(41234), sin_addr=inet_addr("127.0.0.1")}, 16) = 0',
  ].join("\n");

  it("passes a build that only execs node and zfb and talks to loopback", () => {
    const report = auditStrace(benign);
    expect(report.findings).toBe(0);
    expect(report.execCount).toBe(2);
    expect(report.programs).toEqual(["node", "zfb"]);
    expect(report.loopbackConnects).toHaveLength(1);
  });

  it("flags an esbuild exec attempt even when it fails", () => {
    const report = auditStrace(
      `${benign}\n4103 execve("/usr/local/bin/esbuild", ["esbuild", "--version"], 0x1 /* 3 vars */) = -1 ENOENT (No such file or directory)\n` +
        '4104 execve("/x/node_modules/@esbuild/linux-x64/bin/esbuild", ["esbuild"], 0x1) = 0',
    );
    expect(report.esbuildExecs).toHaveLength(2);
    expect(report.findings).toBe(2);
  });

  it("records package-manager execs without failing on them", () => {
    const report = auditStrace(
      `${benign}\n4107 execve("/usr/local/bin/pnpm", ["pnpm", "exec", "zfb-adapter-cloudflare"], 0x1) = 0`,
    );
    expect(report.packageManagerExecs).toHaveLength(1);
    expect(report.findings).toBe(0);
  });

  it("flags downloader execs, remote connects and DNS lookups", () => {
    const report = auditStrace(
      [
        '4105 execve("/usr/bin/curl", ["curl", "https://registry.npmjs.org"], 0x1) = 0',
        '4106 connect(7, {sa_family=AF_INET, sin_port=htons(443), sin_addr=inet_addr("104.16.0.35")}, 16) = -1 EINPROGRESS',
        '4106 connect(8, {sa_family=AF_INET6, sin6_port=htons(443), sin6_flowinfo=htonl(0), inet_pton(AF_INET6, "2606:4700::6810", &sin6_addr), sin6_scope_id=0}, 28) = 0',
        '4106 connect(9, {sa_family=AF_INET, sin_port=htons(53), sin_addr=inet_addr("127.0.0.53")}, 16) = 0',
        '4106 connect(10, {sa_family=AF_INET6, sin6_port=htons(8080), sin6_flowinfo=htonl(0), inet_pton(AF_INET6, "::1", &sin6_addr), sin6_scope_id=0}, 28) = 0',
      ].join("\n"),
    );
    expect(report.downloaderExecs).toHaveLength(1);
    expect(report.networkConnects).toHaveLength(3);
    expect(report.loopbackConnects).toHaveLength(1);
    expect(report.findings).toBe(4);
  });
});
