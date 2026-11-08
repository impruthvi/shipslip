// PROTOTYPE (throwaway, branch prototype/app-shell). Fake data and wizard state
// for the Add project layout question. Nothing here talks to Rust.

import { useEffect, useRef, useState } from "react";
import { status, type Status } from "../../Cell";
import * as fixtures from "../../fixtures.gen";
import { twinFolder } from "../../text";
import type { EnvCellView, MapView, Mark, OutcomeView } from "../../types";

export const params = new URLSearchParams(window.location.search);
export const scenario = (params.get("check") ?? "pass") as Scenario;
export const emptyData = params.get("data") === "empty";

export const fullMap: MapView = (fixtures.map as Extract<typeof fixtures.map, { kind: "loaded" }>).map;

export type CheckoutState = "ready" | "setupNotFinished" | "needsApproval";

export interface EnvMark {
  env: string;
  mark: Mark | "live";
  status: Status;
  cell: EnvCellView | null;
  production: boolean;
}

export interface Checkout {
  id: string;
  project: string;
  path: string;
  state: CheckoutState;
  /** Folder shown when two checkouts share a project name. */
  folder: string | null;
  envs: EnvMark[];
  row: number | null;
  pathMissing: boolean;
}

export interface ReceiptLine {
  id: string;
  env: string;
  outcome: OutcomeView;
  age: string;
  atMs: number;
  sha: string | null;
}

const running = { checkout: "/Users/you/code/billing", env: "staging", text: "Deploying · 1m 12s" };
export const runningNow = running;

function marksFor(rowIndex: number): EnvMark[] {
  const row = fullMap.rows[rowIndex];
  return row.cells.flatMap((cell, col) => {
    const env = fullMap.envs[col];
    if (cell.kind !== "present") return [];
    const live = row.repoRoot === running.checkout && env === running.env ? running.text : undefined;
    return [{
      env,
      mark: live ? "live" : cell.mark,
      status: status(cell, live),
      cell,
      production: env === "production",
    }];
  });
}

const labels = fullMap.rows.map((r) => r.label);

export const baseCheckouts: Checkout[] = [
  ...fullMap.rows.map((row, i) => ({
    id: row.repoRoot,
    project: row.label,
    path: row.repoRoot,
    state: "ready" as const,
    folder: twinFolder(labels, row.label, row.repoRoot),
    envs: marksFor(i),
    row: i,
    pathMissing: row.pathMissing,
  })),
  {
    id: "/Users/you/code/docs",
    project: "docs",
    path: "/Users/you/code/docs",
    state: "needsApproval",
    folder: null,
    envs: [],
    row: null,
    pathMissing: false,
  },
];

export function receiptsFor(c: Checkout): ReceiptLine[] {
  const out: ReceiptLine[] = [];
  for (const e of c.envs) {
    const cell = e.cell;
    if (!cell) continue;
    if (cell.outcome && cell.age && cell.atMs) {
      out.push({ id: `${e.env}-r`, env: e.env, outcome: cell.outcome, age: cell.age, atMs: cell.atMs, sha: cell.shortSha });
    }
    if (cell.lastAttempt) {
      out.push({
        id: `${e.env}-l`,
        env: e.env,
        outcome: cell.lastAttempt.outcome,
        age: cell.lastAttempt.age,
        atMs: cell.lastAttempt.atMs,
        sha: null,
      });
    }
  }
  return out.sort((a, b) => b.atMs - a.atMs);
}

/* ---------- Wizard ---------- */

export type Scenario = "pass" | "hostkey" | "changed" | "branch";
export type WizardStep = "folder" | "answers" | "ssh" | "check" | "approval";

export const stepTitles: Record<WizardStep, string> = {
  folder: "Folder",
  answers: "Environment",
  ssh: "SSH alias",
  check: "Connection check",
  approval: "Approval",
};

