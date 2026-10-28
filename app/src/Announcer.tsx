import { useCallback, useState } from "react";

export type Announce = (text: string, urgent?: boolean) => void;

interface Said {
  text: string;
  /** Changes each time, so the same words are read again. */
  n: number;
}

/** Two live regions: polite for progress, assertive for refusals and end states.
 * They live outside the workspace, so an end state is still read after it closes. */
export function useAnnouncer() {
  const [polite, setPolite] = useState<Said>({ text: "", n: 0 });
  const [assertive, setAssertive] = useState<Said>({ text: "", n: 0 });
  const announce = useCallback<Announce>((text, urgent = false) => {
    (urgent ? setAssertive : setPolite)((prev) => ({ text, n: prev.n + 1 }));
  }, []);
  const regions = (
    <>
      <div className="visually-hidden" aria-live="polite" data-testid="announce-polite">
        <span key={polite.n}>{polite.text}</span>
      </div>
      <div className="visually-hidden" aria-live="assertive" data-testid="announce-assertive">
        <span key={assertive.n}>{assertive.text}</span>
      </div>
    </>
  );
  return { announce, regions };
}
