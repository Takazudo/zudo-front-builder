import { describe, expect, it } from "vite-plus/test";
import { indexNpmPayload, readNpmPayload } from "../release/npm-tarball-members.mjs";

import { makeTarGz } from "./helpers/darwin-fixture.mjs";

const file = (name) => ({ name: `package/${name}`, typeflag: "0", data: Buffer.from("text") });
const directory = (name) => ({ name, typeflag: "5" });

describe("npm payload destination index", () => {
  it("accepts unique regular files with normal directory markers", () => {
    const readme = file("docs/readme.txt");
    const license = file("license.txt");
    const entries = [
      directory("package/"),
      readme,
      directory("package/docs"),
      directory("package/docs/"),
      license,
    ];
    expect([...indexNpmPayload(entries)]).toEqual([
      ["docs/readme.txt", readme],
      ["license.txt", license],
    ]);
  });

  it("indexes a unique PAX-resolved name with multibyte characters", () => {
    const text = " path=package/docs/café.txt\n";
    const pax = Buffer.from(`${Buffer.byteLength(text) + 2}${text}`);
    const payload = readNpmPayload(
      makeTarGz([
        { name: "PaxHeader", typeflag: "x", data: pax },
        file("fallback.txt"),
        file("readme.txt"),
      ]),
    );
    expect([...payload.keys()]).toEqual(["docs/café.txt", "readme.txt"]);
  });

  it("rejects repeated effective names even with identical data", () => {
    expect(() => indexNpmPayload([file("readme.txt"), file("readme.txt")])).toThrow(
      "duplicate file destination: package/readme.txt",
    );
  });

  it.each([
    [file("docs"), directory("package/docs/")],
    [directory("package/docs"), file("docs")],
    [file("docs"), file("docs/readme.txt")],
    [file("docs/readme.txt"), file("docs")],
  ])("rejects an explicit or implicit file/directory conflict", (first, second) => {
    expect(() => indexNpmPayload([first, second])).toThrow("file/directory conflict: package/docs");
  });
});
