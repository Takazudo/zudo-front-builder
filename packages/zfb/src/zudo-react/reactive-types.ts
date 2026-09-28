export interface ReadonlySignal<T> {
  readonly value: T;
  readonly $$zudoReactive: "zudo-react.reactive.v1";
}

export interface Signal<T> extends ReadonlySignal<T> {
  value: T;
}
