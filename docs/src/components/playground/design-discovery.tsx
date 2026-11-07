// Rendered as a component, not inline MDX markup: MDX turns a line holding a
// tag plus text into a paragraph, which wrapped these cards in stray <p>s
// (an empty first grid cell) and mapped <strong> to the prose text color.
const MINIATURES = ["Everyday", "Editorial", "Workbench"] as const;

interface DesignDiscoveryProps {
  title: string;
  body: string;
  action: string;
  href: string;
}

export default function DesignDiscovery({ title, body, action, href }: DesignDiscoveryProps) {
  return (
    <div class="design-discovery">
      <div class="design-discovery-copy">
        <p class="design-discovery-kicker">zudo-wind</p>
        <h2>{title}</h2>
        <p>{body}</p>
        <a class="design-discovery-action" href={href}>
          {action}
        </a>
      </div>
      <div class="design-discovery-miniatures" role="img" aria-label={MINIATURES.join(", ")}>
        {MINIATURES.map((name) => (
          <div class={`design-discovery-mini ${name.toLowerCase()}`}>
            <span>FIELDNOTES</span>
            <strong>
              Small
              <br />
              ideas.
            </strong>
            <b>{name}</b>
            <i></i>
          </div>
        ))}
      </div>
    </div>
  );
}
