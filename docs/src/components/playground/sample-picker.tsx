import { computed, type ReadonlySignal } from "@takazudo/zfb/zudo-react";

export interface PlaygroundSample {
  id: string;
  label: string;
  value: string;
}

export interface SamplePickerProps {
  samples: readonly PlaygroundSample[];
  onPick: (sample: PlaygroundSample) => void;
  activeSampleId: ReadonlySignal<string | undefined>;
  label?: string;
}

export default function SamplePicker({
  samples,
  onPick,
  activeSampleId,
  label = "Samples",
}: SamplePickerProps) {
  return (
    <fieldset class="m-0 border-0 p-0">
      <legend class="mb-vsp-2xs text-caption font-semibold text-muted">{label}</legend>
      <div class="flex flex-wrap gap-hsp-sm">
        {samples.map((sample) => {
          const active = computed(() => sample.id === activeSampleId.value);
          const className = computed(
            () =>
              `rounded border px-hsp-md py-vsp-xs text-small transition-colors focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 ${
                active.value
                  ? "border-accent bg-accent text-bg"
                  : "border-muted bg-surface text-fg hover:border-accent"
              }`,
          );

          return (
            <button
              type="button"
              aria-pressed={active}
              class={className}
              on:click={() => onPick(sample)}
            >
              {sample.label}
            </button>
          );
        })}
      </div>
    </fieldset>
  );
}
