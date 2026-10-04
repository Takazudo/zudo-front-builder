// @vitest-environment node
import { describe, expect, it } from "vite-plus/test";
import {
  computed,
  readSnapshot,
  signal,
  subscribe,
  subscriberCount,
} from "../../zudo-react/reactive.js";
import { batch, flush } from "../../zudo-react/scheduler.js";

describe("reactive core", () => {
  it("uses Object.is and only assignment not mutation", async () => {
    const number = signal(NaN);
    const seen: number[] = [];
    const sub = subscribe(() => seen.push(number.value));
    sub.run();
    number.value = NaN;
    await flush();
    expect(seen).toHaveLength(1);
    number.value = -0;
    await flush();
    number.value = 0;
    await flush();
    expect(seen).toHaveLength(3);
    const object = signal({ n: 1 });
    const objectSub = subscribe(() => {
      void object.value;
      seen.push(9);
    });
    objectSub.run();
    object.value.n = 2;
    await flush();
    expect(seen).toHaveLength(4);
    sub.dispose();
    objectSub.dispose();
  });
  it("distinguishes snapshot reads and live reads", async () => {
    const source = signal(1);
    const seen: number[] = [];
    const sub = subscribe(() => seen.push(source.value));
    sub.run();
    const snapshot = readSnapshot(source);
    source.value = 2;
    await flush();
    expect(snapshot).toBe(1);
    expect(seen).toEqual([1, 2]);
    sub.dispose();
  });
  it("settles a diamond once and reads the latest value in a batch", async () => {
    const source = signal(1);
    const left = computed(() => source.value + 1);
    const right = computed(() => source.value * 2);
    const total = computed(() => left.value + right.value);
    const seen: number[] = [];
    const sub = subscribe(() => seen.push(total.value));
    sub.run();
    batch(() => {
      source.value = 2;
      expect(total.value).toBe(7);
      source.value = 3;
    });
    await flush();
    expect(seen).toEqual([4, 10]);
    sub.dispose();
    expect(subscriberCount(source)).toBe(0);
    expect(subscriberCount(left)).toBe(0);
    expect(subscriberCount(right)).toBe(0);
  });
  it("replaces stale dependency edges", async () => {
    const choice = signal(true),
      a = signal(1),
      b = signal(2);
    const selected = computed(() => (choice.value ? a.value : b.value));
    const seen: number[] = [];
    const sub = subscribe(() => seen.push(selected.value));
    sub.run();
    choice.value = false;
    await flush();
    expect(subscriberCount(a)).toBe(0);
    a.value = 5;
    await flush();
    expect(seen).toEqual([1, 2]);
    b.value = 6;
    await flush();
    expect(seen).toEqual([1, 2, 6]);
    sub.dispose();
  });
  it("diagnoses cycles and writes inside computed evaluation", () => {
    let first!: ReturnType<typeof computed<number>>;
    let second!: ReturnType<typeof computed<number>>;
    first = computed(() => second.value + 1);
    second = computed(() => first.value + 1);
    expect(() => first.value).toThrow(/ZR_COMPUTED_CYCLE.*computed:.*computed:/);
    const source = signal(1);
    const invalid = computed(() => {
      source.value = 2;
      return 1;
    });
    expect(() => invalid.value).toThrow(/ZR_COMPUTED_WRITE/);
  });
  it("does not retain upstream subscriptions for an unobserved computed", () => {
    const source = signal(1);
    const derived = computed(() => source.value + 1);
    expect(derived.value).toBe(2);
    expect(subscriberCount(source)).toBe(0);
    source.value = 2;
    expect(derived.value).toBe(3);
  });
});
