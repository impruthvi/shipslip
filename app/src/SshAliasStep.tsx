import { useEffect, useId, useState } from "react";
import { api } from "./api";
import { errorMessage } from "./text";
import type { AliasCheck, HostBlock, HostDraft, HostField, Resolved, SshAliases } from "./types";

/** The ssh alias step's answers, kept by the sheet so going back keeps them. */
export interface SshChoice {
  /** A listed alias, or "None of these fit"; null before either is picked. */
  mode: "pick" | "new" | null;
  picked: string;
  draft: HostDraft;
  /** The last Check again and the fields it checked. */
  check: { draft: HostDraft; result: AliasCheck } | null;
}

export const freshChoice = (project: string): SshChoice => ({
  mode: null,
  picked: "",
  draft: {
    alias: project,
    hostName: "",
    user: "",
    identityFile: "",
    port: "22",
  },
  check: null,
});

const sameDraft = (a: HostDraft, b: HostDraft) =>
  a.alias === b.alias &&
  a.hostName === b.hostName &&
  a.user === b.user &&
  a.identityFile === b.identityFile &&
  a.port === b.port;

/** The alias Continue may use: one picked, or a new one `ssh -G` now
 * resolves to the fields as they read. */
export function chosenAlias(choice: SshChoice): string | null {
  if (choice.mode === "pick") return choice.picked || null;
  if (choice.mode === "new" && choice.check?.result.kind === "matches" && sameDraft(choice.check.draft, choice.draft))
    return choice.draft.alias.trim();
  return null;
}

const target = (r: Resolved) => `${r.user}@${r.hostName}${r.port === 22 ? "" : `:${r.port}`}`;

/** What the fields ask ssh to resolve to; an empty User accepts ssh's. */
const wanted = (draft: HostDraft, resolved: Resolved): Resolved => ({
  hostName: draft.hostName.trim(),
  user: draft.user.trim() || resolved.user,
  port: Number(draft.port.trim() || 22),
});

function CheckResult({ check }: { check: { draft: HostDraft; result: AliasCheck } }) {
  const { draft, result } = check;
  const alias = draft.alias.trim();
  switch (result.kind) {
    case "invalid":
      return null;
    case "matches":
      return (
        <p className="sheet-hint" role="status">
          Found: <span className="mono">ssh -G {alias}</span> resolves to{" "}
          <span className="mono">{target(result.resolved)}</span>.
        </p>
      );
    case "differs":
      return (
        <p className="sheet-hint tone-attention" role="status">
          <span className="mono">ssh -G {alias}</span> resolves to{" "}
          <span className="mono">{target(result.resolved)}</span>, not{" "}
          <span className="mono">{target(wanted(draft, result.resolved))}</span> yet. Paste the block into
          ~/.ssh/config, save the file, then check again.
        </p>
      );
    case "failed":
      return (
        <div className="sheet-hint tone-attention" role="status">
          <p>
            <span className="mono">ssh -G {alias}</span> did not run cleanly:
          </p>
          <pre className="sheet-detail mono">{result.message}</pre>
        </div>
      );
  }
}

/**
 * Pick a `Host` alias from `~/.ssh/config`, or describe the server and paste
 * the `Host` block Shipslip builds. Shipslip never writes ssh files.
 */
