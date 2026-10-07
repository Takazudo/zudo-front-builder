import assert from "node:assert/strict";
import { test } from "node:test";
import { Script } from "node:vm";
import {
  PRESETS,
  FONTS,
  KEYS,
  LIMITS,
  createState,
  createSnapshot,
  restoreSnapshot,
  isValidValue,
  setValue,
  variablesCSS,
  variableMap,
  currentRules,
  previewDocument,
  getSeedFiles,
  windObject,
  contrast,
  onAccent,
  crc32,
  zipData,
  zipBytes,
} from "../../src/components/playground/design-workshop/model.js";

// Independent accepted-spec expectations, rather than snapshots of implementation.
const accepted = {
  everyday: [
    "#3159d8",
    "#f4f7fb",
    "#ffffff",
    "#17253c",
    "#586882",
    "#d5deeb",
    "sans",
    16,
    1.65,
    16,
    40,
    24,
    16,
    8,
    60,
    42,
  ],
  editorial: [
    "#913b28",
    "#faf8f2",
    "#fffefa",
    "#352b26",
    "#736457",
    "#d9cdbd",
    "serif",
    17,
    1.8,
    20,
    56,
    30,
    20,
    0,
    56,
    47,
  ],
  workbench: [
    "#cae887",
    "#171e1b",
    "#202a25",
    "#e0e9df",
    "#acbaac",
    "#405047",
    "mono",
    16,
    1.55,
    12,
    32,
    18,
    12,
    3,
    64,
    35,
  ],
};
const valueKeys = [
  "accent",
  "background",
  "surface",
  "ink",
  "muted",
  "border",
  "headingFont",
  "bodySize",
  "lineHeight",
  "groupGap",
  "sectionGap",
  "horizontalSpace",
  "horizontalGap",
  "radius",
  "readingWidth",
  "headingSize",
];

