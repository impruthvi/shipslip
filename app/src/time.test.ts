import { expect, test } from "vitest";
import { countdown, elapsed, spoken, stamp, utcStamp } from "./time";

const AT = Date.UTC(2026, 9, 6, 14, 21);

test("local time with its zone, UTC on hover", () => {
  expect(stamp(AT, "Asia/Kolkata", "en-IN")).toBe("06 Oct 2026 · 19:51 IST");
  expect(stamp(AT, "UTC", "en-US")).toBe("06 Oct 2026 · 14:21 UTC");
  expect(stamp(Date.UTC(2026, 0, 5, 9, 3), "Europe/London", "en-GB")).toBe("05 Jan 2026 · 09:03 GMT");
  expect(utcStamp(AT)).toBe("2026-10-06 14:21 UTC");
});

test("running times, the watch countdown and their spoken form", () => {
  expect([elapsed(-5), elapsed(8_400), elapsed(134_000), elapsed(3_780_000)]).toEqual([
    "0s",
    "8s",
    "2m 14s",
    "1h 3m",
  ]);
  expect([countdown(120_000), countdown(101_200), countdown(0), countdown(-1)]).toEqual([
    "2:00",
    "1:42",
    "0:00",
    "0:00",
  ]);
  expect([spoken(1_000), spoken(12_000), spoken(120_000), spoken(134_000)]).toEqual([
    "1 second",
    "12 seconds",
    "2 minutes",
    "2 minutes 14 seconds",
  ]);
});
