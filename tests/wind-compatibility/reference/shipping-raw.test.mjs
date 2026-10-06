import test from "node:test";
import assert from "node:assert/strict";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { sha256 } from "../../../scripts/wind-compatibility/reference.mjs";
import {
  assertReplayedReferenceCss,
  validateShippingRawCase,
} from "../../../scripts/wind-compatibility/reference-shipping.mjs";

async function withOutput(run) {
  const output = await mkdtemp(join(tmpdir(), "zfb-shipping-raw-"));
  const save = async (name, bytes) => {
    await writeFile(join(output, name), bytes);
    return { path: name, sha256: sha256(bytes) };
  };
  try {
    await run(output, save);
  } finally {
    await rm(output, { recursive: true, force: true });
  }
}

test("shipping CSS raw case rejects forged pass, absent bytes and stale served hash", async () => {
  await withOutput(async (output, save) => {
    const css = Buffer.from(".bg-new{display:block}");
    const requirement = {
      mode: "cli-css",
      screenshotRequired: false,
      expectedObservation: { present: [".bg-new"], absent: [".bg-old"] },
    };
    const observation = (sha) =>
      JSON.stringify({
        caseId: "css-initial",
        engine: "chromium",
        observed: requirement.expectedObservation,
        cssResponse: { status: 200, contentType: "text/css; charset=utf-8", sha256: sha },
      });
    const generatedCss = await save("generated.css", css);
    const servedCss = await save("served.css", css);
    const result = {
      id: "css-initial",
      engine: "chromium",
      outcome: "passed",
      raw: {
        generatedCss,
        servedCss,
        observation: await save("observation.json", observation(servedCss.sha256)),
        screenshot: null,
      },
    };
    await validateShippingRawCase(result, requirement, output, {});
    await writeFile(join(output, "served.css"), "");
    await assert.rejects(
      validateShippingRawCase(result, requirement, output, {}),
      /raw servedCss changed/,
    );
    await writeFile(join(output, "served.css"), css);
    const wrong = Buffer.from(".bg-old{display:block}");
    result.raw.generatedCss = await save("generated.css", wrong);
    result.raw.servedCss = await save("served.css", wrong);
    result.raw.observation = await save(
      "observation.json",
      observation(result.raw.servedCss.sha256),
    );
    await assert.rejects(
      validateShippingRawCase(result, requirement, output, {}),
      /selector\/token facts changed/,
    );
    await rm(join(output, "served.css"));
    await assert.rejects(validateShippingRawCase(result, requirement, output, {}), /ENOENT/);
  });
});

test("dev evidence rejects missing or reordered warm transitions", async () => {
  await withOutput(async (output, save) => {
    const contracts = [
      { id: "initial", present: [".bg-old"], absent: [".bg-new"] },
      { id: "source-add", present: [".bg-new"], absent: [] },
    ];
    const requirement = {
      mode: "production-dev-warm",
      screenshotRequired: false,
      expectedObservation: { steps: contracts, cleanFinalMatches: true },
    };
    const steps = [];
    for (const [index, contract] of contracts.entries()) {
      const css = Buffer.from(index ? ".bg-new{display:block}" : ".bg-old{display:block}");
      const input = Buffer.from(index ? "new source" : "old source");
      steps.push({
        ...contract,
        mutation: {
          path: index ? "components/added.tsx" : "components/shipping.tsx",
          sha256: sha256(input),
          oldPathAbsent: null,
        },
        rawInput: await save(`input-${index}`, input),
        rawConfig: await save(`config-${index}.json`, JSON.stringify({ phase: index })),
        inputDigest: (index ? "b" : "a").repeat(64),
        cssResponse: { status: 200, contentType: "text/css; charset=utf-8", sha256: sha256(css) },
        rawCss: await save(`step-${index}.css`, css),
      });
    }
    const servedCss = await save("served.css", ".bg-new{display:block}");
    const result = {
      id: "dev-stylesheet",
      engine: "chromium",
      outcome: "passed",
      steps,
      cleanFinalCss: await save("clean.css", ".bg-new{display:block}"),
      raw: {
        generatedCss: null,
        servedCss,
        screenshot: null,
        observation: await save(
          "observation.json",
          JSON.stringify({
            caseId: "dev-stylesheet",
            engine: "chromium",
            observed: requirement.expectedObservation,
            cssResponse: {
              status: 200,
              contentType: "text/css; charset=utf-8",
              sha256: servedCss.sha256,
            },
          }),
        ),
      },
    };
    await validateShippingRawCase(result, requirement, output, {});
    await assert.rejects(
      validateShippingRawCase({ ...result, steps: steps.slice(1) }, requirement, output, {}),
      /transition raw evidence incomplete/,
    );
    await assert.rejects(
      validateShippingRawCase({ ...result, steps: [...steps].reverse() }, requirement, output, {}),
      /transition identity invalid/,
    );
  });
});

