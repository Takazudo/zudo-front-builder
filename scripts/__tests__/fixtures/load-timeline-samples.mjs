// Loads scripts/__tests__/fixtures/supervisor-timeline-samples.txt into a
// { NAME: "value" } object for the vitest suites (#2930). See that file's
// header for the corpus's format and its ownership rules.

import { readFileSync } from "node:fs";

const SAMPLES_URL = new URL("./supervisor-timeline-samples.txt", import.meta.url);

function parseSamples(text) {
  const samples = {};
  for (const rawLine of text.split("\n")) {
    const line = rawLine.trim();
    if (line === "" || line.startsWith("#")) continue;
    const separatorIndex = line.indexOf("=");
    if (separatorIndex === -1) {
      throw new Error(`malformed line in ${SAMPLES_URL}: ${JSON.stringify(rawLine)}`);
    }
    const name = line.slice(0, separatorIndex);
    samples[name] = line.slice(separatorIndex + 1);
  }
  return samples;
}

/** { NAME: "[supervisor-timeline] ..." } for every named sample in the corpus. */
export const TIMELINE_SAMPLES = parseSamples(readFileSync(SAMPLES_URL, "utf8"));
