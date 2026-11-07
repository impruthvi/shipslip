import { useId, useState, type FormEvent } from "react";

/** The primary confirm. With `expected`, the name must be typed: exact and
 * case-sensitive, spaces around it ignored. Enter submits only on a match. */
export function TypedConfirm({
  expected,
  verb,
  action,
  busy,
  blocked = false,
  onConfirm,
}: {
  /** The env name to type, or null when nothing needs typing. */
  expected: string | null;
  /** What the button does, as "Type production to <verb>". */
  verb: string;
  /** The button's label. */
  action: string;
  busy: boolean;
  /** Disabled for a reason shown elsewhere. */
  blocked?: boolean;
  onConfirm: (typedEnv: string | null) => void;
}) {
  const [typed, setTyped] = useState("");
  const noteId = useId();
  const matches = expected === null || typed.trim() === expected;
  const ready = matches && !busy && !blocked;
  const prompt = `Type ${expected} to ${verb}`;
  // A block's reason is shown by the caller, so no typing hint competes with it.
  const note = matches || blocked ? null : typed.trim() === "" ? prompt : `Doesn't match ${expected}`;

  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (ready) onConfirm(expected);
  };

  return (
    <form className="footer-end" onSubmit={submit}>
      {expected !== null && (
        <input
          className="typed-input"
          aria-label={prompt}
          aria-describedby={note ? noteId : undefined}
          value={typed}
          onChange={(event) => setTyped(event.target.value)}
          autoComplete="off"
          autoCapitalize="off"
          spellCheck={false}
          disabled={busy}
        />
      )}
      <div className="typed-action">
        <button
          type="submit"
          className="button button-primary"
          disabled={!ready}
          aria-describedby={note ? noteId : undefined}
        >
          {action}
        </button>
        {note && (
          <span id={noteId} className="typed-note muted">
            {note}
          </span>
        )}
      </div>
    </form>
  );
}
