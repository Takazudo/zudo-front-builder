import { signal, type CSSProperties } from "@takazudo/zfb/zudo-react";

const styles = new Map<string, ReturnType<typeof signal<CSSProperties>>>();

function initialStyle(): CSSProperties {
  return {
    position: "absolute",
    left: "12px",
    top: "0.5rem",
    width: "120px",
    height: "80px",
    "max-height": "40px",
    "font-size": "16px",
    padding: 0,
    opacity: 0.5,
    "line-height": 1.25,
    "--zfb-style-number": 2,
    "-webkit-mask-image": "linear-gradient(black, transparent)",
    "-webkit-mask-size": "100% 100%",
  };
}

export default function StyleLiterals(props: { kind: string }) {
  const style = signal(initialStyle());
  styles.set(props.kind, style);
  return (
    <section>
      <span>Style fixture</span>
      <div id={`style-${props.kind}`} style={style}>
        Styled target
      </div>
    </section>
  );
}

export function run(index: number) {
  const kind = index === 0 ? "hydrate" : "mount";
  const style = styles.get(kind);
  if (!style) throw new Error(`Missing style signal for ${kind}`);
  return { style };
}
