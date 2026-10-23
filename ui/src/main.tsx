// SPDX-License-Identifier: GPL-3.0-or-later
// web888d web UI: the receiver (layout, keyboard shortcuts, start-audio
// overlay), or the admin pages under #/admin.

import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import {
  AudioCtl,
  Bandwidth,
  ContrastCtl,
  DspCtl,
  FreqDisplay,
  LeaseBanner,
  ModeBar,
  ShareCtl,
  SMeter,
  StatusBar,
  ZoomCtl,
} from "./ctl/controls.tsx";
import { SpotPanel, SpotsCtl } from "./ctl/spots.tsx";
import { stepFor } from "./fmt.ts";
import type { Mode } from "./proto.ts";
import {
  audioError,
  audioOn,
  conn,
  connNote,
  login,
  loginError,
  needLogin,
  reconnect,
  setMode,
  start,
  startAudio,
  st,
  tune,
} from "./state.ts";
import { AdminApp } from "./admin/admin.tsx";
import { Pane } from "./view/pane.tsx";

const KEY_MODES: Record<string, Mode> = { u: "usb", l: "lsb", c: "cw", a: "am" };

function useKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (t.tagName === "INPUT" || t.tagName === "SELECT" || e.ctrlKey || e.metaKey || e.altKey) return;
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

function Login() {
  const [pw, setPw] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async (e: Event) => {
    e.preventDefault();
    setBusy(true);
    await login(pw).finally(() => setBusy(false));
  };
  return (
    <form class="overlay login" onSubmit={submit}>
      <p>This receiver is private.</p>
      <input
        type="password"
        placeholder="Password"
        autocomplete="current-password"
        autofocus
        value={pw}
        onInput={(e) => setPw((e.target as HTMLInputElement).value)}
      />
      <button type="submit" disabled={busy || !pw}>
        Listen
      </button>
      {loginError.value && <p class="warn">{loginError.value}</p>}
    </form>
  );
}

function Overlay() {
  if (needLogin.value) return <Login />;
  if (conn.value === "ended") {
    return (
      <div class="overlay">
        <p>Session ended: {connNote.value}.</p>
        <button onClick={reconnect}>Reconnect</button>
      </div>
    );
  }
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
      <LeaseBanner />
      <main>
        <div class="view">
          <Pane />
          <Overlay />
        </div>
        <SpotPanel />
      </main>
      <footer>
        <div class="controls">
          <AudioCtl />
          <DspCtl />
          <ZoomCtl />
          <ContrastCtl />
          <ShareCtl />
          <SpotsCtl />
        </div>
        <StatusBar />
      </footer>
    </>
  );
}

const isAdmin = () => location.hash.startsWith("#/admin");
const admin = isAdmin();
// switching between the receiver and the admin pages starts afresh
window.addEventListener("hashchange", () => {
  if (isAdmin() !== admin) location.reload();
});
if (admin) {
  document.title = "web888d admin";
  render(<AdminApp />, document.getElementById("app")!);
} else {
  void start();
  render(<App />, document.getElementById("app")!);
}