test("reference replay rejects a caller-hashed Wind substitute", () => {
  assert.doesNotThrow(() =>
    assertReplayedReferenceCss(Buffer.from("reference"), Buffer.from("reference")),
  );
  assert.throws(
    () => assertReplayedReferenceCss(Buffer.from("reference"), Buffer.from("wind")),
    /cannot be reproduced/,
  );
});

test("token-change replay rejects stale CSS even with rehashed raw files and forged RGB observation", async () => {
  await withOutput(async (output, save) => {
    const requirement = {
      mode: "cli-css-warm",
      screenshotRequired: false,
      expectedObservation: {
        present: [".bg-brand"],
        absent: [],
        backgroundColor: "rgb(34, 68, 102)",
      },
    };
    const result = {
      id: "token-change",
      engine: "chromium",
      outcome: "passed",
      raw: {
        config: await save(
          "config.json",
          JSON.stringify({ wind: { tokens: { colors: { brand: "#224466" } } } }),
        ),
        screenshot: null,
      },
    };
    const retain = async (color) => {
      const css = `@layer zw-tokens { :root { --zw-color-brand: ${color}; } } .bg-brand { background-color: var(--zw-color-brand); }`;
      result.raw.generatedCss = await save("generated.css", css);
      result.raw.servedCss = await save("served.css", css);
      result.raw.observation = await save(
        "observation.json",
        JSON.stringify({
          caseId: "token-change",
          engine: "chromium",
          observed: requirement.expectedObservation,
          cssResponse: {
            status: 200,
            contentType: "text/css; charset=utf-8",
            sha256: result.raw.servedCss.sha256,
          },
        }),
      );
    };
    await retain("#369");
    await assert.rejects(
      validateShippingRawCase(result, requirement, output, {}),
      /Shipping token color differs from contract/,
    );
    await retain("#246");
    await validateShippingRawCase(result, requirement, output, {});
  });
});

test("token-removal replay rejects a retained config token despite rehashed passing CSS", async () => {
  await withOutput(async (output, save) => {
    const requirement = {
      mode: "cli-css-warm",
      screenshotRequired: false,
      expectedObservation: {
        present: [".bg-brand"],
        absent: [".text-transient", "--zw-color-transient"],
      },
    };
    const css = ".bg-brand { background-color: #369; }";
    const generatedCss = await save("generated.css", css);
    const servedCss = await save("served.css", css);
    const result = {
      id: "token-removal",
      engine: "chromium",
      outcome: "passed",
      raw: {
        generatedCss,
        servedCss,
        screenshot: null,
        observation: await save(
          "observation.json",
          JSON.stringify({
            caseId: "token-removal",
            engine: "chromium",
            observed: requirement.expectedObservation,
            cssResponse: {
              status: 200,
              contentType: "text/css; charset=utf-8",
              sha256: servedCss.sha256,
            },
          }),
        ),
        config: await save(
          "config.json",
          JSON.stringify({
            wind: { tokens: { colors: { brand: "#336699", transient: "#883344" } } },
          }),
        ),
      },
    };
    await assert.rejects(
      validateShippingRawCase(result, requirement, output, {}),
      /removed token still present/,
    );
    result.raw.config = await save(
      "config.json",
      JSON.stringify({ wind: { tokens: { colors: { brand: "#336699" } } } }),
    );
    await validateShippingRawCase(result, requirement, output, {});
  });
});
