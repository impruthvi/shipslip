/* eslint-disable no-restricted-syntax -- white and black check the WCAG formula */
import { expect, test } from "vitest";
import css from "./styles.css?raw";

// Text contrast at least 4.5:1 (WCAG AA) for every text color on every
// background it is drawn on.

const darkCss = css.slice(css.indexOf("@media (prefers-color-scheme: dark)"));
const darkSlipCss = css.slice(css.indexOf("/* A lit receipt on a dark desk"));

// The first declaration in a block wins; later blocks override only what they name.
function token(name: string, theme: "light" | "dark" | "darkSlip" = "light"): string {
  const sources = { light: [css], dark: [darkCss, css], darkSlip: [darkSlipCss, css] }[theme];
  for (const source of sources) {
    const match = source.match(new RegExp(`--color-${name}:\\s*(#[0-9a-fA-F]{6})`));
    if (match) return match[1];
  }
  throw new Error(`no --color-${name}`);
}

function luminance(hex: string): number {
  const [r, g, b] = [1, 3, 5].map((i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4;
  });
  return 0.2126 * r + 0.7152 * g + 0.0722 * b;
}

function ratio(text: string, background: string, theme: "light" | "dark" | "darkSlip" = "light"): number {
  const [a, b] = [luminance(token(text, theme)), luminance(token(background, theme))].sort(
    (x, y) => y - x,
  );
  return (a + 0.05) / (b + 0.05);
}

const pairs: [text: string, background: string][] = [
  ["text", "surface"],
  ["text", "surface-raised"],
  ["text", "selection"],
  ["text", "surface-alt"],
  ["accent", "selection"],
  ["surface-raised", "accent"],
  ["text-muted", "surface"],
  ["text-muted", "surface-alt"],
  ["text-muted", "surface-raised"],
  ["text-muted", "selection"],
  ["status-bad", "surface"],
  ["status-bad", "surface-raised"],
  ["status-bad", "status-bad-soft"],
  ["status-bad", "selection"],
  ["status-good", "surface"],
  ["status-good", "surface-raised"],
  ["status-good", "selection"],
  ["status-attention", "surface"],
  ["status-attention", "surface-raised"],
  ["status-attention", "status-attention-soft"],
  ["status-attention", "selection"],
  ["on-primary", "primary"],
];

test.each(pairs)("%s on %s is at least 4.5:1", (text, background) => {
  expect(ratio(text, background)).toBeGreaterThanOrEqual(4.5);
});

test.each(pairs)("dark: %s on %s is at least 4.5:1", (text, background) => {
  expect(ratio(text, background, "dark")).toBeGreaterThanOrEqual(4.5);
});

// The receipt stays paper in dark mode, so its ink must still read on dimmed paper.
const slipPairs = pairs.filter(([text]) => !text.includes("primary"));
test.each(slipPairs)("dark receipt: %s on %s is at least 4.5:1", (text, background) => {
  expect(ratio(text, background, "darkSlip")).toBeGreaterThanOrEqual(4.5);
});

test("the ratio is computed the WCAG way", () => {
  expect(ratio("text", "text")).toBeCloseTo(1);
  expect(luminance("#ffffff")).toBeCloseTo(1);
  expect(luminance("#000000")).toBe(0);
});
