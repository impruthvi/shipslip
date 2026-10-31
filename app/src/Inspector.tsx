import { lowerFirst } from "./text";
import type { EnvCellView, FlagView } from "./types";
import { WarningIcon } from "./Warning";

function Flag({ flag }: { flag: FlagView }) {
  if (!flag.bad) return <li className="inspector-note muted sentence">{flag.text}</li>;
  const failed = flag.kind === "newErrors" || flag.kind === "smokeFailed";
  return (
    <li className={`inspector-warning ${failed ? "tone-bad" : "tone-attention"}`}>
      {flag.newErrors ? <span className="chip">NEW</span> : <WarningIcon />}
      <span className="sentence">{flag.text}</span>
    </li>
  );
}

function Warning({ text }: { text: string }) {
  return (
    <li className="inspector-warning tone-attention">
      <WarningIcon />
      <span className="sentence">{text}</span>
    </li>
  );
}

/** The top of the panel: which app and env, where it deploys, and every
 * warning the map line had no room for. */
export function InspectorHead({
  cell,
  label,
  env,
  production,
}: {
  cell: EnvCellView;
  label: string;
  env: string;
  production: boolean;
}) {
  const last = cell.lastAttempt;
  const lastBad = last !== null && last.outcome.tone === "bad";
  return (
    <header className="inspector-head">
      <p className="inspector-title">
        <span>
          {label} → {env}
        </span>
        {production && <span className="production-chip">PRODUCTION</span>}
      </p>
      {cell.target && <p className="inspector-target mono muted">{cell.target}</p>}
      <ul className="inspector-warnings">
        {lastBad && (
          <li className="inspector-warning tone-attention">
            <WarningIcon />
            <span>
              Last attempt {lowerFirst(last.outcome.text)} · {last.age}
            </span>
          </li>
        )}
        {lastBad && last.flags.filter((flag) => flag.bad).map((flag) => <Flag key={`last ${flag.text}`} flag={flag} />)}
        {cell.qualifiers.map((text) => (
          <Warning key={text} text={text} />
        ))}
        {cell.targetMoved && <Warning text={cell.targetMoved} />}
        {cell.flags.map((flag) => (
          <Flag key={flag.text} flag={flag} />
        ))}
      </ul>
    </header>
  );
}
