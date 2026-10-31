import { fireEvent, render, screen, within } from "@testing-library/react";
import { describe, expect, test, vi } from "vitest";
import * as fixtures from "./fixtures.gen";
import { Slip } from "./Slip";
import type { EnvCellView, SlipView } from "./types";
import type { Opened } from "./useReceipt";

vi.mock("./api", () => ({ api: { copyText: async () => {} } }));

const map = fixtures.map.kind === "loaded" ? fixtures.map.map : (null as never);

function cellAt(repoRoot: string, env: string): EnvCellView {
  const cell = map.rows.find((r) => r.repoRoot === repoRoot)?.cells[map.envs.indexOf(env)];
  if (cell?.kind !== "present") throw new Error(`no cell ${repoRoot} ${env}`);
  return cell;
}

function slipOf(id: number | null): SlipView {
  const result = fixtures.receipts[String(id)];
  if (result?.kind !== "opened") throw new Error(`no receipt ${id}`);
  return result.slip;
}

function receiptOf(slip: SlipView): Extract<SlipView, { kind: "receipt" }> {
  if (slip.kind !== "receipt") throw new Error("not a receipt");
  return slip;
}

function show(opened: Opened, cell = cellAt("/Users/you/code/shop", "staging")) {
  const onView = vi.fn();
  render(
    <Slip
      opened={opened}
      cell={cell}
      label="shop"
      env="staging"
      context={{
        text: "Latest attempt · recorded code comes from an earlier run",
        links: [{ text: "open", label: "Open the receipt behind the recorded code", view: "code" }],
      }}
      onView={onView}
    />,
  );
  return { onView };
}

describe("receipt slips", () => {
  test("failed at a step: failed mark on it, later steps not run, the step output, two command boxes", () => {
    const failed = slipOf(cellAt("/Users/you/code/billing", "staging").receipt);
    show(failed);
    expect(screen.getByRole("heading", { name: "billing · staging" })).toBeInTheDocument();
    expect(screen.getByText("Failed at step 1")).toHaveClass("tone-bad");
    expect(screen.getByText("Deploy failed at step 1.")).toBeInTheDocument();
    const steps = within(screen.getByRole("list")).getAllByRole("listitem");
    expect(steps[1]).toHaveTextContent("1php artisan migrate --force · failed · exit 1");
    expect(within(steps[1]).getByRole("img", { hidden: true, name: "failed" })).toBeInTheDocument();
    expect(steps[1]).toHaveTextContent("SQLSTATE[42000]");
    expect(screen.getAllByRole("button", { name: /^Copy / }).map((b) => b.getAttribute("aria-label"))).toEqual([
      "Copy slip from-step staging 1",
      "Copy slip deploy staging",
    ]);
    const command = screen.getByText("slip from-step staging 1", { selector: "p code" });
    expect(command.textContent).not.toContain("`");
  });

  test("the header: run id, local start time with UTC on hover, from → to", () => {
    const receipt = receiptOf(slipOf(cellAt("/Users/you/code/shop", "staging").receipt));
    show(receipt);
    expect(screen.getByText(`Receipt · ${receipt.runId}`)).toBeInTheDocument();
    const time = screen.getByText(/\d\d \w{3} \d{4} · \d\d:\d\d/);
    expect(time).toHaveAttribute("title", expect.stringMatching(/ UTC$/));
    expect(screen.getByTitle(receipt.fromSha)).toHaveTextContent(receipt.fromSha.slice(0, 7));
    expect(screen.getByTitle(receipt.targetSha)).toHaveTextContent(receipt.targetSha.slice(0, 7));
  });

  test("a start time outside what a date can hold renders as unknown, not a crash", () => {
    const receipt = receiptOf(slipOf(cellAt("/Users/you/code/shop", "staging").receipt));
    show({ ...receipt, startedAtMs: 2 ** 64 });
    expect(screen.getByText("time not known")).toBeInTheDocument();
  });

  test("a succeeded run shows its outcome without the summary sentence", () => {
    const receipt = receiptOf(slipOf(cellAt("/Users/you/code/shop", "staging").receipt));
    show(receipt);
    expect(screen.getByText("Succeeded")).toHaveClass("tone-good");
    expect(screen.queryByText(receipt.summary ?? "")).not.toBeInTheDocument();
  });

  test("log watch: NEW count, the five largest groups, then the rest; long text expands", () => {
    const receipt = receiptOf(slipOf(cellAt("/Users/you/code/shop", "staging").receipt));
    const watch = receipt.watch!;
    const groups = Array.from({ length: 5 }, (_, i) => ({ ...watch.groups[0], count: 10 - i, exception: `E${i}` }));
    show({ ...receipt, watch: { ...watch, groups, moreGroups: 3 } });
    expect(screen.getByText("NEW 8")).toBeInTheDocument();
    expect(screen.getByText("and 3 more groups")).toBeInTheDocument();
    const first = screen.getByRole("button", { name: /^E0: SQLSTATE/ });
    expect(first).toHaveClass("error-clamped");
    expect(first).toHaveAttribute("aria-expanded", "false");
    fireEvent.click(first);
    expect(first).not.toHaveClass("error-clamped");
    expect(first).toHaveAttribute("aria-expanded", "true");
  });

  test("unfinished: says so and offers slip attach", () => {
    show(fixtures.slips.unfinished);
    expect(screen.getByText("Unfinished")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Copy slip attach staging" })).toBeInTheDocument();
  });

  test("an env with shell characters gets no command box, only the copy-by-hand note", () => {
    show(fixtures.slips.copyWithheld);
    expect(screen.queryByRole("button", { name: /^Copy/ })).not.toBeInTheDocument();
    expect(screen.getByText("Env name has shell characters; copy by hand")).toBeInTheDocument();
    expect(screen.getByText(/`slip up qa;eu`/)).toBeInTheDocument();
  });

  test("the context line links back to the receipt behind the recorded code", () => {
    const { onView } = show(slipOf(cellAt("/Users/you/code/api", "production").lastAttempt!.receipt));
    expect(screen.getByText(/^Latest attempt/)).toBeInTheDocument();
    const open = screen.getByRole("button", { name: "Open the receipt behind the recorded code" });
    expect(open).toHaveTextContent("open");
    fireEvent.click(open);
    expect(onView).toHaveBeenCalledWith("code");
  });
});

describe("slips without a readable receipt", () => {
  test("no receipt on this Mac, with what this Mac does know", () => {
    show({ kind: "none" }, cellAt("/Users/you/code/blog", "staging"));
    expect(screen.getByText("No receipt for shop · staging on this Mac. Approved before.")).toBeInTheDocument();
  });

  test("file gone since the map was read", () => {
    show(fixtures.slips.missing);
    expect(
      screen.getByText("The receipt file is no longer there. Refresh ⌘R to re-read the map."),
    ).toBeInTheDocument();
  });

  test("written by a newer Shipslip", () => {
    show(fixtures.slips.newerVersion);
    expect(screen.getByText("Written by a newer Shipslip. Update slip to read it.")).toBeInTheDocument();
  });

  test("unreadable shows the reason as plain text", () => {
    show({ kind: "unreadable", message: "<b>EOF</b> at line 1" });
    expect(screen.getByText("<b>EOF</b> at line 1")).toBeInTheDocument();
  });
});
