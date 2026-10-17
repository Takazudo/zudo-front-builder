import answerModule from "../wasm/answer.wasm";

export default function WasmPage() {
  if (!(answerModule instanceof WebAssembly.Module)) {
    throw new Error("Wasm fixture expected an uninstantiated WebAssembly.Module");
  }
  const instance = new WebAssembly.Instance(answerModule);
  const answer = (instance.exports.answer as () => number)();
  return (
    <html lang="en">
      <head>
        <meta charset="utf-8" />
        <title>Wasm fixture</title>
      </head>
      <body>
        <p data-v8-fixture="wasm-answer">Wasm answer: {answer}</p>
      </body>
    </html>
  );
}
