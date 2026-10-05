/** @jsxRuntime automatic */
/** @jsxImportSource preact */

import { CODE_BLOCK_ENHANCER_SCRIPT } from "@takazudo/zudo-doc/code-syntax";
import { buildWindPreviewEnhancerScript } from "./wind-preview-enhancer-script.mjs";

const windPreviewEnhancerScript = buildWindPreviewEnhancerScript(CODE_BLOCK_ENHANCER_SCRIPT);

export default function WindPreviewEnhancer() {
  return <script dangerouslySetInnerHTML={{ __html: windPreviewEnhancerScript }} />;
}
