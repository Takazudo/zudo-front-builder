export function currentWindSpecIdentity(profile) {
  const baseline = profile?.sourceBaseline;
  if (
    profile?.profileId !== "wind-preset-free" ||
    profile?.profileVersion !== 1 ||
    profile?.profileRevision !== 4 ||
    baseline?.languageSpecVersion !== 1 ||
    baseline?.languageSpecRevision !== 14
  )
    throw Error("Wind profile/spec identity differs from pilot adapter");
  return {
    windSpecVersion: baseline.languageSpecVersion,
    windSpecRevision: baseline.languageSpecRevision,
  };
}

export function assertWindSpecIdentity(profile, source) {
  const baseline = profile?.sourceBaseline;
  currentWindSpecIdentity(profile);
  const declarations = [...source.matchAll(/^pub const SPEC_(VERSION|REVISION): u32 = (\d+);$/gm)];
  if (
    declarations.length !== 2 ||
    declarations[0][1] !== "VERSION" ||
    declarations[1][1] !== "REVISION" ||
    Number(declarations[0][2]) !== baseline.languageSpecVersion ||
    Number(declarations[1][2]) !== baseline.languageSpecRevision
  )
    throw Error("Wind source spec identity differs from pilot adapter");
  return true;
}
