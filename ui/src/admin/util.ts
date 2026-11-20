// SPDX-License-Identifier: GPL-3.0-or-later
// Small helpers the admin pages share.

import { useEffect, useRef } from "preact/hooks";

/** Call `f` now and every `ms` while mounted. */
export function usePoll(f: () => Promise<void>, ms: number) {
  const ref = useRef(f);
  ref.current = f;
  useEffect(() => {
    let stop = false;
    let timer: number | undefined;
    const run = async () => {
      await ref.current().catch(() => {});
      if (!stop) timer = window.setTimeout(run, ms);
    };
    void run();
    return () => {
      stop = true;
      clearTimeout(timer);
    };
  }, [ms]);
}

/** How long ago a Unix time was: "12 s", "5 min", "2 h 3 min". */
export function ago(unixS: number): string {
  const s = Math.max(0, Date.now() / 1000 - unixS);
  if (s < 60) return `${s.toFixed(0)} s`;
  if (s < 3600) return `${(s / 60).toFixed(0)} min`;
  return `${Math.floor(s / 3600)} h ${Math.floor((s % 3600) / 60)} min`;
}
