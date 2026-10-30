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

/** A live run that quitting now would cut short (`quit_status`). */
export interface QuitRun {
  handle: string;
  project: string;
  env: string;
  checkout: string;
  operation: "deploy" | "up" | "attach";
  stage: string;
  /** What "Detach and quit" does: Shipslip reaches a safe point first. */
  detach: string;
  /** What "Quit now" may leave behind. */
  quitNow: string;
}

/** What "Review deploy…" offers for the selected cell (`deploy_entry`). */
export type DeployEntry =
  | { kind: "hidden" }
  | { kind: "unavailable"; reason: string }
  | { kind: "running"; handle: string }
  | {
      kind: "ready";
      production: boolean;
      needsTrust: boolean;
      /** Recipe steps 1..N, for picking where a run starts. */
      steps: string[];
      /** What this checkout's newest run left to do. */
      recovery: RecoveryView | null;
    };

/** One way to continue after a run, with the CLI command that does the same. */
export type RecoveryAction = (
  | { kind: "fromStep"; step: number }
  | { kind: "attach" }
  | { kind: "up" }
) & { command: string };

export interface RecoveryView {
  runId: string;
  sentence: string;
  /** What to do first when nothing is safe to run yet. */
  advice: string | null;
  /** The recommended action first. */
  actions: RecoveryAction[];
}

/** One setting under review; values are lines of untrusted display text. */
export interface TrustRow {
  name: string;
  /** null for a first approval. */
  previous: string[] | null;
  proposed: string[];
  /** The lines are the numbered recipe steps. */
  steps: boolean;
  changed: boolean;
}

export interface TrustReview {
  handle: string;
  env: string;
  project: string;
  production: boolean;
  settings: TrustRow[];
}

/** Attach or Up, reviewed before it runs (`review_attach`, `review_up`). */
export interface RecoveryReview {
  handle: string;
  env: string;
  production: boolean;
  operation: "up" | "attach";
  sshAlias: string;
  path: string;
  runId: string | null;
  targetSha: string | null;
  phase: string;
  /** `index` is the receipt's step number; `null` for `php artisan up` on its own. */
  remainingSteps: { index: number | null; command: string }[];
}

/** A preview the window's reload closed. Nothing ran on the server. */
export interface ClosedPreview {
  checkout: string;
  env: string;
  released: boolean;
}

/** `live_runs`: polled for the map's cells and toolbar. */
export interface LiveRuns {
  runs: RunInfo[];
  closedPreviews: ClosedPreview[];
}

/** Sent on the `prepare_deploy` channel as each phase starts. */
export type PrepareProgress =
  | { phase: "connecting"; alias: string }
  | { phase: "fetching"; branch: string }
  | { phase: "checking" }
  | { phase: "locking" };

export type RunPlan = "Deploy" | "Rerun" | { FromStep: number };

export interface DeployReview {
  handle: string;
  env: string;
  production: boolean;
  checkout: string;
  sshAlias: string;
  path: string;
  branch: string;
  maintenance: boolean;
  /** How long the log is watched after the last step; null when it isn't. */
  watchMs: number | null;
  recipeHash: string;
  fromSha: string;
  targetSha: string;
  /** Short SHA and subject, newest first. */
  commits: string[];
  moreCommits: number;
  /** Recipe steps 1..N; step 0, the git fast-forward, is built in. */
  steps: string[];
  runPlan: RunPlan;
}

export type ErrorCode =
  | "stale"
  | "notFound"
  | "busy"
  | "cancelled"
  | "invalidHandle"
  | "changed"
  | "untrusted"
  | "confirmation"
  | "connection"
  | "blocked"
  | "local";

/** A refused command. `message` is untrusted text, already escaped. */
export interface CommandError {
  code: ErrorCode;
  message: string;
  connectionKind?: "hostKeyUnknown" | "hostKeyChanged" | "keyNotLoaded" | "other";
  blockedKind?: "lockHeld" | "fetchFailed" | "upToDate";
  nextStep?: Span;
}

export interface RunInfo {
  handle: string;
  project: string;
  checkout: string;
  env: string;
  runId: string | null;
  targetSha: string | null;
  operation: "deploy" | "up" | "attach";
  stage: string;
  production: boolean;
  sshAlias: string;
  path: string;
  maintenance: boolean;
  /** Recipe steps 1..N. */
  steps: string[];
  /** null when bringing the app up. */
  runPlan: RunPlan | null;
  watchMs: number | null;
  startedAtMs: number;
}

export type ProgressStatus =
  | "Pending"
  | "Running"
  | "Ok"
  | "Failed"
  | "Unknown"
  | "NotStarted"
  | "Skipped";

export interface Progress {
  name: string | null;
  status: ProgressStatus;
  exitCode: number | null;
}

export type Terminal =
  | {
      kind: "finished";
      label: string;
      summary: string;
      succeeded: boolean;
      unknown: boolean;
      /** Another run took the server lock before anything ran. */
      lockTaken: boolean;
    }
  | { kind: "detached"; step: number }
  | { kind: "interrupted"; step: number; reason: string }
  | { kind: "error"; reason: string };

export interface SmokeView {
  status: "notConfigured" | "skipped" | "passed" | "failed";
  httpStatus: number | null;
  latencyMs: number | null;
  reason: string | null;
}

export interface RunState {
  terminal: Terminal | null;
  /** Keyed by step index; 0 is the git fast-forward. */
  steps: Record<string, Progress>;
  maintenanceUp: Progress | null;
  maintenanceDown: Progress | null;
  warnings: string[];
  droppedWarnings: number;
  appLeftDown: boolean;
  serverHead: string | null;
  treeDirty: boolean | null;
  watchWindowMs: number | null;
  watch: WatchView | null;
  smoke: SmokeView | null;
}

export type OutputSource =
  | { kind: "step"; index: number }
  | { kind: "maintenance"; phase: "Down" | "Up" }
  | { kind: "log"; phase: "During" | "After"; fileLine: string | null };

/** One line of remote output: untrusted text, already escaped. */
export interface OutputLine {
  sequence: number;
  text: string;
  source: OutputSource;
}

/** One `subscribe_run` delivery. `state` is whole; `output` is new lines only,
 * except on `replay`, which starts over. Ack `through` before the next. */
export interface RunBatch {
  handle: string;
  subscription: string;
  replay: boolean;
  after: number;
  through: number;
  droppedOutput: number;
  output: OutputLine[];
  state: RunState;
}
