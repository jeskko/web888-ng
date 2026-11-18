// SPDX-License-Identifier: GPL-3.0-or-later
// Accessibility helpers: modal dialogs that take the focus (and give it
// back), keep Tab inside and close on Escape; the global keys stay quiet
// while one is open; and a polite announcement for screen readers.

import { signal } from "@preact/signals";
import { useEffect, useRef } from "preact/hooks";

/** How many modal dialogs are open: the page's own keys wait meanwhile. */
let modals = 0;
export const modalOpen = () => modals > 0;

const FOCUSABLE =
  'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';

function focusables(box: HTMLElement): HTMLElement[] {
  return [...box.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((e) => e.offsetParent !== null || e === document.activeElement);
}

/** A modal dialog's behaviour, for the element the returned ref is put on
 * (the box with role="dialog"): while `open`, focus moves in (to an
 * element marked data-autofocus, else the first control, else the box),
 * Tab and Shift+Tab cycle inside, Escape calls `close`; when it closes
 * the focus returns to where it was. */
export function useDialog<T extends HTMLElement = HTMLDivElement>(open: boolean, close: () => void) {
  const ref = useRef<T>(null);
  const closeRef = useRef(close);
  closeRef.current = close;
  useEffect(() => {
    if (!open) return;
    const opener = document.activeElement as HTMLElement | null;
    modals++;
    const first = () => {
      const b = ref.current;
      if (!b) return;
      const want = b.querySelector<HTMLElement>("[data-autofocus]") ?? focusables(b)[0];
      if (want) want.focus();
      else {
        if (!b.hasAttribute("tabindex")) b.tabIndex = -1;
        b.focus();
      }
    };
    const raf = requestAnimationFrame(first);
    const onKey = (e: KeyboardEvent) => {
      const b = ref.current;
      if (!b) return;
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopPropagation();
        closeRef.current();
      } else if (e.key === "Tab") {
        const list = focusables(b);
        if (!list.length) {
          e.preventDefault();
          return;
        }
        const i = list.indexOf(document.activeElement as HTMLElement);
        if (e.shiftKey && (i <= 0 || !b.contains(document.activeElement))) {
          e.preventDefault();
          list[list.length - 1].focus();
        } else if (!e.shiftKey && (i === list.length - 1 || !b.contains(document.activeElement))) {
          e.preventDefault();
          list[0].focus();
        }
      }
    };
    document.addEventListener("keydown", onKey, true);
    return () => {
      cancelAnimationFrame(raf);
      modals--;
      document.removeEventListener("keydown", onKey, true);
      if (opener && document.contains(opener)) opener.focus();
    };
  }, [open]);
  return ref;
}

/** Arrow keys (Up/Down, Home/End) move the focus between the buttons in
 * a list (a drop-down's choices). For onKeyDown on the list's box. */
export function arrowNav(e: KeyboardEvent) {
  const box = e.currentTarget as HTMLElement;
  const list = [...box.querySelectorAll<HTMLElement>("button:not([disabled])")];
  const i = list.indexOf(document.activeElement as HTMLElement);
  let j = -1;
  if (e.key === "ArrowDown" || e.key === "ArrowRight") j = (i + 1) % list.length;
  else if (e.key === "ArrowUp" || e.key === "ArrowLeft") j = (i - 1 + list.length) % list.length;
  else if (e.key === "Home") j = 0;
  else if (e.key === "End") j = list.length - 1;
  if (j < 0) return;
  e.preventDefault();
  e.stopPropagation();
  list[j]?.focus();
}

/** Whether a key event belongs to something that wants the key itself (a
 * text field, a list with arrow keys, a dialog): the page's shortcuts
 * leave it alone. */
export function keyIsTaken(e: KeyboardEvent): boolean {
  if (e.ctrlKey || e.metaKey || e.altKey || e.isComposing || modalOpen()) return true;
  const el = e.target as HTMLElement | null;
  if (!el) return false;
  if (el.isContentEditable || ["INPUT", "SELECT", "TEXTAREA"].includes(el.tagName)) return true;
  return !!el.closest?.("[role=dialog], [data-keys-own]");
}

/** The latest polite announcement (an aria-live region in the page reads
 * it out): connection changes, queue position, a decoder's new text when
 * the listener asks for that. */
export const announcement = signal("");
let clearTimer = 0;
export function announce(text: string) {
  // the same text twice is announced twice: clear, then set
  announcement.value = "";
  clearTimeout(clearTimer);
  setTimeout(() => (announcement.value = text), 50);
  clearTimer = window.setTimeout(() => (announcement.value = ""), 10_000);
}

/** A clickable table row that the keyboard can reach too: Tab stops on
 * it, Enter or Space does what the click does. Spread on the <tr>. */
export function rowAction(act: (() => void) | undefined) {
  if (!act) return {};
  return {
    onClick: act,
    tabIndex: 0,
    onKeyDown: (e: KeyboardEvent) => {
      // not for a button inside the row (its own Enter)
      if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
        e.preventDefault();
        act();
      }
    },
  };
}

/** A popover's focus: when `open` turns on, the focus moves to its first
 * control (or the element marked data-autofocus); when it closes and the
 * focus was inside, it goes back to where it came from. Put the ref on
 * the popover's box. */
export function usePopFocus<T extends HTMLElement = HTMLDivElement>(open: boolean) {
  const ref = useRef<T>(null);
  useEffect(() => {
    if (!open) return;
    const opener = document.activeElement as HTMLElement | null;
    const raf = requestAnimationFrame(() => {
      const b = ref.current;
      (b?.querySelector<HTMLElement>("[data-autofocus]") ?? (b ? focusables(b)[0] : null))?.focus();
    });
    return () => {
      cancelAnimationFrame(raf);
      const inside = !document.activeElement || document.activeElement === document.body || ref.current?.contains(document.activeElement);
      if (inside && opener && document.contains(opener)) opener.focus();
    };
  }, [open]);
  return ref;
}
