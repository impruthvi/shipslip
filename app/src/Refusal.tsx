import { CommandBox } from "./CommandBox";
import { withCode } from "./text";
import type { CommandError } from "./types";

interface Wording {
  title: string;
  /** Said in place of the raw error, which then goes under Details. */
  body?: string;
}

/** The refusal screen's title, by what refused. */
export function refusalWording(error: CommandError): Wording {
  switch (error.connectionKind) {
    case "hostKeyUnknown":
      return {
        title: "Shipslip doesn't know this server yet",
        body: "Connect once in Terminal and compare the fingerprint.",
      };
    case "hostKeyChanged":
      return {
        title: "This server's identity changed",
        body: "Verify with your hosting provider before connecting. Shipslip will not continue.",
      };
    case "keyNotLoaded":
      return { title: "Your ssh key isn't loaded", body: "Add it to the ssh agent, then try again." };
    case "other":
      return { title: "Couldn't connect to the server" };
  }
  switch (error.blockedKind) {
    case "lockHeld":
      return { title: "Another run holds the server lock" };
    case "fetchFailed":
      return { title: "The server couldn't fetch the code" };
    case "upToDate":
      return { title: "Already up to date" };
  }
  switch (error.code) {
    case "connection":
      return { title: "Couldn't release the server lock" };
    case "untrusted":
      return {
        title: "These settings need approval",
        body: "The settings for this env changed since they were approved. Review them before deploying.",
      };
    case "changed":
      return { title: "The config changed" };
    case "busy":
      return { title: "This env already has a deploy session" };
    case "confirmation":
      return { title: "The typed name didn't match" };
    case "stale":
      return { title: "The map changed" };
    case "notFound":
      return { title: "Nothing to continue" };
    default:
      return { title: "Shipslip couldn't continue" };
  }
}

/** Title, where, one next step, the raw error under Details. Remote text stays text. */
export function Refusal({
  error,
  where,
  action,
}: {
  error: CommandError;
  /** "shop · production · shop-prod" */
  where: string;
  /** The one thing to do from here: Check again, Rerun…, Review configuration. */
  action?: { label: string; run: () => void };
}) {
  const { title, body } = refusalWording(error);
  // A changed host key never gets a command to run.
  const step = error.connectionKind === "hostKeyChanged" ? undefined : error.nextStep;
  return (
    <div className="workspace-body">
      <h2 className="refusal-title">{title}</h2>
      <p className="muted">{where}</p>
      <p className="workspace-message">{withCode(body ?? error.message)}</p>
      {step?.kind === "command" && (
        <CommandBox prefix={step.prefix} command={step.command} copyText={step.copyText} />
      )}
      {step?.kind === "text" && <p className="workspace-message">{step.text}</p>}
      {action && (
        <p className="workspace-message">
          <button type="button" className="button" onClick={action.run}>
            {action.label}
          </button>
        </p>
      )}
      {body && (
        <details className="workspace-details">
          <summary className="muted">Details</summary>
          <pre className="mono">{error.message}</pre>
        </details>
      )}
    </div>
  );
}
