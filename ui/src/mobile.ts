// SPDX-License-Identifier: GPL-3.0-or-later
// Whether to draw the phone layout: a narrow window, or a touch device
// held sideways (short). `?ui=mobile` or `?ui=desktop` forces one.

import { signal } from "@preact/signals";

const query = window.matchMedia("(max-width: 640px), (pointer: coarse) and (max-height: 500px)");
const forced = new URLSearchParams(location.search).get("ui");

const want = () => (forced === "mobile" ? true : forced === "desktop" ? false : query.matches);

export const isMobile = signal(want());
query.addEventListener("change", () => (isMobile.value = want()));
