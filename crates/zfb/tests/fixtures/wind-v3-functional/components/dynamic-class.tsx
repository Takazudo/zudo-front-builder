export default function DynamicClass({ color }: { color: string }) {
  const className = `bg-${color}`;
  return <div class={className}>Dynamic class construction</div>;
}
