import { Island } from "@takazudo/zfb";
import { h, signal } from "@takazudo/zfb/zudo-react";
import { hydrate } from "@takazudo/zfb/zudo-react/client";
import { renderToString } from "@takazudo/zfb/zudo-react/server";
import { createIslandTest, withIslandTestContext } from "@takazudo/zfb/zudo-react/testing";

export default function HarnessFixture() {
  return <p>Testing harness fixture</p>;
}

export async function run() {
  const value = signal("before");
  let clicks = 0;
  function Counter() {
    return (
      <button
        id="harness-counter"
        on:click={() => {
          clicks++;
        }}
      >
        {value}
      </button>
    );
  }

  const success = createIslandTest(Counter, {}, { document });
  const button = success.host.querySelector("button")!;
  const root = success.hydrate();
  const hydrated = root !== null && success.host.querySelector("button") === button;
  button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  value.value = "after";
  await success.flush();
  const flushedText = button.textContent;
  document.body.append(success.host);
  success.dispose();
  success.dispose();
  button.dispatchEvent(new MouseEvent("click", { bubbles: true }));
  value.value = "after-dispose";
  await success.flush();
  const cleanup = {
    rootDisposed: root?.disposed === true,
    hostRemoved: !success.host.isConnected,
    textAfterDispose: button.textContent,
    clicksAfterDispose: clicks,
  };

  function FailureProbe() {
    return <p>transport stays intact</p>;
  }
  const failure = createIslandTest(FailureProbe, {}, { document });
  failure.host.setAttribute("data-props", "{");
  const beforeFailure = failure.host.innerHTML;
  const failedRoot = failure.hydrate();
  const failureResult = {
    returnedNull: failedRoot === null,
    diagnostic: failure.diagnostics[0]?.code,
    domPreserved: failure.host.innerHTML === beforeFailure,
  };
  failure.dispose();

  function MountProbe() {
    return <p>client mount</p>;
  }
  const mounted = createIslandTest(MountProbe, {}, { document });
  const serverParagraph = mounted.host.querySelector("p");
  const mountedRoot = mounted.mount();
  const mountReplacedChildren =
    mountedRoot !== null && mounted.host.querySelector("p") !== serverParagraph;
  mounted.dispose();

  function FullPageCard() {
    return <strong>Full-page SDK Island</strong>;
  }
  const scope = globalThis as typeof globalThis & { __zfb?: Record<string, unknown> };
  const hadMetadata = Object.hasOwn(scope, "__zfb");
  const previousMetadata = scope.__zfb;
  const fullPage = withIslandTestContext(
    { components: [FullPageCard], build: "packed-browser" },
    () =>
      renderToString(
        <html>
          <head>
            <title>Harness integration</title>
          </head>
          <body>
            <main>
              <Island>
                <FullPageCard />
              </Island>
            </main>
          </body>
        </html>,
      ),
  );
  const metadataRestored = hadMetadata
    ? scope.__zfb === previousMetadata
    : !Object.hasOwn(scope, "__zfb");

  function ConsoleProbe() {
    return <p>console expected text</p>;
  }
  const consoleProbe = createIslandTest(ConsoleProbe, {}, { document });
  consoleProbe.host.querySelector("p")!.textContent = "console changed text";
  const consoleRoot = hydrate(h(ConsoleProbe, {}), consoleProbe.host, {
    identity: consoleProbe.identity,
  });
  consoleRoot?.unmount();
  consoleProbe.dispose();

  return {
    html: success.html,
    hydrated,
    flushedText,
    cleanup,
    failureResult,
    mountReplacedChildren,
    fullPageIncludesIsland:
      fullPage.includes('data-zfb-island="FullPageCard"') &&
      fullPage.includes('data-zfb-build="packed-browser"'),
    fullPageIncludesPageShell: fullPage.startsWith("<html><head>") && fullPage.includes("<main>"),
    metadataRestored,
  };
}
