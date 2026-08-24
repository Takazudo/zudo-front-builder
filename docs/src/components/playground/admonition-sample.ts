import type { PlaygroundSample } from "./sample-picker";

export const NOTE_DIRECTIVE_FEATURES = {
  directives: {
    note: "Note",
  },
} as const;

export type NoteDirectiveFeatures = typeof NOTE_DIRECTIVE_FEATURES;

export const NOTE_DIRECTIVE_SAMPLE: PlaygroundSample = {
  id: "admonition-directive",
  label: "Admonition directive",
  value: `# Admonition directive

:::note[Heads up]

The registered \`note\` directive becomes a \`<Note>\` component.

:::
`,
};

export function isNoteDirectiveSample(sample: PlaygroundSample): boolean {
  return sample.id === NOTE_DIRECTIVE_SAMPLE.id;
}

export function noteDirectiveFeatures(enabled: boolean): NoteDirectiveFeatures | undefined {
  return enabled ? NOTE_DIRECTIVE_FEATURES : undefined;
}
