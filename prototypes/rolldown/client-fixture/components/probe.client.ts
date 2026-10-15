const marker = (id: string, value: string) => {
  const element = document.getElementById(id);
  if (!element) throw new Error(`missing ${id} marker`);
  element.textContent = value;
};

// The SSR bundle may import this module while rendering the page.
if (typeof document !== "undefined") {
  marker("entry", "ENTRY_READY");
  void (async () => {
    const { lazyValue } = await import("./lazy.ts");
    marker("lazy", lazyValue);
  })();

  const worker = new Worker(new URL("./worker.ts", import.meta.url), { type: "module" });
  worker.onerror = (event) => {
    marker("worker-error", event.message || "worker error");
  };
  worker.onmessage = (event) => {
    marker("worker", event.data.worker);
    marker("nested", event.data.nested);
  };
  worker.postMessage("go");
}