export interface FolderChoice {
  path: string;
  project: string;
  kind: "new" | "existing" | "notGit";
  warnings: string[];
}

export const folderChoices: FolderChoice[] = [
  {
    path: "/Users/you/code/crm",
    project: "crm",
    kind: "new",
    warnings: [
      "Found yarn.lock; automatic frontend deploy steps support only package-lock.json. Add the appropriate install and build steps to .shipslip.toml.",
    ],
  },
  { path: "/Users/you/code/support", project: "support", kind: "existing", warnings: [] },
  { path: "/Users/you/Downloads/notes", project: "notes", kind: "notGit", warnings: [] },
];

export const existingEnvsFor = (project: string) => [
  { env: "production", ssh: `${project}-prod`, path: `/var/www/${project}`, branch: "main", production: true },
  { env: "staging", ssh: `${project}-staging`, path: `/var/www/${project}`, branch: "develop", production: false },
];

/** Host aliases read from ~/.ssh/config. Read only. */
export const sshHosts = [
  { alias: "production1", host: "203.0.113.10", user: "deploy" },
  { alias: "staging1", host: "203.0.113.20", user: "deploy" },
  { alias: "crm-prod", host: "203.0.113.7", user: "forge" },
];

export const identityFiles = ["~/.ssh/id_ed25519", "~/.ssh/id_rsa", "~/.ssh/id_ed25519_work"];

export interface Answers {
  env: string;
  alias: string;
  path: string;
  branch: string;
  production: boolean;
  maintenance: boolean;
  smokeUrl: string;
}

export interface HostDraft {
  alias: string;
  hostName: string;
  user: string;
  identityFile: string;
  port: string;
}

export function hostBlock(h: HostDraft): string {
  const lines = [`Host ${h.alias}`, `  HostName ${h.hostName || "…"}`, `  User ${h.user || "…"}`];
  if (h.identityFile) lines.push(`  IdentityFile ${h.identityFile}`);
  if (h.port && h.port !== "22") lines.push(`  Port ${h.port}`);
  return lines.join("\n");
}

const q = (s: string) => JSON.stringify(s);
const steps = [
  "composer install --no-dev --no-interaction --prefer-dist --optimize-autoloader",
  "php artisan migrate --force",
  "php artisan optimize",
  "php artisan queue:restart",
];

/** Same text `InitPlan::render` writes. */
export function renderToml(project: string, a: Answers): string {
  let t = `# Shipslip deploy config. Review every value, then run \`slip trust ${a.env}\`.\n\n`;
  t += `[project]\nname = ${q(project)}\nstack = "laravel"\n\n[env.${a.env}]\n`;
  t += `# Host alias from your ~/.ssh/config.\nssh = ${q(a.alias)}\n`;
  t += `# Absolute path of the app's Git checkout on the server.\npath = ${q(a.path)}\n`;
  t += `branch = ${q(a.branch)}\n`;
  t += `# Production deploys ask you to type the environment name to confirm.\nproduction = ${a.production}\n`;
  t += `# Runs \`php artisan down\` before the steps and \`php artisan up\` once they all succeed.\nmaintenance = ${a.maintenance}\n`;
  t += a.smokeUrl
    ? `# Requested from your computer after the steps; 2xx passes.\nsmoke_url = ${q(a.smokeUrl)}\n`
    : `# smoke_url = "https://example.com/health"\n`;
  t += `\n[recipe.deploy]\n# Run in order on the server. Remove any step that is not safe for this app.\nsteps = [\n`;
  for (const s of steps) {
    if (s.includes("migrate")) t += "  # Runs database migrations on every deploy.\n";
    t += `  ${q(s)},\n`;
  }
  return t + "]\n";
}

export type CheckState = "waiting" | "running" | "pass" | "fail" | "notRun";

export interface CheckLine {
  label: string;
  state: CheckState;
  reason: string;
  /** Hard stop: no command offered, wizard cannot go on. */
  hardStop?: boolean;
  /** A command the person runs in Terminal before Check again. */
  command?: string;
}

