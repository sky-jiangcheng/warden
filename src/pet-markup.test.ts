import { describe, expect, it } from "vitest";

// The desk pet is a vector character whose parts are addressed by class name
// from two files that never see each other at build time: the markup in
// index.html and the rules in styles.css. Nothing type-checks that pair — a
// renamed class is a silently invisible part (the pet keeps rendering, just
// without its eyes), and neither `npm run build` nor `tsc` notices.
//
// This is not hypothetical: the pet was replaced wholesale on 2026-11-17
// (bitmap cat → vector robot), and the failure mode of a half-done swap is
// exactly "one file still speaks the old vocabulary". So both directions are
// pinned: every part the CSS animates must exist in the markup, and no rule
// may still reference the retired cat's layers.

const fsName = "node:" + "fs";
const pathName = "node:" + "path";
const procName = "node:" + "process";

interface NodeFs {
  readFileSync(p: string, enc: string): string;
}
interface NodePath {
  resolve(...p: string[]): string;
}
interface NodeProcess {
  cwd(): string;
}

async function read(rel: string): Promise<string> {
  const fs = (await import(/* @vite-ignore */ fsName)) as unknown as NodeFs;
  const path = (await import(/* @vite-ignore */ pathName)) as unknown as NodePath;
  const proc = (await import(/* @vite-ignore */ procName)) as unknown as NodeProcess;
  return fs.readFileSync(path.resolve(proc.cwd(), rel), "utf8");
}

/// Parts the CSS moves or paints. A part missing from the markup means the
/// corresponding motion silently does nothing.
const PARTS = [
  "pet",
  "pet__bot",
  "pet__shell",
  "pet__screen",
  "pet__eyes",
  "pet__eye",
  "pet__iris",
  "pet__spark",
  "pet__eyes--smile",
  "pet__eye-smile",
  "pet__mouth--o",
  "pet__drool",
  "pet__drool-strand",
  "pet__drool-bead",
  "pet__eyes--closed",
  "pet__lid",
  "pet__antenna",
  "pet__bulb",
  "pet__arm",
  "pet__think",
  "pet__halo",
  "pet__shadow",
];

describe("desk pet markup ↔ stylesheet agreement", () => {
  it("every animated part exists in the markup", async () => {
    const html = await read("index.html");
    const missing = PARTS.filter((p) => !new RegExp(`class="[^"]*\\b${p}\\b`).test(html));
    expect(missing, "parts the CSS addresses but the markup never declares").toEqual([]);
  });

  it("the motion hooks the stylesheet expects are all addressed", async () => {
    const css = await read("src/styles.css");
    // data-mood / data-look are set by companion.ts and key every animation.
    for (const hook of ["data-mood", "data-look"]) {
      expect(css, `${hook} is what the CSS branches on`).toContain(hook);
    }
    // The gaze is two custom properties; without them the eyes cannot move.
    for (const v of ["--look-x", "--look-y"]) {
      expect(css).toContain(v);
    }
  });

  it("the retired bitmap cat leaves no rules behind", async () => {
    const css = await read("src/styles.css");
    // These class names belonged to the two-layer cat. A leftover rule is
    // dead weight at best and, if it ever matches, a wrong face at worst.
    for (const dead of ["pet__frame", "pet__body-frame", "pet__layer"]) {
      expect(css, `${dead} belongs to the removed cat`).not.toContain(dead);
    }
  });

  it("the pet does not reference the deleted bitmap assets", async () => {
    const html = await read("index.html");
    expect(html).not.toContain("assets/pet/");
  });
});
