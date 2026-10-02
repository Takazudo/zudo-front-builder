import { Island, clientScript } from "@takazudo/zfb";
import { CrossPipelineIsland } from "../components/cross-pipeline/Island";

export default function CrossPipelinePage() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>ZFB_CROSS_PIPELINE_PAGE</title>
        <script type="module" src={clientScript("cross-pipeline")} />
      </head>
      <body>
        <main>
          <h1>ZFB_CROSS_PIPELINE_PAGE</h1>
          <Island>
            <CrossPipelineIsland />
          </Island>
        </main>
      </body>
    </html>
  );
}
