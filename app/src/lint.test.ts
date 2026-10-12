/* eslint-disable no-restricted-syntax -- fixtures for the raw-hex rule */
import { ESLint } from "eslint";
import { describe, expect, test } from "vitest";

const eslint = new ESLint();

async function ruleIds(code: string): Promise<string[]> {
  const [result] = await eslint.lintText(code, { filePath: "src/Probe.tsx" });
  return result.messages.map((m) => m.ruleId ?? "");
}

describe("lint guards", () => {
  test("rejects dangerouslySetInnerHTML", async () => {
    const ids = await ruleIds(
      'export const P = ({ t }: { t: string }) => <div dangerouslySetInnerHTML={{ __html: t }} />;\n',
    );
    expect(ids).toContain("react/no-danger");
  });

  test.each([
    'export const P = () => <div style={{ color: "#fff" }} />;\n',
    'export const P = () => <svg><path fill="#B42318" /></svg>;\n',
    "export const c = `1px solid #e2e0d9`;\n",
  ])("rejects raw hex colors: %s", async (code) => {
    expect(await ruleIds(code)).toContain("no-restricted-syntax");
  });

  test("allows color variables", async () => {
    const ids = await ruleIds(
      'export const P = () => <div style={{ color: "var(--color-text)" }} />;\n',
    );
    expect(ids).toEqual([]);
  });
});
