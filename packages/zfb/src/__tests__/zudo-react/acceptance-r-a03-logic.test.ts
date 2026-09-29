// @vitest-environment node
import { describe, expect, it } from "vitest";

import { batch, computed, h, signal, type ReadonlySignal } from "../../zudo-react/index.js";
import { subscribe, subscriberCount } from "../../zudo-react/reactive.js";
import { flush } from "../../zudo-react/scheduler.js";
import { renderToString } from "../../zudo-react/server.js";

describe("R-A03 reactive logic without DOM bindings", () => {
  it("keeps signal and computed values live in child and attribute positions while .value stays a snapshot", async () => {
    const source = signal("alpha");
    const upper = computed(() => source.value.toUpperCase());
    let setupExecutions = 0;
    let returned!: ReturnType<typeof h>;

    function PropConsumer(props: { label: ReadonlySignal<string>; upper: ReadonlySignal<string> }) {
      setupExecutions++;
      const snapshot = props.label.value;
      returned = h(
        "p",
        { title: props.label, "aria-label": props.upper },
        props.label,
        " / ",
        snapshot,
        " / ",
        props.upper,
      );
      return returned;
    }

    const description = h(PropConsumer, { label: source, upper });
    expect(renderToString(description)).toBe(
      '<p title="alpha" aria-label="ALPHA">alpha / alpha / ALPHA</p>',
    );
    expect(setupExecutions).toBe(1);

    const observed: Array<{
      title: string;
      aria: string;
      child: string;
      snapshot: string;
      computedChild: string;
    }> = [];
    const children = returned.props.children as readonly unknown[];
    const observer = subscribe(() => {
      observed.push({
        title: (returned.props.title as ReadonlySignal<string>).value,
        aria: (returned.props["aria-label"] as ReadonlySignal<string>).value,
        child: (children[0] as ReadonlySignal<string>).value,
        snapshot: children[2] as string,
        computedChild: (children[4] as ReadonlySignal<string>).value,
      });
    });
    observer.run();
    expect(subscriberCount(source)).toBe(2);
    expect(subscriberCount(upper)).toBe(1);

    source.value = "beta";
    await flush();

    expect(observed).toEqual([
      { title: "alpha", aria: "ALPHA", child: "alpha", snapshot: "alpha", computedChild: "ALPHA" },
      { title: "beta", aria: "BETA", child: "beta", snapshot: "alpha", computedChild: "BETA" },
    ]);
    expect(setupExecutions).toBe(1);
    expect(renderToString(returned)).toBe(
      '<p title="beta" aria-label="BETA">beta / alpha / BETA</p>',
    );

    observer.dispose();
    expect(subscriberCount(source)).toBe(0);
    expect(subscriberCount(upper)).toBe(0);
  });

  it("replaces a computed value's dependency set after its branch changes", async () => {
    const chooseLeft = signal(true);
    const left = signal("left");
    const right = signal("right");
    const selected = computed(() => (chooseLeft.value ? left.value : right.value));
    const seen: string[] = [];
    const observer = subscribe(() => seen.push(selected.value));
    observer.run();

    expect(seen).toEqual(["left"]);
    expect(subscriberCount(left)).toBe(1);
    chooseLeft.value = false;
    await flush();
    expect(seen).toEqual(["left", "right"]);
    expect(subscriberCount(left)).toBe(0);
    expect(subscriberCount(right)).toBe(1);

    left.value = "ignored";
    await flush();
    expect(seen).toEqual(["left", "right"]);
    right.value = "right-updated";
    await flush();
    expect(seen).toEqual(["left", "right", "right-updated"]);

    observer.dispose();
    expect(subscriberCount(right)).toBe(0);
  });

  it("batches writes, settles computed diamonds, and resolves flush after queued work", async () => {
    const source = signal(1);
    const plusOne = computed(() => source.value + 1);
    const doubled = computed(() => source.value * 2);
    const total = computed(() => plusOne.value + doubled.value);
    const seen: number[] = [];
    const observer = subscribe(() => seen.push(total.value));
    observer.run();

    batch(() => {
      source.value = 2;
      expect(total.value).toBe(7);
      source.value = 3;
    });
    await flush();

    expect(seen).toEqual([4, 10]);
    observer.dispose();
    expect(subscriberCount(source)).toBe(0);
    expect(subscriberCount(plusOne)).toBe(0);
    expect(subscriberCount(doubled)).toBe(0);
  });
});
