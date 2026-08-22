import {
  __forceTrapForTests,
  __getTrapRecoveryStateForTests,
  highlightCode,
  init,
  parseToAst,
} from "@takazudo/zfb-md-wasm";
import {
  __getTrapRecoveryStateForTests as getHighlightRecoveryState,
  highlightCode as highlightOnly,
} from "@takazudo/zfb-md-wasm/highlight";
import {
  __forceTrapForTests as forceRenderTrap,
  __getTrapRecoveryStateForTests as getRenderRecoveryState,
  renderHtml,
} from "@takazudo/zfb-md-wasm/render";
import {
  __getTrapRecoveryStateForTests as getParseRecoveryState,
  parseToAst as parseOnly,
} from "@takazudo/zfb-md-wasm/parse";

window.runFixture = async (action = "root") => {
  if (action === "highlight") {
    return {
      result: await highlightOnly("const subpath = true;", { language: "javascript" }),
      state: getHighlightRecoveryState(),
    };
  }
  if (action === "render") {
    return {
      result: await renderHtml("# Render subpath\n"),
      state: getRenderRecoveryState(),
    };
  }
  if (action === "parse") {
    return {
      result: await parseOnly("# Parse subpath\n"),
      state: getParseRecoveryState(),
    };
  }
  if (action === "coexist") {
    const beforeRender = getRenderRecoveryState();
    const beforeParse = getParseRecoveryState();
    const rendered = await renderHtml("# Render coexistence\n");
    const parsed = await parseOnly("# Parse coexistence\n");
    let trapName;
    try {
      await forceRenderTrap();
    } catch (error) {
      trapName = error?.name;
    }
    const parsedAfterRenderTrap = await parseOnly("# Parse remains healthy\n");
    return {
      rendered,
      parsed,
      parsedAfterRenderTrap,
      trapName,
      beforeRender,
      beforeParse,
      afterRender: getRenderRecoveryState(),
      afterParse: getParseRecoveryState(),
    };
  }

  if (action !== "root") {
    throw new Error(`unknown packed fixture action: ${action}`);
  }

  let transientError;
  try {
    await init();
  } catch (error) {
    transientError = error instanceof Error ? error.message : String(error);
  }
  if (!transientError) {
    throw new Error("the first Wasm request unexpectedly succeeded");
  }

  await init();
  const beforeTrap = __getTrapRecoveryStateForTests();
  const parsed = await parseToAst("# Vite\n\nFixture.");
  // Combined packed-consumer regressions: list-end UTF-16 positions (#1916)
  // and structured UTF-16 diagnostics whose message remains opaque (#1915).
  const listSource = "- 日本 😀\n\nnext\n";
  const list = await parseToAst(listSource, { filename: "list.md", frontmatter: "none" });
  const diagnostic = await parseToAst("😀<a></b>", { filename: "diagnostic.mdx" });
  const highlighted = await highlightCode("const vite = true;", {
    language: "javascript",
  });

  let trapName;
  try {
    await __forceTrapForTests();
  } catch (error) {
    trapName = error?.name;
  }
  const recovered = await highlightCode("const recovered = true;", {
    language: "javascript",
  });
  const afterTrap = __getTrapRecoveryStateForTests();

  return {
    transientError,
    trapName,
    parsed,
    listSource,
    list,
    diagnostic,
    highlighted,
    recovered,
    beforeTrap,
    afterTrap,
  };
};

window.runRetry = async (action) => {
  if (action === "render") {
    let firstError;
    try {
      await renderHtml("# Retry render\n");
    } catch (error) {
      firstError = error instanceof Error ? error.message : String(error);
    }
    const result = await renderHtml("# Retry render\n");
    return { firstError, result, state: getRenderRecoveryState() };
  }
  if (action === "parse") {
    let firstError;
    try {
      await parseOnly("# Retry parse\n");
    } catch (error) {
      firstError = error instanceof Error ? error.message : String(error);
    }
    const result = await parseOnly("# Retry parse\n");
    return { firstError, result, state: getParseRecoveryState() };
  }
  throw new Error(`unknown packed retry action: ${action}`);
};
