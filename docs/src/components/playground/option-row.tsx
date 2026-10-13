import { computed, type Child, type Signal } from "@takazudo/zfb/zudo-react";

export interface OptionRowProps {
  label: string;
  checked: Signal<boolean>;
  children?: Child;
  disabled?: boolean;
}

export default function OptionRow({ label, checked, children, disabled = false }: OptionRowProps) {
  const nestedDisabled = computed(() => disabled || !checked.value);

  return (
    <div
      class={`flex flex-col gap-vsp-2xs border-b border-muted last:border-b-0${children ? " pb-vsp-2xs" : ""}`}
    >
      <label class="flex min-h-[44px] min-w-0 cursor-pointer items-center gap-hsp-sm font-mono text-small text-fg wrap-anywhere">
        <input
          type="checkbox"
          class="accent-accent focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
          modelChecked={checked}
          disabled={disabled}
        />
        {label}
      </label>
      {children ? (
        <fieldset
          aria-label={`${label} options`}
          class="m-0 flex flex-wrap gap-hsp-md border-0 p-0 pl-hsp-xl text-small transition-opacity disabled:opacity-50"
          disabled={nestedDisabled}
        >
          {children}
        </fieldset>
      ) : null}
    </div>
  );
}
