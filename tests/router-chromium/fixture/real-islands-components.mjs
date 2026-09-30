// Shared component definitions for the built server renderer and browser client.
// Passing the real zudo-react API in keeps this module resolvable in both Node and Chromium.
export function createComponents({ h, signal, Show }) {
  function Toggle() {
    const shown = signal(false);
    return h(
      "section",
      { id: "toggle-content" },
      h(
        "button",
        { id: "toggle-button", type: "button", "on:click": () => (shown.value = !shown.value) },
        "toggle",
      ),
      h(Show, {
        when: shown,
        children: () =>
          h("svg", { id: "toggle-svg", viewBox: "0 0 10 10" }, h("path", { d: "M0 0L10 10" })),
        fallback: () => h("span", { id: "toggle-span" }, "off"),
      }),
    );
  }
  function Counter({ id, start }) {
    const count = signal(start);
    return h(
      "section",
      { id: `counter-${id}` },
      h("span", { id: `value-${id}` }, count),
      h("button", { id: `button-${id}`, type: "button", "on:click": () => count.value++ }, "+1"),
    );
  }
  function OtherCounter(props) {
    return Counter(props);
  }
  function Idle({ id }) {
    return h("span", { id: `idle-${id}` }, "idle ready");
  }
  return { Toggle, Counter, OtherCounter, Idle };
}
