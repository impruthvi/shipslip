import { useState } from "react";
import type { ProblemView } from "./types";
import { WarningIcon } from "./Warning";

function summary(problems: ProblemView[]): string {
  const count = (kind: ProblemView["kind"]) => problems.filter((p) => p.kind === kind).length;
  const parts: string[] = [];
  const receipts = count("receipt");
  const dirs = count("projectDir");
  if (receipts) parts.push(receipts === 1 ? "1 receipt" : `${receipts} receipts`);
  if (dirs) parts.push(dirs === 1 ? "1 project folder" : `${dirs} project folders`);
  if (count("trust")) parts.push("the approvals file");
  const what = parts.length > 1 ? `${parts.slice(0, -1).join(", ")} and ${parts.at(-1)}` : parts[0];
  return `${what} could not be read`;
}

interface Props {
  problems: ProblemView[];
  onSelect?: (row: number, col: number) => void;
}

export function ProblemsStrip({ problems, onSelect }: Props) {
  const [open, setOpen] = useState(false);
  if (problems.length === 0) {
    return null;
  }
  const shown = open ? problems : problems.slice(0, 1);
  return (
    <section className="banner" aria-label="Problems">
      <WarningIcon />
      <div className="banner-body">
        <div className="banner-title">{summary(problems)}</div>
        {shown.map((problem, index) => {
          const cell = problem.cells[0];
          return (
            <div className="banner-item" key={index}>
              <span className="mono banner-detail">{problem.message}</span>
              {cell && onSelect && (
                <button type="button" className="link" onClick={() => onSelect(cell.row, cell.col)}>
                  Select cell
                </button>
              )}
            </div>
          );
        })}
      </div>
      {problems.length > 1 && (
        <button type="button" className="link" onClick={() => setOpen(!open)}>
          {open ? "Show less" : "Show all"}
        </button>
      )}
    </section>
  );
}
