// Consumer-facing declaration fixture for `parseToAst` (zfb#1857, epic
// zfb#1854). Mirrors consumer-compatibility-highlight.ts's role for the
// `./highlight` subpath: it imports only what a real consumer would import,
// so `pnpm typecheck:consumer` catches a broken/opaque result type as a
// type error rather than at runtime -- an opaque `unknown` `ast` would NOT
// satisfy requirement 3 of zfb#1828 (custom/unrecognized nodes must stay
// TYPED), and this file is what proves the shipped types actually let a
// consumer narrow on `type` and read a variant-specific field. The raw tier
// keeps that escape hatch; the validated adapter below intentionally returns
// the ecosystem's closed, canonical union.
import type { Root as EcosystemRoot } from "mdast";
import type {} from "mdast-util-to-hast";

import {
  parseToAst,
  toMdastRoot,
  init,
  version,
  type ParseToAstOptions,
  type ParseToAstResult,
  type MdastNode,
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
    // with an escape-hatch member. The raw tier therefore keeps depth broad.
    const depth: unknown = first.depth;
    void depth;
  }

  const canonical: EcosystemRoot = toMdastRoot(result.ast);
  const canonicalFirst = canonical.children[0];
  if (canonicalFirst?.type === "heading") {
    const depth: 1 | 2 | 3 | 4 | 5 | 6 = canonicalFirst.depth;
    canonicalFirst.data ??= {};
    canonicalFirst.data.hName = `h${depth}`;
  }

  await init();
  const packageVersion: string = await version();
  void packageVersion;
}

void consumeParseToAstApi;
