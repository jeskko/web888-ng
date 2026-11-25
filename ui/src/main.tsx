// SPDX-License-Identifier: GPL-3.0-or-later
// web888d web UI: the receiver (layout, keyboard shortcuts, start-audio
// overlay), or the admin pages under #/admin.

import { initI18n, t } from "./i18n.ts";
import { render } from "preact";
import { useEffect, useState } from "preact/hooks";
import {
  AudioCtl,
  Bandwidth,
  ContrastCtl,
  LookCtl,
  DspCtl,
  FreqDisplay,
  freqEdit,
  LeaseBanner,
  ModeBar,
  SMeter,
  StepCtl,
  StatusBar,
  UsersCtl,
  ZoomCtl,
} from "./ctl/controls.tsx";
import { MobileApp, PlayIcon } from "./ctl/mobile.tsx";
import { SpotPanel } from "./ctl/spots.tsx";
import { JttyPanel } from "./ctl/jtty.tsx";
import { CwPanel } from "./ctl/cw.tsx";
import { RttyPanel } from "./ctl/rtty.tsx";
import { PskPanel } from "./ctl/psk.tsx";
import { HfdlPanel } from "./ctl/hfdl.tsx";
import { Js8Panel } from "./ctl/js8.tsx";
import { ImgPanel } from "./ctl/img.tsx";
import { DigiPanel } from "./ctl/digi.tsx";
import { SigPanel } from "./ctl/sigid.tsx";
import { MapPanel } from "./ctl/map.tsx";
import { PanelsCtl } from "./ctl/decoders.tsx";
import { RecCtl } from "./ctl/rec.tsx";
import { TcPanel } from "./ctl/timecode.tsx";
import { ConfigCtl, ConfigPanel } from "./ctl/config.tsx";
import { MemCtl, memOpen, VfoCtl } from "./ctl/memories.tsx";
import { helpOpen, KeyHelp } from "./ctl/keys.tsx";
import { InfoPanel } from "./ctl/info.tsx";
import { PropCtl, PropPanel } from "./ctl/prop.tsx";
import { AfPanel } from "./ctl/afspec.tsx";
import { isMobile } from "./mobile.ts";
import { stepTo } from "./fmt.ts";
import type { Mode } from "./proto.ts";
import {
  audioError,
  audioOn,
  cancelLogin,
  guestOpen,
  cycleAm,
  conn,
  connNote,
  login,
  loginError,
  needLogin,
  reconnect,
  setMode,
  start,
  startAudio,
  swapVfo,
  st,
  stepOf,
  tune,
} from "./state.ts";
import { AdminApp } from "./admin/admin.tsx";
import { SafeBanner } from "./ctl/safe.tsx";
import { NearDial } from "./ctl/neardial.tsx";
import { announcement, keyIsTaken } from "./a11y.ts";
import { Pane } from "./view/pane.tsx";

const KEY_MODES: Record<string, Mode> = { u: "usb", l: "lsb", c: "cw", f: "nbfm", i: "iq" };

function useKeys() {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (keyIsTaken(e)) return;
      const s = st.value;
      if (!s) return;
      const step = stepOf(s.mode) * (e.shiftKey ? 10 : 1);
      if (e.key === "ArrowRight") tune(stepTo(s.hz, step, 1));
      else if (e.key === "ArrowLeft") tune(stepTo(s.hz, step, -1));
      else if (KEY_MODES[e.key]) setMode(KEY_MODES[e.key]);
      else if (e.key === "a") cycleAm();
      else if (e.key === "v") swapVfo();
      else if (e.key === "m") memOpen.value = !memOpen.value;
      else if (e.key === "?") helpOpen.value = true;
      else if (e.key === "g") freqEdit.value = true;
      else return;
      e.preventDefault();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);
}