for (const [id, expected] of Object.entries(accepted)) {
  test(`${id}: accepted decisions drive all previews and five exports`, () => {
    const state = createState(id);
    assert.deepEqual(
      valueKeys.map((key) => state.values[key]),
      expected,
    );
    assert.notEqual(state.values, PRESETS[id].values);
    const files = getSeedFiles(state);
    assert.deepEqual(Object.keys(files).sort(), [
      "DESIGN-NOTES.md",
      "demo.html",
      "design-system.css",
      "usage.tsx",
      "zfb.design.ts",
    ]);
    const css = variablesCSS(state.values);
    assert.ok(files["design-system.css"].endsWith(css));
    for (const mode of ["page", "components", "foundations"]) {
      const preview = previewDocument(state, mode);
      assert.ok(preview.includes(`<style id="ds-variables">${css}</style>`));
      assert.ok(preview.includes(PRESETS[id].name));
    }
    const config = JSON.parse(
      files["zfb.design.ts"].split("export const wind = ")[1].replace(/ as const;\n$/, ""),
    );
    assert.deepEqual(config, windObject(state.values));
    assert.equal(config.spec, 1);
    assert.equal(config.reset, "owned-v1");
    assert.deepEqual(config.tokens.fontSizes.body, {
      size: "var(--ds-font-body)",
      lineHeight: "var(--ds-leading-body)",
    });
    assert.deepEqual(config.breakpoints.wide, { minWidthPx: 960 });
    for (const rule of currentRules(state)) assert.ok(files["DESIGN-NOTES.md"].includes(rule));
    assert.equal(files["demo.html"], previewDocument(state, "page", true, true));
    assert.doesNotMatch(
      files["demo.html"],
      /data-inspect|inspect-on|postMessage|zfb-workshop-inspect/,
    );
    const script = files["demo.html"].match(/<script>([\s\S]*?)<\/script>/)[1];
    assert.doesNotThrow(() => new Script(script));
    assert.match(script, /event\.preventDefault\(\)/);
    assert.doesNotMatch(script, /fetch\(|XMLHttpRequest|localStorage|sessionStorage/);
  });
}

test("edited decisions reach prose, all specimen variables, and the actual export generator", () => {
  const state = createState("editorial");
  const edits = {
    accent: "#ABCDEF",
    headingFont: "mono",
    bodySize: 20,
    lineHeight: 1.4,
    groupGap: 8,
    sectionGap: 64,
    horizontalSpace: 36,
    radius: 18,
    readingWidth: 72,
  };
  for (const [key, value] of Object.entries(edits)) assert.equal(setValue(state, key, value), true);
  const before = createSnapshot(state);
  const variables = variableMap(state.values);
  assert.equal(variables["--ds-brand"], "#abcdef");
  assert.equal(variables["--ds-font-display"], FONTS.mono);
  assert.equal(variables["--ds-hsp-page"], "44px");
  assert.equal(variables["--ds-hsp-gutter"], "20px");
  assert.equal(variables["--ds-font-heading"], "47px");
  const files = getSeedFiles(state);
  for (const [name, value] of Object.entries(variables))
    assert.ok(files["design-system.css"].includes(`${name}: ${value};`));
  for (const phrase of [
    "18px corners",
    "8px within vertical groups",
    "64px between sections",
    "36px",
    "20px body text",
    "1.4 line height",
    "72ch",
    "Headings use mono",
  ])
    assert.ok(files["DESIGN-NOTES.md"].includes(phrase), phrase);
  for (const phrase of ["#abcdef", "8px", "64px", "36px", "20px", "1.4", "72ch", "18px corners"])
    assert.ok(files["demo.html"].includes(phrase), phrase);
  assert.deepEqual(createSnapshot(state), before, "export is pure");
});

test("validation rejects malformed colors, prototype keys and non-finite controls without mutation", () => {
  const state = createState();
  const original = createSnapshot(state);
  for (const [key, value] of [
    ["accent", "#abc"],
    ["accent", "#123456;}</style><script>"],
    ["accent", null],
    ["headingFont", "toString"],
    ["__proto__", {}],
    ["background", "#000000"],
    ["bodySize", "18"],
    ["lineHeight", NaN],
    ["radius", Infinity],
    ["readingWidth", 73],
  ]) {
    assert.equal(isValidValue(key, value), false);
    assert.equal(setValue(state, key, value), false);
  }
  assert.deepEqual(createSnapshot(state), original);
  for (const [key, [min, max]] of Object.entries(LIMITS)) {
    assert.equal(isValidValue(key, min), true);
    assert.equal(isValidValue(key, max), true);
    assert.equal(isValidValue(key, min - 1), false);
    assert.equal(isValidValue(key, max + 1), false);
  }
  assert.throws(() => variablesCSS({ ...state.values, background: "</style>" }), TypeError);
  assert.throws(() => variablesCSS({ ...state.values, headingFont: "constructor" }), TypeError);
  assert.throws(() => contrast("red", "#ffffff"), TypeError);
});

test("snapshots detach, restore known views and clamp only editable finite numbers", () => {
  const state = createState("workbench");
  Object.assign(state, {
    preview: "foundations",
    width: "mobile",
    role: "space",
    route: "starter",
    notes: false,
    inspect: true,
  });
  setValue(state, "accent", "#FFFFFF");
  const snapshot = createSnapshot(state);
  assert.equal(snapshot.version, 1);
  assert.deepEqual(restoreSnapshot(snapshot), state);
  assert.notEqual(snapshot.values, state.values);
  snapshot.values.bodySize = 14;
  assert.equal(state.values.bodySize, 16);
  const restored = restoreSnapshot({
    preset: "editorial",
    values: {
      accent: "#aBcDeF",
      bodySize: 999,
      lineHeight: -1,
      groupGap: NaN,
      radius: Infinity,
      background: "</style>",
      horizontalGap: 99,
      headingSize: 99,
    },
    route: "javascript:alert(1)",
    width: "giant",
    notes: "false",
    inspect: "true",
  });
  assert.equal(restored.values.accent, "#abcdef");
  assert.equal(restored.values.bodySize, 20);
  assert.equal(restored.values.lineHeight, 1.4);
  assert.equal(restored.values.groupGap, 20);
  assert.equal(restored.values.radius, 0);
  assert.equal(restored.values.background, "#faf8f2");
  assert.equal(restored.values.horizontalGap, 20);
  assert.equal(restored.values.headingSize, 47);
  assert.equal(restored.route, "start");
  assert.equal(restored.notes, true);
  assert.equal(restored.inspect, false);
  for (const invalid of [
    null,
    [],
    "everyday",
    { version: 2, preset: "workbench" },
    { version: "1", preset: "editorial" },
  ])
    assert.deepEqual(restoreSnapshot(invalid), createState());
  for (const preset of ["__proto__", "toString", "constructor"])
    assert.deepEqual(createState(preset), createState());
  assert.deepEqual(
    restoreSnapshot(Object.create({ preset: "workbench", values: { accent: "#ffffff" } })),
    createState(),
  );
});

test("contrast selects the more readable candidate label for current accent", () => {
  assert.equal(contrast("#000000", "#ffffff"), 21);
  for (const accent of ["#000000", "#ffffff", "#3159d8", "#913b28", "#cae887"]) {
    const foreground = onAccent({ accent });
    const other = foreground === "#ffffff" ? "#14201a" : "#ffffff";
    assert.ok(contrast(accent, foreground) >= contrast(accent, other));
  }
});

test("starter and standalone previews never inherit inspection mode", () => {
  const state = createState();
  state.inspect = true;
  assert.match(previewDocument(state, "page", false, false, true), /<body class="inspect-on">/);
  assert.doesNotMatch(previewDocument(state, "page", true), /<body class="inspect-on">/);
  assert.doesNotMatch(
    previewDocument(state, "page", true, true, true),
    /data-inspect|inspect-on|postMessage/,
  );
  assert.doesNotMatch(previewDocument(state, "page", false), /<aside class="design-note">/);
  assert.match(previewDocument(state, "page", true), /<aside class="design-note">/);
});

// Inspect ZIP headers independently, including central directory offsets and CRC.
function verifyZip(bytes, files) {
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  const decoder = new TextDecoder();
  const end = bytes.length - 22;
  assert.equal(view.getUint32(end, true), 0x06054b50);
  const count = view.getUint16(end + 10, true);
  assert.equal(count, Object.keys(files).length);
  let central = view.getUint32(end + 16, true);
  const centralStart = central;
  const seen = [];
  for (let index = 0; index < count; index++) {
    assert.equal(view.getUint32(central, true), 0x02014b50);
    const nameLength = view.getUint16(central + 28, true);
    const name = decoder.decode(bytes.subarray(central + 46, central + 46 + nameLength));
    const local = view.getUint32(central + 42, true);
    assert.equal(view.getUint32(local, true), 0x04034b50);
    assert.equal(view.getUint16(local + 6, true), 0x0800);
    assert.equal(view.getUint16(local + 8, true), 0);
    assert.equal(view.getUint16(local + 12, true), 33);
    assert.equal(view.getUint16(local + 26, true), nameLength);
    assert.equal(decoder.decode(bytes.subarray(local + 30, local + 30 + nameLength)), name);
    const size = view.getUint32(local + 18, true);
    const data = bytes.subarray(local + 30 + nameLength, local + 30 + nameLength + size);
    assert.equal(decoder.decode(data), files[name], `${name}: UTF-8 byte equality`);
    assert.equal(view.getUint32(local + 22, true), size);
    assert.equal(view.getUint32(central + 20, true), size);
    assert.equal(view.getUint32(central + 24, true), size);
    assert.equal(view.getUint32(local + 14, true), crc32(data));
    assert.equal(view.getUint32(central + 16, true), crc32(data));
    seen.push(name);
    central += 46 + nameLength;
  }
  assert.deepEqual(seen, Object.keys(files));
  assert.equal(central, end);
  assert.equal(view.getUint32(end + 12, true), end - centralStart);
}

test("all three seed ZIPs and edited ZIP preserve exact generated bytes and CRCs", async () => {
  assert.equal(crc32(new TextEncoder().encode("123456789")), 0xcbf43926);
  assert.equal(crc32(new Uint8Array()), 0);
  for (const id of Object.keys(PRESETS)) {
    const state = createState(id);
    for (const edited of [false, true]) {
      if (edited) {
        setValue(state, "accent", "#ef0123");
        setValue(state, "bodySize", 19);
      }
      const files = getSeedFiles(state);
      const bytes = zipData(files);
      verifyZip(bytes, files);
      assert.deepEqual(zipData(files), bytes, "deterministic bytes");
      const blob = zipBytes(files);
      assert.equal(blob.type, "application/zip");
      assert.deepEqual(new Uint8Array(await blob.arrayBuffer()), bytes);
    }
  }
  verifyZip(zipData({ "日本語.txt": "hello 日本語" }), { "日本語.txt": "hello 日本語" });
});

test("shared definitions stay immutable while states and generated files are independent", () => {
  assert.equal(KEYS.length, 9);
  assert.equal(Object.isFrozen(PRESETS.everyday.values), true);
  assert.equal(Object.isFrozen(PRESETS.editorial.traits), true);
  assert.equal(Object.isFrozen(LIMITS.bodySize), true);
  const first = createState(),
    second = createState();
  assert.equal(setValue(first, "bodySize", 20), true);
  assert.equal(second.values.bodySize, 16);
  assert.equal(PRESETS.everyday.values.bodySize, 16);
  const files = getSeedFiles(first);
  files["design-system.css"] = "changed by caller";
  assert.notEqual(getSeedFiles(first)["design-system.css"], files["design-system.css"]);
  const poisoned = {
    preset: "toString",
    values: {
      accent: "</style><script>alert(1)</script>",
      headingFont: "constructor",
      bodySize: Infinity,
    },
  };
  assert.deepEqual(getSeedFiles(poisoned), getSeedFiles(createState()));
});
