import { readFileSync } from "node:fs";
import { resolve } from "node:path";

// Vitest stubs CSS imports, so read the stylesheet itself. Tests run from web/.
const css = readFileSync(resolve(process.cwd(), "src/styles.css"), "utf8");

function tokens(selector: string): Record<string, string> {
  const start = css.indexOf(selector);
  const block = css.slice(css.indexOf("{", start) + 1, css.indexOf("}", start));
  const found = Object.fromEntries([...block.matchAll(/--([\w-]+):\s*(#[0-9a-f]{6})/g)].map((m) => [m[1]!, m[2]!]));
  expect(Object.keys(found).length).toBeGreaterThan(10);
  return found;
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r! + 0.7152 * g! + 0.0722 * b!;
}

function contrast(a: string, b: string): number {
  const [hi, lo] = [luminance(a), luminance(b)].sort((x, y) => y - x);
  return (hi! + 0.05) / (lo! + 0.05);
}

// Text tokens and the backgrounds they sit on, including status chips.
const PAIRS: Array<[string, string]> = [
  ["text-muted", "surface"],
  ["text-muted", "surface-2"],
  ["text-muted", "bg"],
  ["text-faint", "surface"],
  ["text-faint", "surface-2"],
  ["text-faint", "bg"],
  ["accent", "surface"],
  ["ok", "ok-soft"],
  ["warn", "warn-soft"],
  ["info", "info-soft"],
  ["danger", "danger-soft"],
];

describe("theme tokens", () => {
  const light = tokens(":root {");
  const dark = tokens(':root[data-theme="dark"] {');

  it.each([
    ["light", light],
    ["dark", dark],
  ])("%s text reaches 4.5:1 on its backgrounds", (_, theme) => {
    const failing = PAIRS.map(([text, background]) => ({
      text,
      background,
      ratio: Math.round(contrast(theme[text]!, theme[background]!) * 100) / 100,
    })).filter((pair) => pair.ratio < 4.5);
    expect(failing).toEqual([]);
  });

  it("follows the system dark scheme with the same dark tokens", () => {
    expect(tokens(':root:not([data-theme="light"]) {')).toEqual(dark);
  });
});
