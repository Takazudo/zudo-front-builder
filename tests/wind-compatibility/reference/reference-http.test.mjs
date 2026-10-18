import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import {
  referenceRequest,
  referenceBody,
} from "../../../scripts/wind-compatibility/reference-http.mjs";
import { pinSource } from "../../../scripts/wind-compatibility/reference-cli.mjs";

const api = "https://api.github.com/repos/tailwindlabs/tailwindcss/git/ref/tags/v4.3.3";
const secret = "test-secret-do-not-retain";
const ok = (url) => ({ ok: true, status: 200, url });

test("GitHub requests use standard token precedence; anonymous requests remain supported", async () => {
  for (const [env, token] of [
    [{}, undefined],
    [{ GITHUB_TOKEN: secret }, secret],
    [{ GH_TOKEN: "preferred", GITHUB_TOKEN: secret }, "preferred"],
    [{ GH_TOKEN: "", GITHUB_TOKEN: secret }, secret],
  ]) {
    let calls = 0;
    await referenceRequest(
      api,
      async (url, options) => {
        calls++;
        assert.equal(options.headers.authorization, token ? `Bearer ${token}` : undefined);
        assert.equal(options.redirect, "error");
        return ok(url);
      },
      env,
    );
    assert.equal(calls, 1);
  }
});

test("credentials never go to npm or codeload, and invalid endpoints never fetch", async () => {
  for (const url of [
    "https://registry.npmjs.org/tailwindcss",
    "https://codeload.github.com/tailwindlabs/tailwindcss/tar.gz/abc",
  ]) {
    await referenceRequest(
      url,
      async (target, options) => {
        assert.equal(options.headers.authorization, undefined);
        return ok(target);
      },
      { GITHUB_TOKEN: secret },
    );
  }
  for (const url of [
    `invalid URL ${secret}`,
    "https://api.github.com.evil.example/x",
    "http://api.github.com/x",
    "https://user:password@api.github.com/x",
    "https://api.github.com:1234/x",
  ]) {
    await assert.rejects(
      referenceRequest(url, () => assert.fail("must not fetch"), { GITHUB_TOKEN: secret }),
      { message: "Reference acquisition: invalid endpoint" },
    );
  }
});

test("HTTP failures distinguish evidence of rate limiting from generic denial without retrying", async () => {
  for (const [status, headers, classification] of [
    [403, { "x-ratelimit-remaining": "0" }, "rate-limited"],
    [403, { "retry-after": "60" }, "rate-limited"],
    [403, {}, "forbidden"],
    [403, { "retry-after": secret }, "forbidden"],
    [401, {}, "unauthorized"],
    [404, {}, "not-found"],
    [429, {}, "rate-limited"],
    [500, {}, "server-error"],
    [503, {}, "server-error"],
    [302, {}, "http-error"],
  ]) {
    let calls = 0;
    await assert.rejects(
      referenceRequest(
        api,
        async () => {
          calls++;
          return {
            ok: false,
            status,
            headers: { get: (name) => headers[name] },
            text: () => assert.fail("do not read error bodies"),
          };
        },
        { GITHUB_TOKEN: secret },
      ),
      { message: `Reference acquisition: HTTP ${status} (${classification})` },
    );
    assert.equal(calls, 1);
  }
});

test("transport, redirect and body failures never expose request or response details", async () => {
  await assert.rejects(
    referenceRequest(
      api,
      async () => {
        throw Error(secret, { cause: secret });
      },
      { GITHUB_TOKEN: secret },
    ),
    { message: "Reference acquisition: network failure (offline or transport error)" },
  );
  await assert.rejects(
    referenceRequest(
      api,
      async () => ({ ...ok(`https://example.com/${secret}`), redirected: true }),
      {},
    ),
    { message: "Reference acquisition: unexpected redirect" },
  );
  for (const method of ["json", "arrayBuffer"]) {
    await assert.rejects(
      referenceBody(
        {
          [method]: async () => {
            throw Error(secret);
          },
        },
        method,
      ),
      (error) => {
        assert.ok(
          !JSON.stringify({
            message: error.message,
            cause: error.cause,
            stack: error.stack,
          }).includes(secret),
        );
        return true;
      },
    );
  }
});

test("pinSource authenticates exact tag observation without establishing package provenance", async () => {
  const candidate = { version: "4.3.3", source: { packageGitSha: null } };
  for (const env of [{}, { GITHUB_TOKEN: secret }]) {
    const result = await pinSource(
      candidate,
      { version: "4.3.2" },
      async (url, options) => {
        assert.equal(url, api);
        assert.equal(
          options.headers.authorization,
          env.GITHUB_TOKEN ? `Bearer ${secret}` : undefined,
        );
        return {
          ...ok(url),
          json: async () => ({
            ref: "refs/tags/v4.3.3",
            object: { type: "commit", sha: "a".repeat(40) },
          }),
        };
      },
      env,
    );
    assert.equal(result.source.status, "tag-observed-artifact-link-unverified");
    assert.equal(result.source.packageGitSha, null);
  }
  await assert.rejects(
    pinSource(candidate, { version: "4.3.2" }, async () => ({ ok: false, status: 403 }), {}),
    /HTTP 403 \(forbidden\)/,
  );
  await assert.rejects(
    pinSource(
      candidate,
      { version: "4.3.2" },
      async (url) => ({
        ...ok(url),
        json: async () => ({
          ref: "refs/tags/v4.3.3",
          object: { type: "tag", sha: "a".repeat(40) },
        }),
      }),
      {},
    ),
    /Unverifiable candidate Git tag/,
  );
  assert.equal(
    await pinSource(
      candidate,
      { version: "4.3.3" },
      () => assert.fail("bootstrap must not fetch"),
      {},
    ),
    candidate,
  );
});

test("Health exposes its existing token only on the Wind evidence step", async () => {
  const workflow = await readFile(
    new URL("../../../.github/workflows/health.yml", import.meta.url),
    "utf8",
  );
  const step = workflow
    .split("      - name: Required Wind comparison and shipping evidence\n")[1]
    .split("      - name:")[0];
  assert.match(step, /env:\n          GITHUB_TOKEN: \$\{\{ github.token \}\}/);
  assert.match(step, /run: bash scripts\/wind-compatibility\/ci-run.sh/);
});
