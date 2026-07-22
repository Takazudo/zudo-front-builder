// Consumer-facing declaration fixture for `parseToAst` (zfb#1857, epic
// zfb#1854). Mirrors consumer-compatibility-highlight.ts's role for the
// `./highlight` subpath: it imports only what a real consumer would import,
// so `pnpm typecheck:consumer` catches a broken/opaque result type as a
// type error rather than at runtime -- an opaque `unknown` `ast` would NOT
// satisfy requirement 3 of zfb#1828 (custom/unrecognized nodes must stay
// TYPED), and this file is what proves the shipped types actually let a
// consumer narrow on `type` and read a variant-specific field.
import {
  parseToAst,
  init,
  version,
  type ParseToAstOptions,
  type ParseToAstResult,
  type MdastNode,
  type Heading,
} from "../dist/index.js";

async function consumeParseToAstApi(): Promise<void> {
  const options: ParseToAstOptions = {
    filename: "post.md",
    dialect: "markdown",
    pipeline: { gfm: { table: true } },
  };
  const result: ParseToAstResult = await parseToAst("# hi\n", options);
  const first: MdastNode | undefined = result.ast?.children[0];

  if (first?.type === "heading") {
    // The catch-all `UnknownMdastNode` member's deliberately non-literal
    // `type: string` (see types.ts) keeps it in the narrowed union here --
    // a known, documented limitation of TypeScript discriminated unions
    // with an escape-hatch member. The runtime check above already proved
    // this is really a Heading; the assertion just tells the compiler.
    const heading: Heading = first as Heading;
    const depth: 1 | 2 | 3 | 4 | 5 | 6 = heading.depth;
    void depth;
  }

  await init();
  const packageVersion: string = await version();
  void packageVersion;
}

void consumeParseToAstApi;
