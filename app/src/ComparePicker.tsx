import { pairs, type Pair } from "./compare";

interface Props {
  envs: string[];
  pair: Pair | null;
  onChange: (pair: Pair | null) => void;
}

const label = (pair: Pair) => `${pair.a} → ${pair.b}`;

export function ComparePicker({ envs, pair, onChange }: Props) {
  const options = pairs(envs);
  if (options.length === 0) return null;
  return (
    <div className="compare">
      <label className="muted" htmlFor="compare-pair">
        Compare
      </label>
      <div className="compare-box">
        <select
          id="compare-pair"
          value={pair ? label(pair) : ""}
          onChange={(event) => onChange(options.find((p) => label(p) === event.target.value) ?? null)}
        >
          <option value="">Compare…</option>
          {options.map((option) => (
            <option key={label(option)} value={label(option)}>
              {label(option)}
            </option>
          ))}
        </select>
        {pair && (
          <button type="button" className="compare-clear" aria-label="Clear comparison" onClick={() => onChange(null)}>
            <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
              <path d="M3 3 L9 9 M9 3 L3 9" strokeWidth="1.3" strokeLinecap="round" />
            </svg>
          </button>
        )}
      </div>
    </div>
  );
}

/** The compare markers on column heads and in the key. */
export function SourceMarker() {
  return (
    <svg className="marker" width="8" height="8" viewBox="0 0 8 8" role="img" aria-label="compared from">
      <circle cx="4" cy="4" r="3.25" fill="none" strokeWidth="1.5" />
    </svg>
  );
}

export function TargetMarker() {
  return (
    <svg className="marker marker-target" width="8" height="8" viewBox="0 0 8 8" role="img" aria-label="compared to">
      <path d="M4 0 L8 4 L4 8 L0 4 Z" />
    </svg>
  );
}
