// SPDX-License-Identifier: GPL-3.0-or-later
// Frequency memories (src/memories.ts) and the second VFO. The Mem
// popup stores the dial under a name, lists the memories (a click
// tunes, ✕ forgets), and exports or imports them as a JSON file; key m
// opens it. A/B swaps the dial with the other VFO (key v), A=B copies
// the dial there. All kept in this browser.

import { signal } from "@preact/signals";
import { useEffect, useRef, useState } from "preact/hooks";
import { formatKhz } from "../fmt.ts";
import { t, tn } from "../i18n.ts";
import { exportText, MEM_NAME_MAX, memoryAt, type Memory } from "../memories.ts";
import { deleteMemory, equalVfo, importMemories, memories, recall, st, storeMemory, swapVfo, vfoName, vfoOther } from "../state.ts";

/** The Mem popup is open (the m key toggles it). */
export const memOpen = signal(false);

function tuningText(tu: { hz: number; mode: string }): string {
  return `${formatKhz(tu.hz)} kHz ${tu.mode.toUpperCase()}`;
}

/** A/B and A=B beside the dial. */
export function VfoCtl() {
  const s = st.value;
  if (!s) return null;
  const other = vfoOther.value;
  const me = vfoName.value;
  const them = me === "A" ? "B" : "A";
  return (
    <div class="group vfo">
      <button
        onClick={swapVfo}
        // i18n: VFO = variable frequency oscillator, one of two tuning memories (A and B) of the radio
        title={
          other
            ? t("VFO {me} now. Swap to VFO {them}: {tuning} (key v)", { me, them, tuning: tuningText(other) })
            : t("VFO {me}. Swap to VFO {them} (a copy of this one the first time; key v)", { me, them })
        }
      >
        <b>{me}</b>/{them}
      </button>
      <button onClick={equalVfo} title={t("VFO {them} to this frequency, mode and passband", { them })}>
        {me}={them}
      </button>
    </div>
  );
}

function save(name: string, text: string): void {
  const a = document.createElement("a");
  a.href = URL.createObjectURL(new Blob([text], { type: "application/json" }));
  a.download = name;
  a.click();
  setTimeout(() => URL.revokeObjectURL(a.href), 10_000);
}

function MemList({ close }: { close?: () => void }) {
  const s = st.value;
  const list = memories.value;
  const here = s ? memoryAt(list, s.hz, s.mode) : null;
  const [name, setName] = useState(here?.name ?? "");
  const [note, setNote] = useState("");
  const file = useRef<HTMLInputElement>(null);
  const pick = (m: Memory) => {
    recall(m);
    close?.();
  };
  return (
    <div class="mem-body">
      <form
        class="mem-store"
        onSubmit={(e) => {
          e.preventDefault();
          storeMemory(name);
          setNote(s ? t("Stored {tuning}", { tuning: tuningText(s) }) : "");
        }}
      >
        <input
          type="text"
          placeholder={t("name (optional)")}
          maxLength={MEM_NAME_MAX}
          value={name}
          onInput={(e) => setName(e.currentTarget.value)}
          // keys typed here are the name's, not the receiver's shortcuts
          // (Escape still closes the popup)
          onKeyDown={(e) => e.key !== "Escape" && e.stopPropagation()}
        />
        <button type="submit" disabled={!s} title={s ? t("Store {tuning} with its passband (replaces one at the same frequency and mode)", { tuning: tuningText(s) }) : ""}>
          {t("Store")}
        </button>
      </form>
      {list.length ? (
        <div class="mem-list">
          <table>
            <tbody>
              {list.map((m) => (
                <tr key={`${m.hz}${m.mode}`} class={m === here ? "pick here" : "pick"} onClick={() => pick(m)} title={t("Tune to {tuning}, passband {lo}..{hi} Hz", { tuning: tuningText(m), lo: m.lo, hi: m.hi })}>
                  <td class="r">{formatKhz(m.hz)}</td>
                  <td>{m.mode.toUpperCase()}</td>
                  <td class="mem-name">{m.name}</td>
                  <td>
                    <button
                      class="mem-x"
                      title={t("Forget this memory")}
                      onClick={(e) => {
                        e.stopPropagation();
                        deleteMemory(m);
                      }}
                    >
                      ✕
                    </button>
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      ) : (
        <p class="dim">{t("No memories yet. They are kept in this browser; export them to keep them elsewhere.")}</p>
      )}
      <div class="mem-file">
        <button disabled={!list.length} onClick={() => save("web888-memories.json", exportText(list))} title={t("Save the memories as a file")}>
          {t("Export")}
        </button>
        <button onClick={() => file.current?.click()} title={t("Add the memories of an exported file")}>
          {t("Import")}
        </button>
        <input
          ref={file}
          type="file"
          accept=".json,application/json"
          hidden
          onChange={async (e) => {
            const f = e.currentTarget.files?.[0];
            e.currentTarget.value = "";
            if (!f) return;
            try {
              setNote(tn("Imported {n} memory", "Imported {n} memories", importMemories(await f.text())));
            } catch {
              setNote(t("That file isn't a memories file"));
            }
          }}
        />
        <span class="dim">{note}</span>
      </div>
    </div>
  );
}

/** The Mem button and popup; `inline`: the list itself (the phone's sheet). */
export function MemCtl({ inline = false }: { inline?: boolean }) {
  const box = useRef<HTMLDivElement>(null);
  const open = memOpen.value;
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) memOpen.value = false;
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && (memOpen.value = false);
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
  if (inline) return <MemList />;
  return (
    <div class="group mem-ctl" ref={box}>
      <button class={open ? "on" : ""} onClick={() => (memOpen.value = !open)} title={t("Frequency memories (key m)")}>
        {/* i18n: Mem = short for Memories, stored frequencies */}
        {t("Mem")} {memories.value.length || ""}
      </button>
      {open && (
        <div class="mem-pop">
          <MemList close={() => (memOpen.value = false)} />
        </div>
      )}
    </div>
  );
}
