// Safe mode's notice (`/api/safe`; web888d --safe, web888-ng-os
// docs/safe-mode.md): the unit runs without its radio. On the admin page
// it can also end safe mode (the unit reboots).
import { useEffect, useState } from "preact/hooks";
import { api } from "../state.ts";

interface SafeInfo {
  safe: boolean;
  reason?: string;
}

async function safeInfo(): Promise<SafeInfo> {
  const r = await fetch(api("api/safe"));
  if (!r.ok) return { safe: false };
  return (await r.json()) as SafeInfo;
}

export function SafeBanner({ admin }: { admin?: boolean }) {
  const [info, setInfo] = useState<SafeInfo>({ safe: false });
  const [state, setState] = useState<"" | "leaving" | string>("");
  useEffect(() => {
    safeInfo()
      .then(setInfo)
      .catch(() => {});
  }, []);
  if (!info.safe) return null;
  const leave = async () => {
    if (!confirm("Leave safe mode? The unit reboots and starts the radio again.")) return;
    setState("leaving");
    try {
      const r = await fetch(api("api/admin/safe/leave"), { method: "POST" });
      if (!r.ok) throw new Error((await r.text()) || `HTTP ${r.status}`);
    } catch (e) {
      setState((e as Error).message);
      return;
    }
    // back when the unit answers again, out of safe mode or not
    const again = async () => {
      try {
        const i = await safeInfo();
        if (!i.safe) return location.reload();
      } catch {
        // rebooting
      }
      setTimeout(() => void again(), 5000);
    };
    setTimeout(() => void again(), 15000);
  };
  return (
    <div class="safe-banner" role="alert">
      <strong>Safe mode:</strong> the radio is off{info.reason ? ` (${info.reason})` : ""}.{" "}
      {state === "leaving" ? (
        <span>Rebooting…</span>
      ) : admin ? (
        <button onClick={() => void leave()}>Leave safe mode</button>
      ) : (
        <a href="#/admin">Admin page</a>
      )}
      {state && state !== "leaving" && <span class="warn"> {state}</span>}
    </div>
  );
}
