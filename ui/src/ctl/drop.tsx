// SPDX-License-Identifier: GPL-3.0-or-later
// A drop-down button: the current choice and ▾; a click lists the
// choices under it. A pick, a click elsewhere or Escape closes it.

import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { arrowNav } from "../a11y.ts";

export interface DropItem {
  key: string;
  label: ComponentChildren;
  title?: string;
  on: boolean;
  pick: () => void;
}

export function Drop({ label, title, items, cls = "" }: { label: ComponentChildren; title: string; items: DropItem[]; cls?: string }) {
  const [open, setOpen] = useState(false);
  const box = useRef<HTMLDivElement>(null);
  const btn = useRef<HTMLButtonElement>(null);
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => {
      if (e.key !== "Escape") return;
      setOpen(false);
      btn.current?.focus();
    };
    // the current choice takes the focus, for the arrow keys
    requestAnimationFrame(() => (box.current?.querySelector<HTMLElement>(".drop-pop .on") ?? box.current?.querySelector<HTMLElement>(".drop-pop button"))?.focus());
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
  return (
    <div class={`drop ${cls}`} ref={box}>
      <button ref={btn} class={`drop-btn${open ? " open" : ""}`} title={title} aria-expanded={open} onClick={() => setOpen(!open)}>
        {label} ▾
      </button>
      {open && (
        <div class="drop-pop" role="group" aria-label={title} data-keys-own onKeyDown={arrowNav}>
          {items.map((it) => (
            <button
              key={it.key}
              class={it.on ? "on" : ""} aria-pressed={!!(it.on)}
              title={it.title}
              onClick={() => {
                setOpen(false);
                it.pick();
                btn.current?.focus();
              }}
            >
              {it.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}
