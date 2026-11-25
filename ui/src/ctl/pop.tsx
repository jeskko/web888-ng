// SPDX-License-Identifier: GPL-3.0-or-later
// A button that opens a box of controls beside it (above it in the
// footer, `up`): a click elsewhere or Escape closes it, the keys inside
// are its own (arrows move sliders, not the dial).

import type { ComponentChildren } from "preact";
import { useEffect, useRef, useState } from "preact/hooks";
import { usePopFocus } from "../a11y.ts";

export function Pop({
  label,
  title,
  on = false,
  up = false,
  right = false,
  cls = "",
  children,
}: {
  label: ComponentChildren;
  title: string;
  /** something behind it is on (the button lit) */
  on?: boolean;
  /** the box above the button */
  up?: boolean;
  /** the box's right edge at the button's (near the window's right) */
  right?: boolean;
  cls?: string;
  /** the box's contents; a function gets a way to close it */
  children: ComponentChildren | ((close: () => void) => ComponentChildren);
}) {
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
  const pop = usePopFocus(open);
  return (
    <div class={`group pop ${cls}`} ref={box}>
      <button class={`pop-btn${on ? " on" : ""}${open ? " open" : ""}`} aria-expanded={open} aria-haspopup="dialog" title={title} onClick={() => setOpen(!open)}>
        {label} {up ? "▴" : "▾"}
      </button>
      {open && (
        <div class={`pop-box${up ? " up" : ""}${right ? " right" : ""}`} ref={pop} role="group" aria-label={title} data-keys-own>
          {typeof children === "function" ? children(() => setOpen(false)) : children}
        </div>
      )}
    </div>
  );
}
