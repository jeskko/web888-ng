// SPDX-License-Identifier: GPL-3.0-or-later
// A drop-down button: the current choice and ▾; a click lists the
// choices under it. A pick, a click elsewhere or Escape closes it.

import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";

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
  useEffect(() => {
    if (!open) return;
    const away = (e: PointerEvent) => {
      if (!box.current?.contains(e.target as Node)) setOpen(false);
    };
    const key = (e: KeyboardEvent) => e.key === "Escape" && setOpen(false);
    document.addEventListener("pointerdown", away);
    document.addEventListener("keydown", key);
    return () => {
      document.removeEventListener("pointerdown", away);
      document.removeEventListener("keydown", key);
    };
  }, [open]);
  return (
    <div class={`drop ${cls}`} ref={box}>
      <button class={`drop-btn${open ? " open" : ""}`} title={title} aria-haspopup="listbox" aria-expanded={open} onClick={() => setOpen(!open)}>
        {label} ▾
      </button>
      {open && (
        <div class="drop-pop" role="listbox">
          {items.map((it) => (
            <button
              key={it.key}
              role="option"
              aria-selected={it.on}
              class={it.on ? "on" : ""}
              title={it.title}
              onClick={() => {
                setOpen(false);
                it.pick();
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
