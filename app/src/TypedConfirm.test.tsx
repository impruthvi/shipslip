import { fireEvent, render, screen } from "@testing-library/react";
import { expect, test, vi } from "vitest";
import { TypedConfirm } from "./TypedConfirm";

function setup(onConfirm = vi.fn()) {
  render(
    <TypedConfirm expected="production" verb="deploy" action="Deploy to production" busy={false} onConfirm={onConfirm} />,
  );
  return {
    onConfirm,
    input: screen.getByLabelText("Type production to deploy"),
    button: screen.getByRole("button", { name: "Deploy to production" }),
  };
}

test("disabled until typed, with the reason under the button", () => {
  const { button } = setup();
  expect(button).toBeDisabled();
  expect(screen.getByText("Type production to deploy")).toBeInTheDocument();
  expect(screen.queryByText(/doesn't match/i)).not.toBeInTheDocument();
});

test("something else typed: a doesn't-match hint, still disabled, and Enter does nothing", () => {
  const { input, button, onConfirm } = setup();
  fireEvent.change(input, { target: { value: "Production" } });
  expect(button).toBeDisabled();
  expect(screen.getByText("Doesn't match production")).toBeInTheDocument();
  expect(input).toHaveAccessibleDescription("Doesn't match production");
  fireEvent.submit(input);
  expect(onConfirm).not.toHaveBeenCalled();
});

test("leading and trailing spaces are ignored; Enter submits the expected name", () => {
  const { input, button, onConfirm } = setup();
  fireEvent.change(input, { target: { value: "  production " } });
  expect(button).toBeEnabled();
  expect(screen.queryByText(/doesn't match|Type production/i)).not.toBeInTheDocument();
  fireEvent.submit(input);
  expect(onConfirm).toHaveBeenCalledWith("production");
});

test("busy or blocked keeps the button disabled even on a match", () => {
  const onConfirm = vi.fn();
  const { rerender } = render(
    <TypedConfirm expected="production" verb="approve" action="Approve configuration" busy blocked={false} onConfirm={onConfirm} />,
  );
  fireEvent.change(screen.getByLabelText("Type production to approve"), { target: { value: "production" } });
  expect(screen.getByRole("button", { name: "Approve configuration" })).toBeDisabled();
  rerender(
    <TypedConfirm expected="production" verb="approve" action="Approve configuration" busy={false} blocked onConfirm={onConfirm} />,
  );
  expect(screen.getByRole("button", { name: "Approve configuration" })).toBeDisabled();
  // The caller says why it is blocked; no typing hint competes.
  expect(screen.queryByText("Type production to approve")).not.toBeInTheDocument();
  fireEvent.submit(screen.getByLabelText("Type production to approve"));
  expect(onConfirm).not.toHaveBeenCalled();
});

test("nothing to type: the button is ready and confirms with no name", () => {
  const onConfirm = vi.fn();
  render(<TypedConfirm expected={null} verb="deploy" action="Deploy to staging" busy={false} onConfirm={onConfirm} />);
  expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  fireEvent.click(screen.getByRole("button", { name: "Deploy to staging" }));
  expect(onConfirm).toHaveBeenCalledWith(null);
});
