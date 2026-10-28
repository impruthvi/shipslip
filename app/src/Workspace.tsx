import { useEffect, useRef, useState } from "react";
import type { Announce } from "./Announcer";
import { deploy } from "./deployApi";
import { CommandBox } from "./CommandBox";
import { Mark } from "./Mark";
import { Preview, type PreviewBusy } from "./DeployPreview";
import { endSentence } from "./run";
import { Running } from "./Running";
import { elapsed } from "./time";
import type { CommandError, DeployReview, PrepareProgress, RunInfo } from "./types";

/** The cell a deploy was asked for. */
export interface DeployTarget {
  generation: number;
  row: number;
  env: string;
  /** The app's name on the map. */
  label: string;
  production: boolean;
}

/** `finished`: a run ended, so its receipt should open. */
export type Leave = "back" | "finished";

type Screen =
  | { kind: "preparing"; phases: { progress: PrepareProgress; at: number }[]; cancelling: boolean }
  | { kind: "refused"; error: CommandError }
  | { kind: "preview"; review: DeployReview; lockedAt: Date; busy: PreviewBusy }
  | { kind: "running"; review: DeployReview; info: RunInfo };

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

/** Preparing → Preview → Running, in place of the map. */
export function Workspace({
  target,
  onLeave,
  announce,
}: {
  target: DeployTarget;
  onLeave: (how: Leave) => void;
  announce: Announce;
}) {
  const [screen, setScreen] = useState<Screen>({ kind: "preparing", phases: [], cancelling: false });
  const [now, setNow] = useState(() => Date.now());
  const heading = useRef<HTMLHeadingElement>(null);
  const alive = useRef(true);
  const started = useRef(false);
  const cancelled = useRef(false);
  const props = useRef({ target, onLeave, announce });
  useEffect(() => {
    props.current = { target, onLeave, announce };
  });

  const refuse = (error: unknown) => {
    if (!alive.current) return;
    const refused = refusal(error);
    setScreen({ kind: "refused", error: refused });
    props.current.announce(refused.message, true);
  };
  const release = (handle: string) =>
    deploy.cancel(handle).then(() => alive.current && props.current.onLeave("back"), refuse);

  useEffect(() => {
    alive.current = true;
    if (!started.current) {
      started.current = true;
      const { generation, row, env } = props.current.target;
      deploy
        .prepare(generation, row, env, "Deploy", (progress) => {
          if (!alive.current) return;
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
            if (!alive.current) void deploy.cancel(review.handle).catch(() => {});
            else if (cancelled.current) void release(review.handle);
            else setScreen({ kind: "preview", review, lockedAt: new Date(), busy: null });
          },
          (error: unknown) => {
            if (cancelled.current) {
              if (alive.current) props.current.onLeave("back");
            } else refuse(error);
          },
        );
    }
    return () => {
      alive.current = false;
    };
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
    if (screen.kind === "preview") {
      setScreen({ ...screen, busy: "releasing" });
      void release(screen.review.handle);
    } else if (screen.kind === "refused") {
      onLeave("back");
    } else if (screen.kind === "preparing") {
      cancel();
    }
  };
  const confirm = (typedEnv: string | null) => {
    if (screen.kind !== "preview") return;
    const { review } = screen;
    setScreen({ ...screen, busy: "confirming" });
    deploy.confirm(review.handle, typedEnv).then(
      (info) => alive.current && setScreen({ kind: "running", review, info }),
      refuse,
    );
  };

  // Escape is Back or Cancel; a running deploy ignores it. Dialogs own it while open.
  const backRef = useRef(back);
  useEffect(() => {
    backRef.current = back;
  });
  const idle =
    screen.kind === "refused" ||
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

  const review = screen.kind === "preview" || screen.kind === "running" ? screen.review : null;
  const production = review?.production ?? target.production;

  return (
    <div className="workspace">
      <header className="workspace-header">
        <div className="workspace-heading">
          <h1 ref={heading} tabIndex={-1}>
            Deploy {target.label} → {target.env}
          </h1>
          {production && <span className="production-chip">PRODUCTION</span>}
          {screen.kind !== "running" && (
            <button type="button" className="link" disabled={!idle} onClick={back}>
              Release map
            </button>
          )}
        </div>
        {review && (
          <p className="workspace-target mono muted">
            {review.sshAlias} · {review.path}
          </p>
        )}
      </header>
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
                        <span className="tone-good" aria-label="done">
                          ✓
                        </span>
                      ) : (
                        <Mark kind="live" />
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
          <div className="workspace-body">
            <p className="workspace-message">{screen.error.message}</p>
            {screen.error.nextStep?.kind === "command" && (
              <CommandBox
                prefix={screen.error.nextStep.prefix}
                command={screen.error.nextStep.command}
                copyText={screen.error.nextStep.copyText}
              />
            )}
            {screen.error.nextStep?.kind === "text" && (
              <p className="workspace-message">{screen.error.nextStep.text}</p>
            )}
          </div>
          <footer className="workspace-footer">
            <button type="button" className="button" onClick={() => onLeave("back")}>
              Back
            </button>
          </footer>
        </>
      )}
      {screen.kind === "preview" && (
        <Preview
          review={screen.review}
          lockedAt={screen.lockedAt}
          busy={screen.busy}
          onBack={back}
          onDeploy={confirm}
        />
      )}
      {screen.kind === "running" && (
        <Running
          review={screen.review}
          info={screen.info}
          announce={announce}
          onEnd={(terminal) => {
            announce(endSentence(terminal), true);
            onLeave("finished");
          }}
        />
      )}
    </div>
  );
}