function planLines(alias: string, path: string, branch: string, attempt: number): CheckLine[] {
  const fails: Partial<Record<number, Omit<CheckLine, "label" | "state">>> = {};
  if (scenario === "hostkey" && attempt === 0) {
    fails[1] = {
      reason: `This Mac has not seen the host key for ${alias}. Run the command in Terminal, compare the fingerprint, answer yes, then check again.`,
      command: `ssh ${alias} true`,
    };
  }
  if (scenario === "changed") {
    fails[1] = {
      reason: `The host key for ${alias} is not the one ~/.ssh/known_hosts records. Find out why the key changed before going on.`,
      hardStop: true,
    };
  }
  if (scenario === "branch") {
    fails[3] = { reason: `The checkout is on develop; this Environment deploys ${branch}.` };
  }
  const pass = [
    `${alias} → 203.0.113.7 as forge (read with ssh -G)`,
    "Connected",
    `${path} has a .git folder`,
    `On ${branch}`,
    "Laravel Framework 11.31.0",
    `${path}/storage/logs/laravel.log`,
  ];
  const labels = [
    `Alias ${alias} resolves`,
    "ssh connects",
    "Remote path is a git checkout",
    `On branch ${branch}`,
    "php artisan --version runs",
    "Laravel log readable",
  ];
  return labels.map((label, i) => ({ label, state: "waiting", reason: pass[i], ...fails[i] }));
}

export function useCheck(alias: string, path: string, branch: string) {
  const [lines, setLines] = useState<CheckLine[] | null>(null);
  const attempt = useRef(0);
  const timers = useRef<number[]>([]);
  useEffect(() => () => timers.current.forEach(clearTimeout), []);
  const run = () => {
    timers.current.forEach(clearTimeout);
    const plan = planLines(alias, path, branch, attempt.current);
    attempt.current += 1;
    const failAt = plan.findIndex((l) => l.command || l.hardStop);
    setLines(plan.map((l, i) => ({ ...l, state: i === 0 ? "running" : "waiting" })));
    plan.forEach((line, i) => {
      if (failAt >= 0 && i > failAt) return;
      const stops = i === failAt;
      const failed = Boolean(line.command || line.hardStop || (scenario === "branch" && i === 3));
      timers.current.push(
        window.setTimeout(() => {
          setLines((prev) =>
            prev?.map((l, j) => {
              if (j === i) return { ...l, state: failed ? "fail" : "pass" };
              if (stops && j > i) return { ...l, state: "notRun", reason: "Not run" };
              if (j === i + 1) return { ...l, state: "running" };
              return l;
            }) ?? null,
          );
        }, 450 * (i + 1)),
      );
    });
  };
  const done = lines !== null && lines.every((l) => l.state !== "waiting" && l.state !== "running");
  const blocked = lines?.some((l) => l.state === "fail" && (l.hardStop || l.command)) ?? false;
  const passed = done && lines!.every((l) => l.state === "pass");
  return { lines, run, done, blocked, passed };
}

/** Where a wizard reopened from the sidebar starts. */
export function resumeFor(id: string | undefined): { folder: FolderChoice; step: WizardStep } | undefined {
  if (id === "/Users/you/code/crm") return { folder: folderChoices[0], step: "answers" };
  if (id === "/Users/you/code/docs") {
    return { folder: { path: id, project: "docs", kind: "existing", warnings: [] }, step: "check" };
  }
  return undefined;
}

