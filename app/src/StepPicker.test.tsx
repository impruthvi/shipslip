import { render, screen, within } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { Preview } from "./DeployPreview";
import * as fixtures from "./fixtures.gen";
import { StepPicker } from "./StepPicker";

const picked = fixtures.pickerEntry.kind === "ready" ? fixtures.pickerEntry.steps : (null as never);

function rows() {
  return within(screen.getByRole("group", { name: "Steps" })).getAllByRole("listitem");
}

test("earlier steps read not run this time, without strikethrough", () => {
  render(<StepPicker steps={picked} initial={3} onBack={vi.fn()} onPick={vi.fn()} />);
  const [first, second, third] = rows();
  expect(first).toHaveTextContent("not run this time · ran OK in the last run");
  expect(second).toHaveTextContent("not run this time");
  expect(second).not.toHaveTextContent("ran OK");
  expect(third).not.toHaveTextContent("not run this time");
  for (const row of rows()) {
    expect(row).not.toHaveClass("step-skipped");
    expect(row).not.toHaveTextContent("skipped");
  }
});

test("ran OK in the last run is only said of steps before the chosen one", () => {
  render(<StepPicker steps={picked} initial={1} onBack={vi.fn()} onPick={vi.fn()} />);
  for (const row of rows()) {
    expect(row).not.toHaveTextContent("not run this time");
    expect(row).not.toHaveTextContent("ran OK");
  }
});

test("the preview names step 0 by its label, with the exact command as mono detail", () => {
  render(
    <Preview
      review={fixtures.deployReview}
      lockedAt={new Date()}
      busy={null}
      onBack={vi.fn()}
      onDeploy={vi.fn()}
      onRerun={vi.fn()}
    />,
  );
  const steps = within(screen.getByRole("region", { name: "Steps" }));
  const zero = steps.getByText("git fast-forward to bbbbbbb").closest("li")!;
  expect(within(zero).getByText(`git merge --ff-only ${"b".repeat(40)}`, { selector: "code" })).toHaveClass(
    "mono-detail",
  );
});
