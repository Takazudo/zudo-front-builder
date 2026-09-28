import { Island } from "@takazudo/zfb";

import ClientOnly from "../components/client-only";
import AssembledClass from "../components/assembled-class";
import styles from "../components/probe.module.css";
import "../styles/global.css";

export default function Home() {
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>Wind real-build assets fixture</title>
      </head>
      <body>
        <main>
          <h1>Wind real-build assets fixture</h1>
          <p class="asset-project">Project stylesheet asset</p>
          <p class="asset-package">Package stylesheet asset</p>
          <p class="asset-nested">Nested package stylesheet asset</p>
          <div class="ordinary-card global-authored-marker">An ordinary authored class</div>
          <div class={styles.moduleMarker}>CSS Modules marker</div>
          <img id="project-image" src="/assets/project-image.svg" alt="Project asset" />
          <AssembledClass />
          <Island when="load">
            <ClientOnly />
          </Island>
        </main>
      </body>
    </html>
  );
}
