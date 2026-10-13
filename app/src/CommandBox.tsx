import { useEffect, useState } from "react";
import { api } from "./api";

/** How long "Copied" stays before the button reads "Copy" again. */
export const COPIED_MS = 1500;

interface Props {
  /** `cd '<repo>' && `, shown in pencil. */
  prefix: string;
  command: string;
  /** Exactly what lands on the clipboard: prefix + command. */
  copyText: string;
}

export function CommandBox({ prefix, command, copyText }: Props) {
  const [copy, setCopy] = useState<"idle" | "copied" | "failed">("idle");

  useEffect(() => {
    if (copy !== "copied") return;
    const timer = setTimeout(() => setCopy("idle"), COPIED_MS);
    return () => clearTimeout(timer);
  }, [copy]);

  const onCopy = () => {
    api.copyText(copyText).then(
      () => setCopy("copied"),
      () => setCopy("failed"),
    );
  };

  return (
    <div className="command-box">
      <div className="command-text mono">
        <span className="muted">{prefix.trimEnd()}</span> <span>{command}</span>
      </div>
      <button
        type="button"
        className={`command-copy${copy === "copied" ? " tone-good" : copy === "failed" ? " tone-bad" : ""}`}
        onClick={onCopy}
        aria-label={copy === "idle" ? `Copy ${command}` : undefined}
      >
        {copy === "copied" ? "Copied" : copy === "failed" ? "Copy failed" : "Copy"}
      </button>
    </div>
  );
}
