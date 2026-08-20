import * as root from "@takazudo/zfb-md-wasm";
import * as highlight from "@takazudo/zfb-md-wasm/highlight";
import * as render from "@takazudo/zfb-md-wasm/render";
import * as parse from "@takazudo/zfb-md-wasm/parse";

const compiled = await root.compile("# Root\n", { filename: "root.mdx" });
const highlighted = await highlight.highlightCode("const packed = true;", {
  language: "javascript",
});
const rendered = await render.renderHtml("# Render\n");
const parsed = await parse.parseToAst("# Parse\n");

if (typeof compiled.code !== "string" || !compiled.code.includes("MDXContent")) {
  throw new Error("packed root compile did not return an MDX module");
}
if (highlighted.diagnostics.length !== 0 || !highlighted.html.includes("packed")) {
  throw new Error("packed highlight call did not round-trip");
}
if (rendered.diagnostics.length !== 0 || rendered.html !== "<h1>Render</h1>") {
  throw new Error("packed render call did not round-trip");
}
if (parsed.diagnostics.length !== 0 || parsed.ast.type !== "root") {
  throw new Error("packed parse call did not return a root");
}

const versions = await Promise.all([
  root.version(),
  highlight.version(),
  render.version(),
  parse.version(),
]);
if (!versions.every((version) => /^\d+\.\d+\.\d+/.test(version))) {
  throw new Error(`packed versions were not semver-like: ${versions.join(", ")}`);
}

console.log(
  JSON.stringify({
    compiled: compiled.code.length,
    highlighted: highlighted.html.length,
    rendered: rendered.html,
    parsed: parsed.ast.type,
    versions,
  }),
);
