# Everyday starter design

These are editable project-owned choices, not zudo-wind engine defaults.

## Usage rules

- Use accent for primary actions, selected states and keyboard focus. Keep text, notice borders and supporting content neutral.
- Use 8px corners for cards, fields and buttons. Keep surfaces flat.
- Use 16px within vertical groups and 40px between sections. Horizontal card padding is 24px; column gutters are independently 16px.
- Use 16px body text with 1.65 line height and a reading measure of 60ch. Headings use sans; the body uses a clear sans-serif.

## Make it yours

Edit styles/design-system.css for values and design-tokens.json for semantic
names. Basic blog merges those tokens into zfb.config.ts, retaining its Markdown
showcase palette. Node-free keeps its explicit Wind section in zfb.config.json;
update it too if you rename a token. Update design-rules.ts and these notes
when changing values; the home page reads those rules. Neither starter needs
the workshop at runtime.

The accent chain is --ds-brand → --ds-accent → bg-accent. Body text uses
--ds-font-body → text-body. Use px-hsp-card for horizontal padding and
py-vsp-stack for vertical grouping; gap-x-hsp-gutter names column spacing
independently of gap-y-vsp-stack. max-w-reading limits prose, while
rounded-panel shares the corner rule. Basic blog's local dark overrides live
in styles/global.css and follow its existing theme toggle.

Explore alternatives in the [Design system playground](https://zfb.takazudomodular.com/docs/playground/design-system/).
Choosing another design in the playground does not change the initializer.
