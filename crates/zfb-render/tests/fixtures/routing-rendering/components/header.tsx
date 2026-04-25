// A small portable component used across pages and layouts.
// Stays inside the portable-component contract from ADR-002:
// no framework-specific APIs (no signals, no useResource, no
// React-only hooks). Plain props in, JSX out.

export type HeaderProps = {
  title: string;
  subtitle?: string;
};

export default function Header({ title, subtitle }: HeaderProps) {
  return (
    <header className="site-header">
      <h1>{title}</h1>
      {subtitle ? <p className="subtitle">{subtitle}</p> : null}
    </header>
  );
}
