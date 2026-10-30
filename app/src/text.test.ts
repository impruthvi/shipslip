import { expect, test } from "vitest";
import { lowerFirst, shortPath } from "./text";

test("shortPath uses ~ for the home folder and elides the middle", () => {
  expect(shortPath("/Users/ana/code/shop")).toBe("~/code/shop");
  expect(shortPath("/Users/ana")).toBe("~");
  expect(shortPath("/srv/app")).toBe("/srv/app");
  const long = shortPath("/Users/ana/projects/clients/acme/laravel-shipslip-test");
  expect(long).toHaveLength(22);
  expect(long.startsWith("~/projects")).toBe(true);
  expect(long.endsWith("slip-test")).toBe(true);
  expect(long).toContain("…");
});

test("lowerFirst lowers only the first letter", () => {
  expect(lowerFirst("Stopped in maintenance mode")).toBe("stopped in maintenance mode");
  expect(lowerFirst("")).toBe("");
});
