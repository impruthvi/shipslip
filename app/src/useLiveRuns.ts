import { useEffect, useRef, useState } from "react";
import { deploy } from "./deployApi";
import type { LiveRuns } from "./types";

const POLL_MS = 2000;
const TICK_MS = 1000;
const NONE: LiveRuns = { runs: [], closedPreviews: [] };

/** Runs going now, asked of Rust every 2 seconds. `onEnded` hears a run
 * leave the list, so the map can show its result. `now` ticks while any run. */
export function useLiveRuns(onEnded: () => void): { live: LiveRuns; now: number } {
  const [live, setLive] = useState<LiveRuns>(NONE);
  const [now, setNow] = useState(() => Date.now());
  const ended = useRef(onEnded);
  useEffect(() => {
    ended.current = onEnded;
  });

  useEffect(() => {
    let gone = false;
    let known = new Set<string>();
    const ask = () =>
      deploy.liveRuns().then(
        (next) => {
          if (gone) return;
          const handles = new Set(next.runs.map((run) => run.handle));
          if ([...known].some((handle) => !handles.has(handle))) ended.current();
          known = handles;
          setLive(next);
        },
        // Sessions unavailable: nothing is live from this window.
        () => {},
      );
    void ask();
    const timer = window.setInterval(ask, POLL_MS);
    return () => {
      gone = true;
      window.clearInterval(timer);
    };
  }, []);

  const running = live.runs.length > 0;
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now()), TICK_MS);
    return () => window.clearInterval(timer);
  }, [running]);

  return { live, now };
}
