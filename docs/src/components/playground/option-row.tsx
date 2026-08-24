/** @jsxRuntime automatic */
/** @jsxImportSource preact */

import type { ComponentChildren } from "preact";

export interface OptionRowProps {
  label: string;
  checked: boolean;
  onChange: (checked: boolean) => void;
  children?: ComponentChildren;
  disabled?: boolean;
}

export default function OptionRow({
  label,
  checked,
  onChange,
  children,
  disabled = false,
}: OptionRowProps) {
  return (
    <div
      className={`flex flex-col gap-vsp-2xs border-b border-muted last:border-b-0${children ? " pb-vsp-2xs" : ""}`}
    >
      <label className="flex min-h-[44px] min-w-0 cursor-pointer items-center gap-hsp-sm font-mono text-small text-fg [overflow-wrap:anywhere]">
        <input
          type="checkbox"
          className="accent-accent focus-visible:outline-2 focus-visible:outline-accent focus-visible:outline-offset-2"
          checked={checked}
          disabled={disabled}
          onChange={(event) => onChange(event.currentTarget.checked)}
        />
        {label}
      </label>
      {children ? (
        <fieldset
          aria-label={`${label} options`}
          className="m-0 flex flex-wrap gap-hsp-md border-0 p-0 pl-hsp-xl text-small transition-opacity disabled:opacity-50"
          disabled={disabled || !checked}
        >
          {children}
        </fieldset>
      ) : null}
    </div>
  );
}