function Login() {
  const guest = guestOpen.value > 0;
  const [pw, setPw] = useState("");
  const [busy, setBusy] = useState(false);
  const submit = async (e: Event) => {
    e.preventDefault();
    setBusy(true);
    await login(pw).finally(() => setBusy(false));
  };
  return (
    <form class="overlay login" onSubmit={submit}>
      <p>{guest ? t("Log in with the receiver's password for any of its receivers.") : t("This receiver is private.")}</p>
      <input
        type="password"
        aria-label={t("Password")}
        placeholder={t("Password")}
        autocomplete="current-password"
        autofocus
        value={pw}
        onInput={(e) => setPw((e.target as HTMLInputElement).value)}
      />
      <button type="submit" disabled={busy || !pw}>
        {guest ? t("Log in") : t("Listen")}
      </button>
      {guest && (
        <button type="button" onClick={cancelLogin}>
          {t("Stay a visitor")}
        </button>
      )}
      {loginError.value && <p class="warn" role="alert">{loginError.value}</p>}
    </form>
  );
}

function Overlay() {
  if (needLogin.value) return <Login />;
  if (conn.value === "ended") {
    return (
      <div class="overlay" role="alert">
        <p>{t("Session ended: {reason}.", { reason: connNote.value })}</p>
        <button onClick={reconnect}>{t("Reconnect")}</button>
      </div>
    );
  }
  if (conn.value === "busy") {
    return (
      <div class="overlay" role="alert">
        <p>{connNote.value || t("The receiver is in use.")}</p>
        <button onClick={reconnect}>{t("Try again")}</button>
      </div>
    );
  }
  if (audioOn.value) return null;
  return (
    <div class="overlay small">
      <button class="start" onClick={startAudio}>
        <PlayIcon /> {t("Start audio")}
      </button>
      {audioError.value && <p class="warn" role="alert">{audioError.value}</p>}
    </div>
  );
}

/** Where announce() speaks: a polite live region only screen readers hear. */
function Announcer() {
  return (
    <div class="sr-only" aria-live="polite" aria-atomic="true">
      {announcement.value}
    </div>
  );
}

function App() {
  useKeys();
  if (isMobile.value) return <MobileApp overlay={<Overlay />} />;
  return (
    <>
      {/* i18n: a heading only screen readers hear: the page's title */}
      <h1 class="sr-only">{t("Web-888 receiver")}</h1>
      <header>
        <FreqDisplay />
        <VfoCtl />
        <StepCtl />
        <ModeBar />
        <Bandwidth />
        <UsersCtl />
        <SMeter />
        {/* what opens beside the waterfall, and sharing */}
        <div class="group tools">
          <PanelsCtl />
          <MemCtl />
          <RecCtl />
          <PropCtl />
          <ConfigCtl />
        </div>
      </header>
      <LeaseBanner />
      <main>
        <div class="view">
          <Pane />
          <NearDial />
          <Overlay />
        </div>
        <SpotPanel />
        <CwPanel />
        <RttyPanel />
        <PskPanel />
        <TcPanel />
        <HfdlPanel />
        <Js8Panel />
        <ImgPanel />
        <JttyPanel />
        <DigiPanel />
        <SigPanel />
        <AfPanel />
        <MapPanel />
      </main>
      <footer>
        <div class="controls">
          <AudioCtl />
          <DspCtl />
          <ZoomCtl />
          <ContrastCtl />
          <LookCtl />
        </div>
        <StatusBar />
      </footer>
      <KeyHelp />
      <InfoPanel />
      <PropPanel />
      <ConfigPanel />
    </>
  );
}

const isAdmin = () => location.hash.startsWith("#/admin");
const admin = isAdmin();
// switching between the receiver and the admin pages starts afresh
window.addEventListener("hashchange", () => {
  if (isAdmin() !== admin) location.reload();
});
// the language before the first render (a missing catalogue: English);
// the receiver's connection starts meanwhile
if (!admin) void start();
await initI18n();
if (admin) {
  document.title = "web888d admin";
  render(
    <>
      <SafeBanner admin />
      <AdminApp />
    </>,
    document.getElementById("app")!,
  );
} else {
  render(
    <>
      <SafeBanner />
      <App />
      <Announcer />
    </>,
    document.getElementById("app")!,
  );
}
