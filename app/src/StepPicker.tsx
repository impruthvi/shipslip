import { useState, type FormEvent } from "react";
import type { PickStep } from "./types";

/** Where a run on the deployed commit starts. Picked before Prepare, so the
 * server lock is taken for exactly the run that is shown. */
export function StepPicker({
  steps,
  initial,
  onBack,
  onPick,
}: {
  /** Recipe steps 1..N. */
  steps: PickStep[];
  initial: number;
  onBack: () => void;
  onPick: (step: number) => void;
}) {
  const [step, setStep] = useState(() => Math.min(Math.max(initial, 1), steps.length));

  const submit = (event: FormEvent) => {
    event.preventDefault();
    onPick(step);
  };

  return (
    <form className="workspace-form" onSubmit={submit}>
      <div className="workspace-body">
        <fieldset className="workspace-section step-picker">
          <legend className="caps">Steps</legend>
          <p className="muted">Runs the chosen step and every later one on the deployed commit.</p>
          <ol className="recipe mono">
            {steps.map(({ command, ranOkLastRun }, i) => {
              const index = i + 1;
              const notRun = index < step;
              return (
                <li key={index} className={notRun ? "step-not-run" : undefined}>
                  <label className="step-choice">
                    <input
                      type="radio"
                      name="from-step"
                      value={index}
                      checked={step === index}
                      onChange={() => setStep(index)}
                    />
                    <span className="recipe-index muted">{index}</span>
                    <code>{command}</code>
                    {notRun && (
                      <span className="muted step-skip-note">
                        not run this time{ranOkLastRun && " · ran OK in the last run"}
                      </span>
                    )}
                  </label>
                </li>
              );
            })}
          </ol>
        </fieldset>
      </div>
      <div className="workspace-footer">
        <button type="button" className="button" onClick={onBack}>
          Back
        </button>
        <div className="footer-end">
          <button type="submit" className="button button-primary" disabled={steps.length === 0}>
            Prepare run from step {step}
          </button>
        </div>
      </div>
    </form>
  );
}
