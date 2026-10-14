import { render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import * as fixtures from "./fixtures.gen";
import { ReleaseMap } from "./ReleaseMap";
import { Slip } from "./Slip";

vi.mock("./api", () => ({ api: { copyText: async () => {} } }));

// Server and receipt text is untrusted (remote output, file contents, paths).
// Rust escapes it; these check the result end to end.

/** C0 and C1 controls (ANSI escapes start with one) and bidi formatting. */
// eslint-disable-next-line no-control-regex -- matching control characters is the point
const UNSAFE = /[\u0000-\u001f\u007f-\u009f\u061c\u200e\u200f\u202a-\u202e\u2066-\u2069]/;

function strings(value: unknown, path = "fixtures"): [string, string][] {
  if (typeof value === "string") return [[path, value]];
  if (Array.isArray(value)) return value.flatMap((item, i) => strings(item, `${path}[${i}]`));
  if (value && typeof value === "object") {
    return Object.entries(value).flatMap(([key, item]) => strings(item, `${path}.${key}`));
  }
  return [];
}

test("nothing the UI is sent holds a raw control or bidi character", () => {
  const all = strings(fixtures);
  expect(all.length).toBeGreaterThan(500);
  expect(all.filter(([, text]) => UNSAFE.test(text))).toEqual([]);
});

test("the fixtures do carry hostile server text, escaped", () => {
  const text = strings(fixtures).map(([, value]) => value);
  expect(text).toContain("<img src=x onerror=alert(1)> \\u{202e}fdp.exe");
  expect(text).toContain("Unsupported operand \\u{202e}gnp.exe\\x0a<b>bold</b>");
});

const map = fixtures.map.kind === "loaded" ? fixtures.map.map : (null as never);

function slip(repoRoot: string, env: string) {
  const row = map.rows.find((r) => r.repoRoot === repoRoot)!;
  const cell = row.cells[map.envs.indexOf(env)];
  if (cell.kind !== "present") throw new Error("fixture");
  const result = fixtures.receipts[String(cell.receipt)];
  if (result.kind !== "opened") throw new Error("fixture");
  render(<Slip opened={result.slip} cell={cell} label={row.label} env={env} context={null} onView={() => {}} />);
}

test("markup in step output is shown as text, never as elements", () => {
  slip("/Users/you/code/billing", "staging");
  expect(document.querySelector("img")).toBeNull();
  expect(screen.getByText(/<img src=x onerror=alert\(1\)> \\u\{202e\}fdp\.exe/)).toBeInTheDocument();
});

test("markup and a bidi override in a log message are shown as text", () => {
  slip("/Users/you/code/shop", "staging");
  const message = screen.getByRole("button", { name: /^TypeError: Unsupported operand/ });
  expect(within(message).queryByText("bold", { selector: "b" })).toBeNull();
  expect(message).toHaveTextContent("Unsupported operand \\u{202e}gnp.exe\\x0a<b>bold</b>");
});

test("the grid renders every fixture cell as text only", () => {
  const { container } = render(<ReleaseMap map={map} />);
  expect(container.querySelectorAll("img, script, iframe, b")).toHaveLength(0);
});
