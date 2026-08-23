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
    <div className="flex flex-col gap-vsp-2xs border-b border-muted py-vsp-2xs last:border-b-0">
      <label className="flex cursor-pointer items-center gap-hsp-sm py-vsp-xs font-mono text-small text-fg">
        <input
          type="checkbox"
          className="accent-accent"
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