export function useWizard(onFinish: (c: Checkout) => void, resume?: string) {
  const init = resumeFor(resume);
  const [step, setStep] = useState<WizardStep>(init?.step ?? "folder");
  const [folder, setFolder] = useState<FolderChoice | null>(init?.folder ?? null);
  const [answers, setAnswers] = useState<Answers>({
    env: "production",
    alias: "crm",
    path: "/var/www/crm",
    branch: "main",
    production: true,
    maintenance: true,
    smokeUrl: "",
  });
  const [aliasMode, setAliasMode] = useState<"pick" | "new">("pick");
  const [host, setHost] = useState<HostDraft>({ alias: "crm", hostName: "", user: "", identityFile: identityFiles[0], port: "22" });
  const [hostFound, setHostFound] = useState<"unchecked" | "missing" | "found">("unchecked");
  const [warningsRead, setWarningsRead] = useState(false);
  const existing = folder?.kind === "existing";
  const existingEnvs = existingEnvsFor(folder?.project ?? "");
  const alias = existing ? existingEnvs[0].ssh : answers.alias;
  const check = useCheck(alias, existing ? existingEnvs[0].path : answers.path, existing ? existingEnvs[0].branch : answers.branch);

  const order: WizardStep[] = existing ? ["folder", "check", "approval"] : ["folder", "answers", "ssh", "check", "approval"];
  const index = order.indexOf(step);
  const next = () => setStep(order[Math.min(index + 1, order.length - 1)]);
  const back = () => setStep(order[Math.max(index - 1, 0)]);
  const checkHostAgain = () => {
    // Prototype: the first Check again misses, the second finds the block.
    setHostFound((h) => (h === "unchecked" ? "missing" : "found"));
  };
  const finish = () => {
    const f = folder!;
    onFinish({
      id: f.path,
      project: f.project,
      path: f.path,
      state: "ready",
      folder: null,
      envs: (existing ? existingEnvs : [{ env: answers.env, production: answers.production }]).map((e) => ({
        env: e.env,
        mark: "noCode",
        status: { word: "No code", tone: "muted" },
        cell: null,
        production: e.production,
      })),
      row: null,
      pathMissing: false,
    });
  };
  const canNext: Record<WizardStep, boolean> = {
    folder: folder !== null && folder.kind !== "notGit",
    answers: answers.env.length > 0 && answers.alias.length > 0 && answers.path.startsWith("/"),
    ssh: aliasMode === "pick" ? sshHosts.some((h) => h.alias === answers.alias) || hostFound === "found" : hostFound === "found",
    check: check.done && !check.blocked,
    approval: folder?.warnings.length ? warningsRead : true,
  };
  return {
    step, setStep, order, index, next, back, finish, canNext,
    folder, setFolder, existing,
    answers, setAnswers: (patch: Partial<Answers>) => setAnswers((a) => ({ ...a, ...patch })),
    aliasMode, setAliasMode,
    host, setHost: (patch: Partial<HostDraft>) => setHost((h) => ({ ...h, ...patch })),
    hostFound, checkHostAgain,
    warningsRead, setWarningsRead,
    check, alias,
    toml: folder ? renderToml(folder.project, answers) : "",
  };
}

export type Wizard = ReturnType<typeof useWizard>;

/** The shell's own state: which checkouts exist and what the main pane shows. */
export type View = { kind: "all" } | { kind: "checkout"; id: string } | { kind: "wizard"; resume?: string };

export function useShell(start: (list: Checkout[]) => View) {
  const [list, setList] = useState<Checkout[]>(() =>
    emptyData
      ? []
      : [
          ...baseCheckouts,
          { id: "/Users/you/code/crm", project: "crm", path: "/Users/you/code/crm", state: "setupNotFinished" as const, folder: null, envs: [], row: null, pathMissing: false },
        ],
  );
  const [view, setView] = useState<View>(() => start(list));
  const add = (c: Checkout) => {
    setList((l) => [...l.filter((x) => x.id !== c.id), c]);
    setView({ kind: "checkout", id: c.id });
  };
  const remove = (id: string) => {
    setList((l) => l.filter((x) => x.id !== id));
    setView({ kind: "all" });
  };
  return { list, view, setView, add, remove };
}

export const nothingRecorded = (c: Checkout) => c.envs.every((e) => !e.cell);
