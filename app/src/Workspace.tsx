import { useEffect, useRef, useState } from "react";
import type { Announce } from "./Announcer";
import { deploy } from "./deployApi";
import { StepIcon } from "./Mark";
import { Preview, type PreviewBusy } from "./DeployPreview";
import { RecoveryConfirm, type RecoveryBusy } from "./RecoveryConfirm";
import { Refusal, refusalWording } from "./Refusal";
import { endSentence } from "./run";
import { Running } from "./Running";
import { StepPicker } from "./StepPicker";
import { TrustReview, type TrustBusy } from "./TrustReview";
import { elapsed } from "./time";
import type {
  CommandError,
  DeployReview,
  PrepareProgress,
  RecoveryReview,
  RunInfo,
  RunPlan,
  TrustReview as TrustReviewData,
} from "./types";

/** The cell a deploy was asked for. */
export interface DeployTarget {
  generation: number;
  row: number;
  env: string;
  /** The app's name on the map. */
  label: string;
  production: boolean;
}

/** What the workspace opens on. */
export type Start =
  | { kind: "deploy"; plan: RunPlan }
  /** Run from step: pick where, then prepare. */
  | { kind: "pick"; steps: string[]; step: number }
  | { kind: "trust" }
  | { kind: "up" }
  | { kind: "attach" }
  /** Back to a run already going. */
  | { kind: "follow"; info: RunInfo };

/** `finished`: a run ended, so its receipt should open. `approved`: settings
 * were approved. `away`: the map was shown while a run keeps going. */
export type Leave = "back" | "finished" | "approved" | "away";

export const LOCK_TAKEN =
  "Another run took the server lock while this preview was open. Nothing ran on the server.";

type Screen =
  | { kind: "preparing"; plan: RunPlan; phases: { progress: PrepareProgress; at: number }[]; cancelling: boolean }
  | { kind: "refused"; error: CommandError; retry: RunPlan | null }
  | { kind: "preview"; review: DeployReview; lockedAt: Date; busy: PreviewBusy }
  | { kind: "running"; info: RunInfo }
  | { kind: "loading" }
  | { kind: "trust"; review: TrustReviewData; busy: TrustBusy; changedAgain: boolean }
  | { kind: "pick"; steps: string[]; step: number }
  | { kind: "recovery"; review: RecoveryReview; busy: RecoveryBusy }
  | { kind: "taken"; plan: RunPlan };

export function phaseText(progress: PrepareProgress): string {
  switch (progress.phase) {
    case "connecting":
      return `Connecting to ${progress.alias}…`;
    case "fetching":
      return `Fetching ${progress.branch}…`;
    case "checking":
      return "Checking the server…";
    case "locking":
      return "Taking the deploy lock…";
  }
}

/** A thrown value from a command, as the refusal it carries. */
function refusal(error: unknown): CommandError {
  if (error && typeof error === "object" && "message" in error && "code" in error) {
    return error as CommandError;
  }
  return { code: "local", message: String(error) };
}

function initial(start: Start): Screen {
  switch (start.kind) {
    case "deploy":
      return { kind: "preparing", plan: start.plan, phases: [], cancelling: false };
    case "pick":
      return { kind: "pick", steps: start.steps, step: start.step };
    case "follow":
      return { kind: "running", info: start.info };
    default:
      return { kind: "loading" };
  }
}

