// A small portable component used across pages and layouts.
// Stays inside the portable-component contract:
// no framework-specific APIs (no signals, no useResource, no
// React-only hooks). Plain props in, JSX out.

export type HeaderProps = {
  title: string;
  subtitle?: string;
};

export default function Header({ title, subtitle }: HeaderProps) {
  return (
    <header class="site-header">
      <h1>{title}</h1>
      {subtitle ? <p class="subtitle">{subtitle}</p> : null}
    </header>
  );
}
