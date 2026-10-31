import { expect, test } from "vitest";
import { lowerFirst, parentDir, shortPath, tailPath } from "./text";

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

test("tailPath keeps the end of a path, where checkouts differ", () => {
  expect(tailPath("/Users/me/Herd")).toBe("~/Herd");
  expect(tailPath("/private/tmp/slip-test.2XvZ", 18)).toBe("…/slip-test.2XvZ");
  expect(parentDir("/private/tmp/try-shipslip/laravel-shipslip-test")).toBe("/private/tmp/try-shipslip");
});
