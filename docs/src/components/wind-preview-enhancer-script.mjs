const SCRIPT_START = "(function () {\n";
const INITIAL_SCAN = "  // Run on initial load.";
const INSTALL_FLAG = "__zfbWindPreviewCodeEnhancerInstalled";

const LATE_PREVIEW_OBSERVER = `  // Mirror the stock body-end enhancer through its own button handler, preserving
  // its private ResizeObserver state without changing DOM or router events.
  var stockWrapMode = readWrapMode();
  var synchronizingStockWrap = false;
  // On an SPA-entered route, a stock button can expose its live state even when
  // sessionStorage was blocked after earlier interaction.
  var existingWrapButtons = document.querySelectorAll(".code-block-wrapper .code-btn-wrap");
  for (var existingIndex = 0; existingIndex < existingWrapButtons.length; existingIndex++) {
    var existingWrapper = existingWrapButtons[existingIndex].closest(".code-block-wrapper");
    var existingPre = existingWrapper && existingWrapper.querySelector("pre");
    if (existingPre && existingPre.clientWidth > 0 && existingPre.dataset.codeOverflow !== undefined) {
      stockWrapMode = existingWrapButtons[existingIndex].getAttribute("aria-pressed") === "true";
      wrapMode = stockWrapMode;
      break;
    }
  }

  function isBridgeWrapButton(button) {
    var owned = false;
    wrapButtons.forEach(function (candidate) {
      if (candidate === button) owned = true;
    });
    return owned;
  }

  function synchronizeStockWrapMode(next) {
    if (stockWrapMode === next) return;
    var buttons = document.querySelectorAll(".code-block-wrapper .code-btn-wrap");
    for (var index = 0; index < buttons.length; index++) {
      if (isBridgeWrapButton(buttons[index])) continue;
      // A hidden native button still has its handler. It updates the stock
      // closure and its ResizeObserver state, including currently hidden code.
      synchronizingStockWrap = true;
      try {
        buttons[index].click();
        stockWrapMode = next;
      } finally {
        synchronizingStockWrap = false;
      }
      break;
    }
  }

  document.addEventListener("click", function (event) {
    if (synchronizingStockWrap || !event.target || !event.target.closest) return;
    var button = event.target.closest(".code-block-wrapper .code-btn-wrap");
    if (!button) return;
    if (isBridgeWrapButton(button)) {
      synchronizeStockWrapMode(wrapMode);
    } else {
      stockWrapMode = !stockWrapMode;
      setWrapMode(stockWrapMode);
    }
  });

  // Both native after-swap scans must finish before selecting a stock button.
  document.addEventListener("zfb:after-swap", function () {
    queueMicrotask(function () {
      synchronizeStockWrapMode(wrapMode);
      setWrapMode(wrapMode);
    });
  });

  var latePreviewCodeObserver = new MutationObserver(function (records) {
    var foundPreviewCode = false;
    for (var recordIndex = 0; recordIndex < records.length && !foundPreviewCode; recordIndex++) {
      var addedNodes = records[recordIndex].addedNodes;
      for (var nodeIndex = 0; nodeIndex < addedNodes.length; nodeIndex++) {
        var node = addedNodes[nodeIndex];
        if (node.nodeType !== 1) continue;
        if (
          (node.matches && node.matches(".zd-html-preview-code pre.hi-root")) ||
          (node.querySelector && node.querySelector(".zd-html-preview-code pre.hi-root"))
        ) {
          foundPreviewCode = true;
          break;
        }
      }
    }
    if (foundPreviewCode) {
      wrapButtons.forEach(function (button, pre) {
        if (!pre.isConnected) {
          resizeObserver.unobserve(pre);
          wrapButtons.delete(pre);
        }
      });
      enhanceCodeBlocks();
      synchronizeStockWrapMode(wrapMode);
    }
  });
  latePreviewCodeObserver.observe(document.documentElement, { childList: true, subtree: true });
`;

function replaceUnique(source, marker, replacement, label) {
  const pieces = source.split(marker);
  if (pieces.length !== 2) {
    throw new Error(`Pinned CodeBlockEnhancer ${label} marker must occur exactly once`);
  }
  return `${pieces[0]}${replacement}${pieces[1]}`;
}

export function buildWindPreviewEnhancerScript(nativeScript) {
  if (typeof nativeScript !== "string" || !nativeScript.length) {
    throw new TypeError("Pinned CodeBlockEnhancer script must be a non-empty string");
  }
  if (!nativeScript.startsWith(SCRIPT_START)) {
    throw new Error("Pinned CodeBlockEnhancer script must start with its expected IIFE marker");
  }

  const guardedScript = `${SCRIPT_START}  if (window.${INSTALL_FLAG}) return;\n  window.${INSTALL_FLAG} = true;\n${nativeScript.slice(SCRIPT_START.length)}`;
  return replaceUnique(
    guardedScript,
    INITIAL_SCAN,
    `${LATE_PREVIEW_OBSERVER}${INITIAL_SCAN}`,
    "initial-scan",
  );
}
