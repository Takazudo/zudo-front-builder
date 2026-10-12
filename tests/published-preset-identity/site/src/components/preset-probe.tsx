"use client";

// One Island: host state rendered by a package component, plus a package
// component whose own signals must share the host's reactive runtime.
import { computed, getScope, signal } from "@takazudo/zfb/zudo-react";
import { Details } from "@takazudo/zudo-doc/details";
import { ThemeToggle } from "@takazudo/zudo-doc/theme-toggle";

type ProbeWindow = Window & { __presetProbe?: { activations: number; cleanups: number } };

export default function PresetProbe() {
  const scope = getScope();
  const active = signal(false);
  const clicks = signal(0);
  const theme = signal("unset");
  scope.onActivate(() => {
    const ledger = ((window as ProbeWindow).__presetProbe ??= { activations: 0, cleanups: 0 });
    ledger.activations += 1;
    active.value = true;
    const readTheme = () => {
      theme.value = document.documentElement.getAttribute("data-theme") ?? "unset";
    };
    readTheme();
    window.addEventListener("theme-preference-changed", readTheme);
    return () => {
      window.removeEventListener("theme-preference-changed", readTheme);
      ledger.cleanups += 1;
    };
  });
  const summary = computed(
    () => `${active.value ? "active" : "server"}:${clicks.value}:${theme.value}`,
  );
  return (
    <section data-preset-probe="">
      <div data-probe-package-toggle="">
        <ThemeToggle />
      </div>
      <Details title={summary}>
        <button
          type="button"
          data-probe-increment=""
          on:click={() => {
            clicks.value += 1;
          }}
        >
          Increment
        </button>
      </Details>
      <output data-probe-host="">{summary}</output>
    </section>
  );
}