/** Trust review, Preparing → Preview → Running, recovery confirms: in place of the map. */
export function Workspace({
  target,
  start,
  onLeave,
  announce,
}: {
  target: DeployTarget;
  start: Start;
  onLeave: (how: Leave) => void;
  announce: Announce;
}) {
  const [screen, setScreen] = useState<Screen>(() => initial(start));
  const [now, setNow] = useState(() => Date.now());
  const [alias, setAlias] = useState<string | null>(null);
  const heading = useRef<HTMLHeadingElement>(null);
  const alive = useRef(true);
  const started = useRef(false);
  const cancelled = useRef(false);
  /** Counts prepares; an older one's late answer is released, not shown. */
  const attempt = useRef(0);
  const props = useRef({ target, start, onLeave, announce });
  useEffect(() => {
    props.current = { target, start, onLeave, announce };
  });

  const refuse = (error: unknown, retry: RunPlan | null = null) => {
    if (!alive.current) return;
    const refused = refusal(error);
    setScreen({ kind: "refused", error: refused, retry });
    props.current.announce(refusalWording(refused).title, true);
  };
  const release = (handle: string) =>
    deploy.cancel(handle).then(() => alive.current && props.current.onLeave("back"), refuse);

  const prepare = (plan: RunPlan) => {
    const mine = ++attempt.current;
    cancelled.current = false;
    setScreen({ kind: "preparing", plan, phases: [], cancelling: false });
    const { generation, row, env } = props.current.target;
    const current = () => alive.current && attempt.current === mine;
    deploy
      .prepare(generation, row, env, plan, (progress) => {
        if (!current()) return;
        if (progress.phase === "connecting") setAlias(progress.alias);
        setScreen((prev) =>
          prev.kind === "preparing"
            ? { ...prev, phases: [...prev.phases, { progress, at: Date.now() }] }
            : prev,
        );
        props.current.announce(phaseText(progress));
      })
      .then(
        (review) => {
          // Left or cancelled while preparing: the lock it took goes back.
          if (!current()) void deploy.cancel(review.handle).catch(() => {});
          else if (cancelled.current) void release(review.handle);
          else {
            setAlias(review.sshAlias);
            setScreen({ kind: "preview", review, lockedAt: new Date(), busy: null });
          }
        },
        (error: unknown) => {
          if (attempt.current !== mine) return;
          if (cancelled.current) {
            if (alive.current) props.current.onLeave("back");
          } else refuse(error, plan);
        },
      );
  };

  const loadTrust = () => {
    setScreen({ kind: "loading" });
    const { generation, row, env } = props.current.target;
    deploy.reviewTrust(generation, row, env).then(
      (review) => alive.current && setScreen({ kind: "trust", review, busy: null, changedAgain: false }),
      (error: unknown) => refuse(error),
    );
  };

  const loadRecovery = (operation: "up" | "attach") => {
    const { generation, row, env } = props.current.target;
    const review = operation === "up" ? deploy.reviewUp : deploy.reviewAttach;
    review(generation, row, env).then(
      (review) => {
        // Left meanwhile: the reservation goes back.
        if (!alive.current) void deploy.cancel(review.handle).catch(() => {});
        else {
          setAlias(review.sshAlias);
          setScreen({ kind: "recovery", review, busy: null });
        }
      },
      (error: unknown) => refuse(error),
    );
  };

  useEffect(() => {
    alive.current = true;
    if (!started.current) {
      started.current = true;
      const first = props.current.start;
      if (first.kind === "deploy") prepare(first.plan);
      else if (first.kind === "trust") loadTrust();
      else if (first.kind === "up" || first.kind === "attach") loadRecovery(first.kind);
      else if (first.kind === "follow") setAlias(first.info.sshAlias);
    }
    return () => {
      alive.current = false;
    };
    // Runs once; later prepares are started by the screens.
  }, []);

  // Each screen starts at its heading.
  useEffect(() => {
    heading.current?.focus();
  }, [screen.kind]);

  useEffect(() => {
    if (screen.kind !== "preparing" || screen.cancelling) return;
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, [screen]);

  const cancel = () => {
    cancelled.current = true;
    setScreen((prev) => (prev.kind === "preparing" ? { ...prev, cancelling: true } : prev));
  };
  const back = () => {
    switch (screen.kind) {
      case "preview":
        setScreen({ ...screen, busy: "releasing" });
        void release(screen.review.handle);
        break;
      case "recovery":
        setScreen({ ...screen, busy: "releasing" });
        void release(screen.review.handle);
        break;
      case "preparing":
        cancel();
        break;
      case "running":
        onLeave("away");
        break;
      default:
        onLeave("back");
    }
  };
  const confirm = (typedEnv: string | null) => {
    if (screen.kind !== "preview") return;
    setScreen({ ...screen, busy: "confirming" });
    deploy.confirm(screen.review.handle, typedEnv).then(
      (info) => alive.current && setScreen({ kind: "running", info }),
      (error: unknown) => refuse(error),
    );
  };
  const confirmRecovery = (typedEnv: string | null) => {
    if (screen.kind !== "recovery") return;
    setScreen({ ...screen, busy: "confirming" });
    deploy.confirm(screen.review.handle, typedEnv).then(
      (info) => alive.current && setScreen({ kind: "running", info }),
      (error: unknown) => refuse(error),
    );
  };
  const editConfig = () => {
    const { generation, row, env } = props.current.target;
    return deploy.openConfig(generation, row, env);
  };
  /** The refusal's way forward, if it has one. */
  const refusalAction = (error: CommandError, retry: RunPlan | null) => {
    if (error.blockedKind === "lockHeld" && retry !== null) {
      return { label: "Check again", run: () => prepare(retry) };
    }
    if (error.blockedKind === "upToDate" && retry === "Deploy") {
      return { label: "Rerun…", run: () => prepare("Rerun") };
    }
    // The settings changed after the map was read: review them here.
    if (error.code === "untrusted") return { label: "Review configuration", run: loadTrust };
    return undefined;
  };
  const rerun = () => {
    if (screen.kind !== "preview") return;
    const { handle } = screen.review;
    setScreen({ ...screen, busy: "releasing" });
    deploy.cancel(handle).then(() => alive.current && prepare("Rerun"), refuse);
  };
  const approve = (typedEnv: string) => {
    if (screen.kind !== "trust") return;
    const shown = screen;
    setScreen({ ...shown, busy: "approving" });
    deploy.approveTrust(shown.review.handle, typedEnv).then(
      () => alive.current && props.current.onLeave("approved"),
      (error: unknown) => {
        if (!alive.current) return;
        const refused = refusal(error);
        if (refused.code === "changed") {
          setScreen({ ...shown, busy: null, changedAgain: true });
          props.current.announce("Config changed again; review the new version.", true);
        } else refuse(refused);
      },
    );
  };

  // Escape is Back or Cancel; a running deploy ignores it. Dialogs own it while open.
  const backRef = useRef(back);
  useEffect(() => {
    backRef.current = back;
  });
  const idle =
    screen.kind === "refused" ||
    screen.kind === "pick" ||
    screen.kind === "taken" ||
    (screen.kind === "trust" && screen.busy === null) ||
    (screen.kind === "recovery" && screen.busy === null) ||
    (screen.kind === "preview" && screen.busy === null) ||
    (screen.kind === "preparing" && !screen.cancelling);
  useEffect(() => {
    if (!idle) return;
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== "Escape" || document.querySelector('[role="alertdialog"]')) return;
      event.preventDefault();
      backRef.current();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [idle]);

  const sub =
    screen.kind === "preview"
      ? screen.review
      : screen.kind === "recovery"
        ? screen.review
        : screen.kind === "running"
          ? screen.info
          : null;
  const production = sub?.production ?? (screen.kind === "trust" ? screen.review.production : target.production);
  const where = [target.label, target.env, alias].filter(Boolean).join(" · ");
  const operation =
    start.kind === "up" || start.kind === "attach"
      ? start.kind
      : start.kind === "follow"
        ? start.info.operation
        : "deploy";
  const title =
    start.kind === "trust" || screen.kind === "trust"
      ? `Review configuration · ${target.label} · ${target.env}`
      : operation === "up"
        ? `Bring ${target.label} → ${target.env} out of maintenance`
        : operation === "attach"
          ? `Attach to ${target.label} → ${target.env}`
          : `Deploy ${target.label} → ${target.env}`;

  return (
    <div className="workspace">
      <header className="workspace-header">
        <div className="workspace-heading">
          <h1 ref={heading} tabIndex={-1}>
            {title}
          </h1>
          {production && <span className="production-chip">PRODUCTION</span>}
          <button
            type="button"
            className="link"
            disabled={!idle && screen.kind !== "running"}
            onClick={back}
          >
            Release map
          </button>
        </div>
        {sub && (
          <p className="workspace-target mono muted">
            {sub.sshAlias} · {sub.path}
          </p>
        )}
      </header>
      {screen.kind === "loading" && (
        <div className="workspace-body">
          <p className="workspace-message muted">Reading the config…</p>
        </div>
      )}
      {screen.kind === "preparing" && (
        <>
          <div className="workspace-body">
            <ol className="phases workspace-section">
              {screen.phases.map(({ progress, at }, i) => {
                const next = screen.phases[i + 1];
                return (
                  <li key={i} className="phase">
                    <span className="phase-mark">
                      {next ? (
                        <StepIcon kind="ok" />
                      ) : (
                        <StepIcon kind="running" />
                      )}
                    </span>
                    <span>{phaseText(progress)}</span>
                    <span className="elapsed muted">{elapsed((next?.at ?? now) - at)}</span>
                  </li>
                );
              })}
            </ol>
          </div>
          <footer className="workspace-footer">
            <button type="button" className="button" disabled={screen.cancelling} onClick={cancel}>
              Cancel
            </button>
            {screen.cancelling && <span className="muted">Releasing the server lock…</span>}
          </footer>
        </>
      )}
      {screen.kind === "refused" && (
        <>
          <Refusal error={screen.error} where={where} action={refusalAction(screen.error, screen.retry)} />
          <footer className="workspace-footer">
            <button type="button" className="button" onClick={() => onLeave("back")}>
              Back
            </button>
          </footer>
        </>
      )}
      {screen.kind === "taken" && (
        <>
          <div className="workspace-body">
            <p className="workspace-message muted">{LOCK_TAKEN}</p>
          </div>
          <footer className="workspace-footer">
            <button type="button" className="button" onClick={() => onLeave("back")}>
              Back
            </button>
            <div className="footer-end">
              <button type="button" className="button button-primary" onClick={() => prepare(screen.plan)}>
                Prepare again
              </button>
            </div>
          </footer>
        </>
      )}
      {screen.kind === "trust" && (
        <TrustReview
          review={screen.review}
          busy={screen.busy}
          changedAgain={screen.changedAgain}
          onBack={back}
          onApprove={approve}
          onReviewAgain={loadTrust}
          onEdit={editConfig}
        />
      )}
      {screen.kind === "pick" && (
        <StepPicker
          steps={screen.steps}
          initial={screen.step}
          onBack={back}
          onPick={(step) => prepare({ FromStep: step })}
        />
      )}
      {screen.kind === "preview" && (
        <Preview
          review={screen.review}
          lockedAt={screen.lockedAt}
          busy={screen.busy}
          onBack={back}
          onDeploy={confirm}
          onRerun={rerun}
        />
      )}
      {screen.kind === "recovery" && (
        <RecoveryConfirm
          review={screen.review}
          busy={screen.busy}
          onBack={back}
          onConfirm={confirmRecovery}
        />
      )}
      {screen.kind === "running" && (
        <Running
          info={screen.info}
          announce={announce}
          onEnd={(terminal) => {
            const plan = screen.info.runPlan;
            if (terminal.kind === "finished" && terminal.lockTaken && plan !== null) {
              announce(LOCK_TAKEN, true);
              setScreen({ kind: "taken", plan });
              return;
            }
            announce(endSentence(terminal), true);
            onLeave("finished");
          }}
        />
      )}
    </div>
  );
}
