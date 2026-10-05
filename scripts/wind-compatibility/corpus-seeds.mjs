// Xorshift32 is deliberately fixed; changing the seed or count changes the reviewed corpus.
export const seed = 0x3831c0de;
export const generatedCount = 8;
export const generatedWidthCount = 6;
export const generatedSourceCount = 6;

export function generatedCandidateLists() {
  let state = seed;
  const next = () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    return state >>> 0;
  };
  const base = ["relative", "flex", "flex-row", "flex"];
  return Array.from({ length: generatedCount }, () => {
    const values = [...base];
    for (let index = values.length - 1; index > 0; index--) {
      const swap = next() % (index + 1);
      [values[index], values[swap]] = [values[swap], values[index]];
    }
    return values;
  });
}

export function generatedWidths() {
  let state = seed ^ 0x57574454;
  const widths = [0, 1, 17];
  for (let index = 0; index < generatedWidthCount - 3; index++) {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    widths.push(2 + ((state >>> 0) % 254));
  }
  return widths;
}

export function generatedSourceStrings() {
  let state = seed ^ 0x48544d4c;
  return Array.from({ length: generatedSourceCount }, () => {
    state ^= state << 13;
    state ^= state >>> 17;
    state ^= state << 5;
    const reverse = (state & 1) === 1;
    const separator = [" ", "  ", "\n"][(state >>> 1) % 3];
    const classes = reverse ? `hidden${separator}block` : `block${separator}hidden`;
    return `<!doctype html><html><body><span class="${classes}">x</span></body></html>`;
  });
}

export async function shrinkWidthFailure(width, fails) {
  if (!(await fails(width))) throw Error("Cannot shrink a passing width");
  let smallest = width;
  for (const candidate of [0, 1, Math.floor(width / 2)]) {
    if (candidate < smallest && (await fails(candidate))) smallest = candidate;
  }
  return smallest;
}

export async function shrinkSourceFailure(source, fails) {
  if (!(await fails(source))) throw Error("Cannot shrink a passing source string");
  let smallest = source;
  const variants = [
    source.replace(/class="hidden\s+block"/, 'class="block hidden"'),
    source.replace(/class="block\s+hidden"/, 'class="block hidden"'),
    '<span class="block hidden">x</span>',
  ];
  for (const variant of variants) {
    if (variant.length < smallest.length && (await fails(variant))) smallest = variant;
  }
  return smallest;
}

export async function shrinkFailure(candidates, fails) {
  let smallest = [...candidates];
  if (!(await fails(smallest))) throw Error("Cannot shrink a passing specimen");
  for (let index = 0; index < smallest.length;) {
    const next = smallest.filter((_, current) => current !== index);
    if (next.length && (await fails(next))) smallest = next;
    else index++;
  }
  return smallest;
}
