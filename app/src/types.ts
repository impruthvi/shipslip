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
  /** Its `.shipslip.toml` has an environment without a current approval. */
  needsApproval: boolean;
  cells: CellView[];
}

export type CellView = { kind: "absent" } | ({ kind: "present" } & EnvCellView);

export type Mark = "recorded" | "incomplete" | "observed" | "notKnown" | "noCode";

export type Tone = "good" | "bad" | "neutral";

export type OutcomeKind =
  | "succeeded"
  | "failed"
  | "stopped"
  | "stoppedInMaintenance"
  | "cancelled"
  | "unknown"
  | "unfinished";

export interface OutcomeView {
  text: string;
  tone: Tone;
  /** How the run ended, for the map's status word. */
  kind: OutcomeKind;
}

export interface FlagView {
  text: string;
  newErrors: boolean;
  /** A check or outcome that needs attention, not necessarily a failure. */
  bad: boolean;
  kind: FlagKind;
}

export type FlagKind =
  | "newErrors"
  | "logWatchInterrupted"
  | "logUnreadable"
  | "noLogFileYet"
  | "logWatchStoppedEarly"
  | "logWatchSkippedInMaintenance"
  | "logNotRecorded"
  | "smokeFailed"
  | "smokeNotRecorded"
  | "appLeftDown";

export interface EnvCellView {
  terminalRun: { runId: string; startedAtMs: number } | null;
  mark: Mark;
  sha: string | null;
  shortSha: string | null;
  atMs: number | null;
  age: string | null;
  qualifiers: string[];
  words: string;
  evidence: string[];
  outcome: OutcomeView | null;
  /** The steps a run-from-step ran, e.g. "steps 4–5 only"; null for a full run. */
  stepsRun: string | null;
  flags: FlagView[];
  target: string | null;
  /** An approval record exists; it may be for an older config. */
  approved: boolean;
  targetMoved: string | null;
  needsAttention: boolean;
  receipt: number | null;
  staleReceipt: number | null;
  lastAttempt: AttemptView | null;
}

export interface AttemptView {
  receipt: number;
  outcome: OutcomeView;
  stepsRun: string | null;
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
  /** The line naming `slip attach`, left out where the app offers Attach. */
  attachLine: number | null;
}

export type StepStatus = "ok" | "failed" | "unknown" | "notRun" | "running" | "skipped";

/** What this Mac recorded of a step's run time; `null` when nothing was. */
export type StepTiming =
  | { kind: "took"; duration: string; durationMs: number }
  | { kind: "endNotKnown"; startedAtMs: number };

export interface RecordedConnectionGap {
  lostAtMs: number;
  reattachedAtMs: number | null;
}

export interface StepView {
  index: string;
  /** The command, or step 0's label. */
  command: string;
  /** The exact command when `command` is step 0's label. */
  detail: string | null;
  status: StepStatus;
  label: string;
  exitCode: number | null;
  timing: StepTiming | null;
  resultSeenAtMs: number | null;
  connectionGaps: RecordedConnectionGap[];
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
  kind: FlagKind | null;
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
  stepsRun: string | null;
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

/** One line of a checkout's receipts table (`checkout_receipts`). */
export interface ReceiptRowView {
  /** Opens with `load_receipt`, like a cell's receipt. */
  receipt: number;
  env: string;
  age: string;
  outcome: OutcomeView;
  /** The commit the run deployed or brought up. */
  sha: string;
  shortSha: string;
}

export type CheckoutReceipts =
  | { kind: "listed"; generation: number; row: number; receipts: ReceiptRowView[] }
  /** The project's receipts folder could not be read. */
  | { kind: "unreadable"; message: string }
  | { kind: "stale" }
  | { kind: "notFound" };

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
/** A Deploy that can be offered now. */
export type ReadyEntry = Extract<DeployEntry, { kind: "ready" }>;

export type DeployEntry =
  | { kind: "hidden" }
  | { kind: "unavailable"; reason: string }
  | { kind: "running"; handle: string }
  | {
      kind: "ready";
      production: boolean;
      needsTrust: boolean;
      /** Recipe steps 1..N, for picking where a run starts. */
      steps: PickStep[];
      /** What this checkout's newest run left to do. */
      recovery: RecoveryView | null;
    };

/** A recipe step in the step picker. */
export interface PickStep {
  command: string;
  /** The newest deploy's receipt recorded this command ending OK. */
  ranOkLastRun: boolean;
}

/** Step 0 as every screen names it, with the exact command as detail. */
export interface StepZeroView {
  /** e.g. "git fast-forward to 2222222". */
  label: string;
  command: string;
}

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
  /** What an Up does, in words; `null` for attach. */
  phase: string | null;
  /** Attach only: the command that was running. Watched until it ends, never run again. */
  watched: RecoveryStep | null;
  /** What runs next, in order. After a watched step, only if it succeeds. */
  remainingSteps: RecoveryStep[];
  /** Up only: recipe steps the deciding run never ran. The app comes up without them. */
  notRun: RecoveryStep[];
  /** Up only, with `notRun`: where run-from-step would start instead. */
  fromStep: number | null;
}

/** `index` is the receipt's step number; `null` for `php artisan down` / `php artisan up`. */
export interface RecoveryStep {
  index: number | null;
  command: string;
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
  smokeConfigured: boolean;
  recipeHash: string;
  fromSha: string;
  targetSha: string;
  /** Short SHA and subject, newest first. */
  commits: string[];
  moreCommits: number;
  /** Recipe steps 1..N; step 0, the git fast-forward, is built in. */
  steps: string[];
  runPlan: RunPlan;
  /** null when the plan does not move the code. */
  stepZero: StepZeroView | null;
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
  blockedKind?: "lockHeld" | "staleLock" | "fetchFailed" | "upToDate";
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
  connectionGap: ConnectionGapView | null;
  /** null when the run does not move the code. */
  stepZero: StepZeroView | null;
}

export type ProgressStatus =
  | "Pending"
  | "Running"
  | "Ok"
  | "Failed"
  | "Unknown"
  | "NotStarted"
  | "Skipped";

export type ConnectionTargetView =
  | { kind: "step"; index: number }
  | { kind: "maintenance"; phase: "Down" | "Up" }
  | { kind: "logWatch" };

export interface ConnectionGapView {
  target: ConnectionTargetView;
  lostAtMs: number;
}

export interface Progress {
  name: string | null;
  status: ProgressStatus;
  exitCode: number | null;
  /** From the finished event; `null` while running. */
  timing: StepTiming | null;
  reattachedAtMs: number | null;
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

export type RunHeadline =
  | { kind: "failed"; step: number; exitCode: number | null; command: string }
  | { kind: "stopped"; step: number | null; notRun: number[] }
  | { kind: "succeeded" };

export interface RunState {
  terminal: Terminal | null;
  headline: RunHeadline | null;
  connectionGap: ConnectionGapView | null;
  watchReattachedAtMs: number | null;
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
