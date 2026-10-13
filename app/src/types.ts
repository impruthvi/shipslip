// The JSON the Rust commands return. Kept in step with
// app/src-tauri/src/{view,receipt_view,state}.rs by src/fixtures.gen.ts,
// which Rust writes from real views and tsc checks against these types.

export type Screen = "map" | "firstRun" | "noneReadable";

export interface MapView {
  screen: Screen;
  envs: string[];
  rows: RowView[];
  problems: ProblemView[];
}

export interface RowView {
  label: string;
  repoRoot: string;
  pathMissing: boolean;
  cells: CellView[];
}

export type CellView = { kind: "absent" } | ({ kind: "present" } & EnvCellView);

export type Mark = "recorded" | "incomplete" | "observed" | "notKnown" | "noCode";

export type Tone = "good" | "bad" | "neutral";

export interface OutcomeView {
  text: string;
  tone: Tone;
}

export interface FlagView {
  text: string;
  newErrors: boolean;
  /** New errors, a failed smoke check, or the app left in maintenance mode. */
  bad: boolean;
}

export interface EnvCellView {
  mark: Mark;
  sha: string | null;
  shortSha: string | null;
  atMs: number | null;
  age: string | null;
  qualifiers: string[];
  words: string;
  evidence: string[];
  outcome: OutcomeView | null;
  flags: FlagView[];
  target: string | null;
  targetMoved: string | null;
  needsAttention: boolean;
  receipt: number | null;
  staleReceipt: number | null;
  lastAttempt: AttemptView | null;
}

export interface AttemptView {
  receipt: number;
  outcome: OutcomeView;
  flags: FlagView[];
  atMs: number;
  age: string;
}

export type ProblemKind = "trust" | "projectDir" | "receipt";

export interface CellPos {
  row: number;
  col: number;
}

export interface ProblemView {
  kind: ProblemKind;
  message: string;
  path: string;
  cells: CellPos[];
}

export interface Loaded {
  generation: number;
  map: MapView;
}

export type LoadResult =
  | ({ kind: "loaded" } & Loaded)
  | { kind: "coalesced" }
  | { kind: "failed"; message: string; current: Loaded | null };

export interface CompareView {
  text: string;
  available: boolean;
}

export type CompareResult =
  | {
      kind: "compared";
      generation: number;
      row: number;
      a: number;
      b: number;
      comparison: CompareView;
    }
  | { kind: "stale" }
  | { kind: "notFound" };

export type Span =
  | { kind: "text"; text: string }
  | { kind: "command"; prefix: string; command: string; copyText: string };

export interface Guidance {
  lines: Span[][];
  copyWithheld: boolean;
}

export type StepStatus = "ok" | "failed" | "unknown" | "notRun" | "running" | "skipped";

export interface StepView {
  index: string;
  command: string;
  status: StepStatus;
  label: string;
  exitCode: number | null;
  output: string[];
}

export interface VariantView {
  count: number;
  location: string | null;
  message: string;
}

export interface GroupView {
  count: number;
  exception: string;
  file: string | null;
  variants: VariantView[];
  moreVariants: number;
}

export interface WatchView {
  status: string;
  duration: string;
  linesRead: number;
  logPath: string;
  baseline: [number, number] | null;
  groups: GroupView[];
  moreGroups: number;
  warnings: string[];
}

export interface ServerHead {
  sha: string | null;
  dirty: boolean;
}

export interface ReceiptView {
  runId: string;
  /** The short headline, e.g. "Failed at step 2". */
  outcome: OutcomeView;
  /** The receipt's own sentence for a finished run. */
  summary: string | null;
  flags: FlagView[];
  project: string;
  repoRoot: string;
  env: string;
  production: boolean;
  target: string;
  branch: string;
  plan: string;
  startedAtMs: number;
  finishedAtMs: number | null;
  duration: string | null;
  startedBy: string | null;
  fromSha: string;
  targetSha: string;
  serverHead: ServerHead | null;
  commits: string[];
  steps: StepView[];
  watch: WatchView | null;
  smoke: string | null;
  notes: string[];
  nextSteps: Guidance;
}

export type SlipView =
  | ({ kind: "receipt" } & ReceiptView)
  | { kind: "missing" }
  | { kind: "newerVersion" }
  | { kind: "unreadable"; message: string };

export type ReceiptResult =
  | { kind: "opened"; generation: number; id: number; slip: SlipView }
  | { kind: "stale" }
  | { kind: "notFound" };