export function SshAliasStep({
  aliases,
  choice,
  disabled,
  onChange,
}: {
  /** null while `~/.ssh/config` is read. */
  aliases: SshAliases | null;
  choice: SshChoice;
  disabled: boolean;
  onChange: (choice: SshChoice) => void;
}) {
  const [block, setBlock] = useState<HostBlock | null>(null);
  const [more, setMore] = useState(choice.draft.port.trim() !== "22" && choice.draft.port.trim() !== "");
  const [checking, setChecking] = useState(false);
  const [copied, setCopied] = useState<"idle" | "copied" | "failed">("idle");
  const [openError, setOpenError] = useState<string | null>(null);
  const fieldId = useId();

  const { draft, mode } = choice;
  useEffect(() => {
    if (mode !== "new") return;
    let current = true;
    api
      .hostBlock(draft)
      .then((b) => current && setBlock(b))
      .catch(() => current && setBlock(null));
    return () => {
      current = false;
    };
  }, [mode, draft]);
  useEffect(() => setCopied("idle"), [block]);

  const edit = (change: Partial<HostDraft>) => onChange({ ...choice, draft: { ...draft, ...change } });

  const checkAgain = () => {
    const checked = draft;
    setChecking(true);
    api
      .checkAlias(checked)
      .then((result) => onChange({ ...choice, check: { draft: checked, result } }))
      .catch((error: unknown) =>
        onChange({
          ...choice,
          check: {
            draft: checked,
            result: {
              kind: "failed",
              message: errorMessage(error),
            },
          },
        }),
      )
      .finally(() => setChecking(false));
  };

  const copy = () => {
    if (block?.kind !== "block") return;
    api.copyText(`${block.lines.join("\n")}\n`).then(
      () => setCopied("copied"),
      () => setCopied("failed"),
    );
  };

  const openConfig = () => {
    setOpenError(null);
    api.openSshConfig().catch((error: unknown) => setOpenError(errorMessage(error)));
  };

  // A field shows its problem once it holds something, or once checked.
  const shown = (field: HostField): string | undefined => {
    const fromCheck =
      choice.check && sameDraft(choice.check.draft, draft) && choice.check.result.kind === "invalid"
        ? choice.check.result.problems
        : [];
    const fromBlock = block?.kind === "invalid" ? block.problems : [];
    const problem = [...fromCheck, ...fromBlock].find((p) => p.field === field);
    if (!problem) return undefined;
    return fromCheck.some((p) => p.field === field) || draft[field].trim() !== "" ? problem.message : undefined;
  };

  const input = (field: HostField, text: string, placeholder?: string) => {
    const id = `${fieldId}-${field}`;
    const problem = shown(field);
    return (
      <div className="sheet-field">
        <label htmlFor={id} className="sheet-label">
          {text}
        </label>
        <input
          id={id}
          name={field}
          className="sheet-input mono"
          value={draft[field]}
          placeholder={placeholder}
          onChange={(event) => edit({ [field]: event.target.value })}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          disabled={disabled}
          aria-invalid={problem ? true : undefined}
          aria-describedby={problem ? `${id}-note` : undefined}
        />
        {problem && (
          <span id={`${id}-note`} className="sheet-hint tone-attention">
            {problem}
          </span>
        )}
      </div>
    );
  };

  const keys = aliases?.identityFiles ?? [];
  return (
    <>
      <p className="muted">Read from ~/.ssh/config. Shipslip never writes ssh files.</p>
      {aliases === null ? (
        <p className="muted" role="status">
          Reading ~/.ssh/config…
        </p>
      ) : (
        <>
          {aliases.error && (
            <p className="sheet-refusal" role="alert">
              {aliases.error}
            </p>
          )}
          <div className="sheet-choices" role="radiogroup" aria-label="SSH alias">
            {aliases.aliases.map((line) => (
              <label key={line.alias} className="sheet-choice">
                <input
                  type="radio"
                  name="ssh-alias"
                  checked={mode === "pick" && choice.picked === line.alias}
                  onChange={() => onChange({ ...choice, mode: "pick", picked: line.alias })}
                  disabled={disabled}
                />
                <span className="mono">{line.alias}</span>
                <span className="sheet-choice-end mono muted">
                  {line.hostName === null ? "ssh -G could not resolve it" : `${line.user}@${line.hostName}`}
                </span>
              </label>
            ))}
            <label className="sheet-choice">
              <input
                type="radio"
                name="ssh-alias"
                checked={mode === "new"}
                onChange={() => onChange({ ...choice, mode: "new" })}
                disabled={disabled}
              />
              <span>None of these fit</span>
            </label>
          </div>
        </>
      )}
      {mode === "new" && (
        <div className="sheet-host">
          <div className="sheet-form">
            {input("alias", "Alias")}
            {input("hostName", "HostName", "203.0.113.7")}
            {input("user", "User", "forge")}
            <div className="sheet-field">
              <label htmlFor={`${fieldId}-identityFile`} className="sheet-label">
                IdentityFile
              </label>
              <select
                id={`${fieldId}-identityFile`}
                className="sheet-input mono"
                value={draft.identityFile}
                onChange={(event) => edit({ identityFile: event.target.value })}
                disabled={disabled}
              >
                <option value="">ssh's default keys</option>
                {keys.map((key) => (
                  <option key={key} value={key}>
                    {key}
                  </option>
                ))}
              </select>
            </div>
            <p>
              <button type="button" className="link" aria-expanded={more} onClick={() => setMore(!more)}>
                More
              </button>
            </p>
            {more && input("port", "Port")}
          </div>
          <div className="sheet-host-block">
            {block?.kind === "block" ? (
              <pre className="sheet-config mono" aria-label="Host block">
                {block.lines.join("\n")}
              </pre>
            ) : (
              <p className="muted">The Host block shows here once HostName is filled in and the fields read right.</p>
            )}
            <p className="sheet-actions">
              <button type="button" className="button" onClick={copy} disabled={block?.kind !== "block"}>
                {copied === "copied" ? "Copied" : copied === "failed" ? "Copy failed" : "Copy"}
              </button>
              <button type="button" className="button" onClick={openConfig}>
                Open ~/.ssh/config
              </button>
              <button
                type="button"
                className="button"
                onClick={checkAgain}
                disabled={disabled || checking || block?.kind !== "block"}
              >
                {checking ? "Checking…" : "Check again"}
              </button>
            </p>
            {openError && (
              <p className="sheet-hint tone-attention" role="alert">
                {openError}
              </p>
            )}
            {choice.check && sameDraft(choice.check.draft, draft) && <CheckResult check={choice.check} />}
          </div>
        </div>
      )}
    </>
  );
}
