import { expect, test } from "vitest";
import { stamp, utcStamp } from "./time";

const AT = Date.UTC(2026, 9, 6, 14, 21);

test("local time with its zone, UTC on hover", () => {
  expect(stamp(AT, "Asia/Kolkata", "en-IN")).toBe("06 Oct 2026 · 19:51 IST");
  expect(stamp(AT, "UTC", "en-US")).toBe("06 Oct 2026 · 14:21 UTC");
  expect(stamp(Date.UTC(2026, 0, 5, 9, 3), "Europe/London", "en-GB")).toBe("05 Jan 2026 · 09:03 GMT");
  expect(utcStamp(AT)).toBe("2026-10-06 14:21 UTC");
});
