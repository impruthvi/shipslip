import "@testing-library/jest-dom/vitest";
import { cleanup } from "@testing-library/react";
import { afterEach, vi } from "vitest";

afterEach(cleanup);

// Map screens never deploy: "Review deploy…" stays hidden unless a test
// mocks `./deployApi` itself.
vi.mock("./deployApi", () => ({
  deploy: {
    entry: async () => ({ kind: "hidden" }),
    prepare: () => new Promise(() => {}),
    confirm: () => new Promise(() => {}),
    cancel: async () => {},
    follow: async () => () => {},
    stop: async () => {},
  },
}));
