import { signal } from "@takazudo/zfb/zudo-react";

const dynamicText = signal("abc");

function LeadingText() {
  return <>{"\n"}abc</>;
}

export default function PreLeadingLf(props: { variant: string }) {
  switch (props.variant) {
    case "leading-lf":
      return <pre>{"\nabc"}</pre>;
    case "leading-two-lf":
      return <pre>{"\n\nabc"}</pre>;
    case "plain":
      return <pre>abc</pre>;
    case "crlf":
      return <pre>{"\r\nabc"}</pre>;
    case "empty-sibling":
      return (
        <pre>
          {""}
          {"\nabc"}
        </pre>
      );
    case "nested-array":
      return <pre>{[["\nabc"]]}</pre>;
    case "component-fragment":
      return (
        <pre>
          <LeadingText />
        </pre>
      );
    case "code-child":
      return (
        <pre>
          <code>{"\nabc"}</code>
        </pre>
      );
    case "raw-html":
      return <pre rawHtml={"\nabc"} />;
    case "signal-island":
      return (
        <pre>
          {"\n"}
          {dynamicText}
        </pre>
      );
    default:
      throw new Error(`Unknown pre fixture variant: ${props.variant}`);
  }
}

export function run() {
  return { dynamicText };
}
