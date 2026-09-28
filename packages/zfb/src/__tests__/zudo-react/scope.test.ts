// @vitest-environment node
import { expect, it } from "vitest";
import { createScope, getScope, withScope } from "../../zudo-react/scope.js";
import { signal } from "../../zudo-react/reactive.js";
import { flush } from "../../zudo-react/scheduler.js";

it("activates children before parents and cleans up in reverse order", () => {
  const seen: string[] = [];
  const parent = createScope({ component: "Parent" });
  const child = parent.child("Child");
  withScope(parent, () => {
    expect(getScope()).toBe(parent);
    parent.onActivate(() => {
      seen.push("parent");
      return () => seen.push("parent activation cleanup");
    });
    parent.onCleanup(() => seen.push("parent first"));
    parent.onCleanup(() => seen.push("parent last"));
  });
  withScope(child, () => {
    child.onActivate(() => {
      seen.push("child");
    });
    child.onCleanup(() => seen.push("child cleanup"));
  });
  parent.activate();
  expect(seen).toEqual(["child", "parent"]);
  parent.dispose();
  parent.dispose();
  expect(seen).toEqual([
    "child",
    "parent",
    "child cleanup",
    "parent activation cleanup",
    "parent last",
    "parent first",
  ]);
});

it("aborts, reports cleanup errors, and continues cleanup", () => {
  const seen: string[] = [];
  const codes: string[] = [];
  const scope = createScope({
    component: "Demo",
    reporter: (diagnostic) => codes.push(`${diagnostic.component}:${diagnostic.code}`),
  });
  const abort = scope.abortSignal;
  withScope(scope, () => {
    scope.onCleanup(() => seen.push("first"));
    scope.onCleanup(() => {
      throw new Error("broken");
    });
  });
  scope.dispose();
  expect(abort.aborted).toBe(true);
  expect(seen).toEqual(["first"]);
  expect(codes).toEqual(["Demo:ZR_CLEANUP_ERROR"]);
});

it("starts effects after activation, cleans before rerun and disposal, and skips queued disposed work", async () => {
  const state = signal(1);
  const seen: string[] = [];
  const scope = createScope({ component: "Effect" });
  withScope(scope, () =>
    scope.effect(() => {
      const value = state.value;
      seen.push(`run ${value}`);
      return () => seen.push(`cleanup ${value}`);
    }),
  );
  state.value = 2;
  await flush();
  expect(seen).toEqual([]);
  scope.activate();
  await flush();
  state.value = 3;
  await flush();
  state.value = 4;
  scope.dispose();
  await flush();
  expect(seen).toEqual(["run 2", "cleanup 2", "run 3", "cleanup 3"]);
});

it("restores setup context after a throw and rejects acquisition outside setup", () => {
  const scope = createScope({ component: "Throw" });
  expect(() =>
    withScope(scope, () => {
      expect(getScope()).toBe(scope);
      throw new Error("setup");
    }),
  ).toThrow("setup");
  expect(() => getScope()).toThrow(/ZR_NO_SCOPE/);
  scope.dispose();
});

it("reports a throwing effect without stopping another effect or losing retry dependencies", async () => {
  const state = signal(1);
  const seen: string[] = [];
  const codes: string[] = [];
  const scope = createScope({
    component: "Effects",
    reporter: (diagnostic) => codes.push(diagnostic.code),
  });
  withScope(scope, () => {
    scope.effect(() => {
      const value = state.value;
      seen.push(`throw ${value}`);
      throw new Error("bad effect");
    });
    scope.effect(() => {
      seen.push(`other ${state.value}`);
    });
  });
  scope.activate();
  await flush();
  state.value = 2;
  await flush();
  expect(seen).toEqual(["throw 1", "other 1", "throw 2", "other 2"]);
  expect(codes).toEqual(["ZR_EFFECT_ERROR", "ZR_EFFECT_ERROR"]);
  scope.dispose();
});

it("disposes the root when child activation fails", () => {
  const codes: string[] = [];
  const root = createScope({
    component: "Root",
    reporter: (diagnostic) => codes.push(diagnostic.code),
  });
  const child = root.child("Child");
  withScope(child, () =>
    child.onActivate(() => {
      throw new Error("activate");
    }),
  );
  expect(() => root.activate()).toThrow("activate");
  expect(root.active).toBe(false);
  expect(child.active).toBe(false);
  expect(codes).toEqual(["ZR_ACTIVATION_ERROR"]);
});

it("diagnoses promise-valued cleanup", async () => {
  const codes: string[] = [];
  const scope = createScope({
    component: "AsyncCleanup",
    reporter: (diagnostic) => codes.push(diagnostic.code),
  });
  withScope(scope, () => {
    scope.effect(() => () => Promise.resolve());
    scope.onCleanup(() => Promise.resolve());
  });
  scope.activate();
  await flush();
  scope.dispose();
  expect(codes).toEqual(["ZR_ASYNC_CLEANUP", "ZR_ASYNC_CLEANUP"]);
});
