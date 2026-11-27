import assert from "node:assert/strict";
import { test } from "vitest";
import { isFixedAstroProtocol } from "../lib/fixed-peer-roles.mjs";
import { startFixedRssSafety } from "../lib/fixed-rss-safety.mjs";

test.each([
  ["linux-fixed-zfb.v1", false, false],
  ["linux-fixed-zfb.v2", false, false],
  ["linux-fixed-zfb.v3", false, true],
  ["linux-fixed-zfb.v4", false, true],
  ["linux-fixed-peer.v1", false, false, "astro"],
  ["linux-fixed-peer.v1", false, false, "next"],
  ["linux-fixed-peer.v2", false, false, "next"],
  ["linux-fixed-peer.v3", false, false, "next"],
  ["linux-fixed-astro-footer.v1", true, false],
  ["linux-fixed-astro-title.v1", true, false],
  ["linux-fixed-astro-category.v1", true, false],
])(
  "initial safety sample preserves the exact call contract for %s",
  async (protocol, astro, needsCustody, bodyFramework) => {
    assert.equal(isFixedAstroProtocol(protocol), astro);
    const server = { handle: { native: { pid: 91 } } };
    const fixedExecutables = { protocol, root: { path: "/pinned/node" } };
    const custody = needsCustody ? { channel: "fixture" } : null;
    const nextCustody = ["linux-fixed-peer.v2", "linux-fixed-peer.v3"].includes(protocol)
      ? { channel: "next-fixture" }
      : null;
    let actual;
    const monitor = startFixedRssSafety({
      server,
      fixedExecutables,
      custody,
      peer: astro || bodyFramework ? { framework: bodyFramework ?? "astro" } : undefined,
      nextOwner: () => nextCustody,
      limits: { sampleEveryMs: 1000, rssCeilingKiB: 4194304 },
      append() {},
      abort() {},
      sample: async (...args) => {
        actual = args;
        throw Error("end controlled initial dispatch");
      },
    });
    await monitor.stop();
    assert.deepEqual(
      actual,
      nextCustody
        ? [server, fixedExecutables.root.path, { nextCustody }]
        : astro || bodyFramework
          ? [server, fixedExecutables.root.path]
          : [server, fixedExecutables.root.path, { fixedExecutables, custody }],
    );
  },
);
