import type { Island as TypeOnlyIsland } from "not-an-sdk";
import { Island as Boundary } from "@takazudo/zfb";
import { h as makeNode } from "@takazudo/zfb/zudo-react";
import { jsx as makeJsx, jsxs as makeJsxs } from "@takazudo/zfb/zudo-react/jsx-runtime";
import { jsxDEV as makeJsxDev } from "@takazudo/zfb/zudo-react/jsx-dev-runtime";
import PackedDefault, {
  PackedCounter as PackedNamed,
  PackedForwardBoundary,
  PackedOuterForwardBoundary,
  PackedFixedBoundary,
  PackedJsxsBoundary,
  PackedDevBoundary,
  ShadowTarget,
} from "@fixture/widgets";
import * as Widgets from "@fixture/widgets";
import { ConsumerA } from "../components/consumer-a";
import { ConsumerB } from "../components/consumer-b";
import { Island as UnrelatedIsland } from "../components/fake-island";
import { NeverRegistered } from "../components/unregistered";
import { LiveCounter } from "../components/live-counter";
import { ForwardBoundary, OuterForwardBoundary } from "../components/wrappers";
import {
  InferredArrow as ArrowAlias,
  ExportedExpression as ExpressionAlias,
  EqualDisplayName,
} from "../components/function-shapes";
import NamedDefault from "../components/named-default";
import AnonymousDefault from "../components/anonymous-default";

void (0 as unknown as TypeOnlyIsland);

export default function Home() {
  function ShadowedBoundary({ children }: { children: unknown }) {
    const Island = UnrelatedIsland;
    return <Island>{children as never}</Island>;
  }

  return (
    <html lang="en">
      <head>
        <title>Boundary acceptance</title>
      </head>
      <body>
        <Boundary>
          <ConsumerA label="Consumer A" />
        </Boundary>
        <Boundary children={<ConsumerB label="Consumer B" />} />
        {Boundary({ children: makeNode(LiveCounter, { label: "Live helper" }) })}
        {makeNode(Boundary, { children: makeNode(ArrowAlias, { label: "Arrow alias" }) })}
        {makeJsx(Boundary, { children: makeJsx(PackedDefault, { label: "Packed default" }) })}
        {makeJsxs(Boundary, {
          children: [makeJsx(Widgets.PackedCounter, { label: "Packed namespace" })],
        })}
        {makeJsxDev(
          Boundary,
          {
            children: makeJsxDev(
              PackedNamed,
              { label: "Packed named" },
              undefined,
              false,
              null,
              null,
            ),
          },
          undefined,
          false,
          null,
          null,
        )}
        <ForwardBoundary>
          <LiveCounter label="Forwarded local" />
        </ForwardBoundary>
        <OuterForwardBoundary>
          <ConsumerB label="Forwarded local chain" />
        </OuterForwardBoundary>
        <PackedForwardBoundary>
          <PackedNamed label="Forwarded package" />
        </PackedForwardBoundary>
        <PackedOuterForwardBoundary>
          <PackedNamed label="Forwarded package chain" />
        </PackedOuterForwardBoundary>
        <PackedFixedBoundary />
        <PackedJsxsBoundary />
        <PackedDevBoundary />
        <Boundary>
          <ShadowTarget label="Explicit star shadow" />
        </Boundary>
        <Boundary>
          <ExpressionAlias label="Named function expression" />
        </Boundary>
        <Boundary>
          <NamedDefault label="Named default" />
        </Boundary>
        <Boundary>
          <AnonymousDefault label="Anonymous default" />
        </Boundary>
        <Boundary>
          <EqualDisplayName label="Equal displayName" />
        </Boundary>
        <UnrelatedIsland>
          <NeverRegistered label="unrelated Island export" />
        </UnrelatedIsland>
        <ShadowedBoundary>
          <NeverRegistered label="shadowed Island binding" />
        </ShadowedBoundary>
      </body>
    </html>
  );
}
