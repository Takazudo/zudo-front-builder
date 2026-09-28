import type { ComponentChildren, VNode } from "preact";

/**
 * Alert components for the `githubAlerts` markdown feature.
 *
 * With `markdown.features.githubAlerts` enabled, a `> [!NOTE]` blockquote is
 * rewritten into a `<Note>` JSX element before rendering — one component name
 * per alert type, fixed by the feature (`Note`, `Tip`, `Important`,
 * `Warning`, `Caution`). Those names are resolved against the components map,
 * which is why `mdx-components.tsx` exports all five: a PascalCase name that
 * is missing from the map throws at render time.
 *
 * The same components are usable directly as JSX inside `.mdx` posts.
 */

type Variant = "note" | "tip" | "important" | "warning" | "caution";

type VariantSpec = {
  label: string;
  /**
   * Full utility strings rather than interpolated fragments — Tailwind
   * scans source text, so a class assembled at runtime is never generated.
   */
  tone: string;
  accent: string;
  icon: VNode;
};

const SPECS: Record<Variant, VariantSpec> = {
  note: {
    label: "Note",
    tone: "border-sky-500 bg-sky-50 dark:bg-sky-950/40",
    accent: "text-sky-700 dark:text-sky-300",
    icon: (
      <>
        <circle cx="12" cy="12" r="9" />
        <path d="M12 16v-4M12 8h.01" />
      </>
    ),
  },
  tip: {
    label: "Tip",
    tone: "border-emerald-500 bg-emerald-50 dark:bg-emerald-950/40",
    accent: "text-emerald-700 dark:text-emerald-300",
    icon: <path d="M12 3l2.1 5.4L19.5 10.5l-5.4 2.1L12 18l-2.1-5.4L4.5 10.5l5.4-2.1z" />,
  },
  important: {
    label: "Important",
    tone: "border-violet-500 bg-violet-50 dark:bg-violet-950/40",
    accent: "text-violet-700 dark:text-violet-300",
    icon: (
      <>
        <path d="M20 14a2 2 0 0 1-2 2H8l-4 4V5a2 2 0 0 1 2-2h12a2 2 0 0 1 2 2z" />
        <path d="M12 7v4M12 13h.01" />
      </>
    ),
  },
  warning: {
    label: "Warning",
    tone: "border-amber-500 bg-amber-50 dark:bg-amber-950/40",
    accent: "text-amber-700 dark:text-amber-300",
    icon: (
      <>
        <path d="M10.6 3.9 2.4 18a1.6 1.6 0 0 0 1.4 2.4h16.4A1.6 1.6 0 0 0 21.6 18L13.4 3.9a1.6 1.6 0 0 0-2.8 0z" />
        <path d="M12 9v4M12 17h.01" />
      </>
    ),
  },
  caution: {
    label: "Caution",
    tone: "border-rose-500 bg-rose-50 dark:bg-rose-950/40",
    accent: "text-rose-700 dark:text-rose-300",
    icon: (
      <>
        <path d="M8.1 2.5h7.8L21.5 8.1v7.8l-5.6 5.6H8.1L2.5 15.9V8.1z" />
        <path d="M12 8v4M12 16h.01" />
      </>
    ),
  },
};

type CalloutProps = {
  variant: Variant;
  children: ComponentChildren;
};

export function Callout({ variant, children }: CalloutProps) {
  const spec = SPECS[variant];
  return (
    <aside
      class={`my-6 rounded-md border-l-4 px-4 py-3 ${spec.tone}`}
      data-component={variant}
      role="note"
    >
      <p class={`flex items-center gap-2 text-sm font-semibold ${spec.accent}`}>
        <svg
          width="16"
          height="16"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          stroke-width="1.8"
          stroke-linecap="round"
          stroke-linejoin="round"
          aria-hidden="true"
        >
          {spec.icon}
        </svg>
        {spec.label}
      </p>
      <div class="mt-1 space-y-3">{children}</div>
    </aside>
  );
}

type AlertProps = { children: ComponentChildren };

export function Note({ children }: AlertProps) {
  return <Callout variant="note">{children}</Callout>;
}

export function Tip({ children }: AlertProps) {
  return <Callout variant="tip">{children}</Callout>;
}

export function Important({ children }: AlertProps) {
  return <Callout variant="important">{children}</Callout>;
}

export function Warning({ children }: AlertProps) {
  return <Callout variant="warning">{children}</Callout>;
}

export function Caution({ children }: AlertProps) {
  return <Callout variant="caution">{children}</Callout>;
}
