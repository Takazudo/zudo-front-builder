import { expect } from "vite-plus/test";

export function workflowStepBlock(workflow, marker) {
  const start = workflow.indexOf(`      - ${marker}\n`);
  expect(start, `step "${marker}" not found in health.yml`).toBeGreaterThanOrEqual(0);
  const next = workflow.indexOf("\n      - ", start + 1);
  return workflow.slice(start, next === -1 ? undefined : next);
}

export function stepIf(block) {
  const match = block.match(/^ {8}if: (.+)$/m);
  if (!match) return null;
  return match[1].replace(/^\$\{\{\s*/, "").replace(/\s*\}\}$/, "");
}

// The subset of the GitHub Actions expression language health.yml uses here.
function evaluate(expression, ctx) {
  const tokens = expression.match(/\|\||&&|==|!=|!|\(|\)|'[^']*'|[A-Za-z_][\w.]*/g);
  let pos = 0;
  const peek = () => tokens[pos];
  const take = () => tokens[pos++];
  const primary = () => {
    const token = take();
    if (token === "(") {
      const value = or();
      expect(take()).toBe(")");
      return value;
    }
    if (token.startsWith("'")) return token.slice(1, -1);
    if (peek() === "(") {
      take();
      expect(take()).toBe(")");
      expect(ctx.functions, `unsupported function ${token}`).toHaveProperty(token);
      return ctx.functions[token];
    }
    const step = token.match(/^steps\.(\w+)\.outcome$/);
    expect(step, `unsupported operand ${token}`).not.toBeNull();
    return ctx.outcomes[step[1]];
  };
  const unary = () => (peek() === "!" ? (take(), !unary()) : comparison());
  const comparison = () => {
    const left = primary();
    if (peek() === "==" || peek() === "!=") {
      const op = take();
      const right = primary();
      return op === "==" ? left === right : left !== right;
    }
    return left;
  };
  const and = () => {
    let value = unary();
    while (peek() === "&&") {
      take();
      value = unary() && value;
    }
    return value;
  };
  const or = () => {
    let value = and();
    while (peek() === "||") {
      take();
      value = or() || value;
    }
    return value;
  };
  const value = or();
  expect(pos).toBe(tokens.length);
  return Boolean(value);
}

export function runs(block, { outcomes, failed = false, cancelled = false }) {
  let expression = stepIf(block) ?? "success()";
  if (!/\b(?:always|cancelled|failure|success)\(\)/.test(expression)) {
    expression = `success() && (${expression})`;
  }
  return evaluate(expression, {
    outcomes,
    functions: {
      always: true,
      cancelled,
      failure: failed && !cancelled,
      success: !failed && !cancelled,
    },
  });
}
