// SPDX-License-Identifier: GPL-3.0-or-later
// web888d web UI: layout, keyboard shortcuts, start-audio overlay.

import { render } from "preact";
import { useEffect } from "preact/hooks";
import { AudioCtl, Bandwidth, ContrastCtl, FreqDisplay, ModeBar, SMeter, StatusBar, ZoomCtl } from "./ctl/controls.tsx";
import { stepFor } from "./fmt.ts";
import type { Mode } from "./proto.ts";
import { audioError, audioOn, conn, connNote, connect, reconnect, setMode, startAudio, st, tune } from "./state.ts";
import { Pane } from "./view/pane.tsx";

const KEY_MODES: Record<string, Mode> = { u: "usb", l: "lsb", c: "cw", a: "am" };

function useKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (t.tagName === "INPUT" || e.ctrlKey || e.metaKey || e.altKey) return;
      const s = st.value;
      if (!s) return;
      const step = stepFor(s.mode) * (e.shiftKey ? 10 : 1);
      if (e.key === "ArrowRight") tune(s.hz + step);
      else if (e.key === "ArrowLeft") tune(s.hz - step);
      else if (KEY_MODES[e.key]) setMode(KEY_MODES[e.key]);
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

function Overlay() {
  if (conn.value === "busy") {
    return (
      <div class="overlay">
        <p>{connNote.value || "The receiver is in use."}</p>
        <button onClick={reconnect}>Try again</button>
      </div>
    );
  }
  if (audioOn.value) return null;
  return (
    <div class="overlay small">
      <button class="start" onClick={startAudio}>
        ▶ Start audio
      </button>
      {audioError.value && <p class="warn">{audioError.value}</p>}
    </div>
  );
}

function App() {
  useKeys();
  return (
    <>
      <header>
        <FreqDisplay />
        <ModeBar />
        <Bandwidth />
        <SMeter />
      </header>
      <main>
        <Pane />
        <Overlay />
      </main>
      <footer>
        <div class="controls">
          <AudioCtl />
          <ZoomCtl />
          <ContrastCtl />
        </div>
        <StatusBar />
      </footer>
    </>
  );
}

connect();
render(<App />, document.getElementById("app")!);
