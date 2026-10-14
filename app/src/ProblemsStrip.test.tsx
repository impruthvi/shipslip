import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { ProblemsStrip } from "./ProblemsStrip";
import type { ProblemView } from "./types";

const problems: ProblemView[] = [
  { kind: "receipt", message: "invalid receipt `…/b.json`", path: "/r/b.json", cells: [{ row: 1, col: 0 }] },
  { kind: "trust", message: "can't read trust.json", path: "/t/trust.json", cells: [] },
  { kind: "projectDir", message: "can't read ~/code/shop/.shipslip.toml", path: "/c/shop", cells: [] },
];

test("only problems tied to a cell offer Select cell; Show all and Show less", () => {
  const onSelect = vi.fn();
  render(<ProblemsStrip problems={problems} onSelect={onSelect} />);
  expect(screen.getByText("1 receipt, 1 project folder and the approvals file could not be read")).toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "Select cell" })).toHaveLength(1);
  expect(screen.queryByText("can't read trust.json")).not.toBeInTheDocument();

  fireEvent.click(screen.getByRole("button", { name: "Show all" }));
  expect(screen.getByText("can't read trust.json")).toBeInTheDocument();
  expect(screen.getByText("can't read ~/code/shop/.shipslip.toml")).toBeInTheDocument();
  expect(screen.getAllByRole("button", { name: "Select cell" })).toHaveLength(1);

  fireEvent.click(screen.getByRole("button", { name: "Select cell" }));
  expect(onSelect).toHaveBeenCalledWith(1, 0);
  fireEvent.click(screen.getByRole("button", { name: "Show less" }));
  expect(screen.queryByText("can't read trust.json")).not.toBeInTheDocument();
});

test("no problems, no strip", () => {
  const { container } = render(<ProblemsStrip problems={[]} />);
  expect(container).toBeEmptyDOMElement();
});
