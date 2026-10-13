import { act, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, expect, test, vi } from "vitest";
import { COPIED_MS, CommandBox } from "./CommandBox";

const copyText = vi.fn<(text: string) => Promise<void>>();
vi.mock("./api", () => ({ api: { copyText: (text: string) => copyText(text) } }));

beforeEach(() => {
  copyText.mockReset();
  vi.useFakeTimers();
});
afterEach(() => vi.useRealTimers());

const props = {
  prefix: "cd '/Users/you/code/shop' && ",
  command: "slip logs staging",
  copyText: "cd '/Users/you/code/shop' && slip logs staging",
};

test("shows exactly what is copied; Copy turns to Copied for 1.5 s", async () => {
  copyText.mockResolvedValue();
  const { container } = render(<CommandBox {...props} />);
  expect(container).toHaveTextContent("cd '/Users/you/code/shop' && slip logs staging");
  await act(async () => fireEvent.click(screen.getByRole("button", { name: "Copy slip logs staging" })));
  expect(copyText).toHaveBeenCalledWith(props.copyText);
  expect(screen.getByRole("button")).toHaveTextContent("Copied");
  act(() => vi.advanceTimersByTime(COPIED_MS));
  expect(screen.getByRole("button")).toHaveTextContent(/^Copy$/);
});

test("a refused clipboard write says Copy failed", async () => {
  copyText.mockRejectedValue(new Error("denied"));
  render(<CommandBox {...props} />);
  await act(async () => fireEvent.click(screen.getByRole("button")));
  expect(screen.getByRole("button")).toHaveTextContent("Copy failed");
});
