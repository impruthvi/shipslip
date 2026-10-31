import { render, screen } from "@testing-library/react";
import { expect, test } from "vitest";
import * as fixtures from "./fixtures.gen";
import { InspectorHead } from "./Inspector";
import type { EnvCellView } from "./types";

function cell(label: string, env: string): EnvCellView {
  if (fixtures.map.kind !== "loaded") throw new Error("fixture not loaded");
  const map = fixtures.map.map;
  const found = map.rows.find((row) => row.repoRoot === `/Users/you/code/${label}`)!.cells[map.envs.indexOf(env)];
  if (found.kind !== "present") throw new Error("absent cell");
  return found;
}

test("the inspector retains both the moved target and the newer maintenance risk", () => {
  const api = cell("api", "production");
  render(<InspectorHead cell={api} label="api" env="production" production />);
  expect(screen.getByText("api → production")).toBeInTheDocument();
  expect(screen.getByText("PRODUCTION")).toBeInTheDocument();
  expect(screen.getByText(api.target!)).toBeInTheDocument();
  expect(screen.getByText(api.targetMoved!)).toBeInTheDocument();
  expect(screen.getByText(/Last attempt stopped in maintenance mode/)).toBeInTheDocument();
  expect(screen.getByText("app may be in maintenance mode").closest("li")).toHaveClass("tone-attention");
});

test("an unreadable newer receipt remains visible beside the older recorded code", () => {
  const billing = cell("billing", "production");
  render(<InspectorHead cell={billing} label="billing" env="production" production />);
  for (const qualifier of billing.qualifiers) {
    expect(screen.getByText(qualifier).closest("li")).toHaveClass("tone-attention");
  }
});

test("new error details moved out of cells keep their NEW label in the inspector", () => {
  const shop = cell("shop", "staging");
  render(<InspectorHead cell={shop} label="shop" env="staging" production={false} />);
  expect(screen.getByText("NEW")).toBeInTheDocument();
  expect(screen.getByText("2 new error groups after this run").closest("li")).toHaveClass("tone-bad");
  expect(screen.queryByText("PRODUCTION")).not.toBeInTheDocument();
});

test("warning and target text stays text even when it looks like markup", () => {
  const markup = '<img src=x onerror="throw 1">';
  const api = { ...cell("api", "production"), target: markup, targetMoved: markup };
  render(<InspectorHead cell={api} label="api" env="production" production />);
  expect(screen.getAllByText(markup)).toHaveLength(2);
  expect(document.querySelector("img")).toBeNull();
});
