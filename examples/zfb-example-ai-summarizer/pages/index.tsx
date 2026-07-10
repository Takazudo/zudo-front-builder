import { Island } from "@takazudo/zfb";

import SummarizeIsland from "../components/summarize-island";
import "../styles/global.css";

export default function HomePage() {
  return (
    <html lang="en">
      <head>
        <meta charSet="utf-8" />
        <meta name="viewport" content="width=device-width, initial-scale=1" />
        <title>AI Summarizer · zfb example</title>
      </head>
      <body>
        <main class="page-shell">
          <header class="app-header">
            <div>
              <p class="eyebrow">Workers AI</p>
              <h1>AI Summarizer</h1>
            </div>
            <a href="https://developers.cloudflare.com/workers-ai/" rel="noreferrer">
              Cloudflare docs
            </a>
          </header>
          <Island when="load">
            <SummarizeIsland />
          </Island>
        </main>
      </body>
    </html>
  );
}
