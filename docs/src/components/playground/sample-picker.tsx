/** @jsxRuntime automatic */
/** @jsxImportSource preact */

export interface PlaygroundSample {
  id: string;
  label: string;
  value: string;
}

export interface SamplePickerProps {
  samples: readonly PlaygroundSample[];
  onPick: (sample: PlaygroundSample) => void;
  activeSampleId?: string;
  label?: string;
}

export default function SamplePicker({
  samples,
  onPick,
  activeSampleId,
  label = "Samples",
}: SamplePickerProps) {
  return (
    <fieldset className="m-0 border-0 p-0">
      <legend className="mb-vsp-2xs text-caption font-semibold text-muted">{label}</legend>
      <div className="flex flex-wrap gap-hsp-sm">
        {samples.map((sample) => {
          const active = sample.id === activeSampleId;

          return (
            <button
              key={sample.id}
              type="button"
              aria-pressed={active}
              className={`rounded border px-hsp-md py-vsp-xs text-small transition-colors focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2 ${
                active
                  ? "border-accent bg-accent text-bg"
                  : "border-muted bg-surface text-fg hover:border-accent"
              }`}
              onClick={() => onPick(sample)}
            >
              {sample.label}
            </button>
          );
        })}
      </div>
    </fieldset>
  );
}
