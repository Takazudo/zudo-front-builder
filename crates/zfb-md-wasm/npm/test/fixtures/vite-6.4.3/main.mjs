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

window.runFixture = async () => {
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

  const highlightOnlyResult = await highlightOnly("const subpath = true;", {
    language: "javascript",
  });
  const highlightState = getHighlightRecoveryState();

  return {
    transientError,
    trapName,
    parsed,
    highlighted,
    recovered,
    beforeTrap,
    afterTrap,
    highlightOnlyResult,
    highlightState,
  };
};
