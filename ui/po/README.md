# Translating the web UI

The receiver's web page (and its admin pages) can be shown in other
languages. The texts are in standard gettext PO files, one per language,
in this directory:

| file | |
|---|---|
| `web888d-ui.pot` | the template: every English text, made from the source; don't edit |
| `fi.po`, `sv.po`, `de.po`, ... | one language each |
| `GLOSSARY.md` | the technical terms and how each language says them |

The page picks the listener's browser language when there is a
catalogue for it, and anyone can choose another in **Config → Language**.
A text without a translation shows in English.

## Fixing or finishing a translation

1. Edit `po/xx.po` with any PO editor (Poedit, Lokalize, Gtranslator,
   Weblate) or a text editor. Each message is:

   ```po
   #. a note for translators, from the source
   #: src/ctl/config.tsx:35
   msgid "Language"
   msgstr "Kieli"
   ```

   Leave `msgstr ""` empty for what you haven't translated. A `#, fuzzy`
   message is a guess: it isn't used until the flag is removed.
2. Check it: `cd ui && node scripts/i18n.mjs check` (placeholders, the
   header) and see it: `npm run build`, then run web888d with
   `--ui-dir ui/dist` (see the main README) and pick the language.
3. Send a pull request with the `.po` file, or the file itself in an
   issue if git isn't your thing.

## Adding a language

```sh
cd ui
node scripts/i18n.mjs new pl "Polski"   # po/pl.po from the template
```

Set the `Plural-Forms` line in the new file's header for your language
(the GNU gettext manual lists them; English, Finnish, Swedish and German
use `nplurals=2; plural=(n != 1);`). Then translate as above. The page
offers the language as soon as `po/pl.po` exists; the language's name
in the menu is the header's `X-Language-Name`.

## What to keep in mind

* **Placeholders** in braces, `{n}`, `{name}`, are filled in by the
  page: keep each one, in the place your sentence needs it. `check`
  fails if one is missing or misspelt.
* **Plurals**: messages with `msgid_plural` have one `msgstr[k]` per
  plural form of your language.
* **Context**: a `msgctxt` line tells apart the same English word in
  different senses ("Peak" of the spectrum display, "Peak" of the ADC).
* **Keep as they are**: mode names (USB, LSB, CW, AM, SAM, FM, IQ, FT8,
  FT4, WSPR, JS8, RTTY, NAVTEX, DSC, PSK31, HFDL, SSTV), units (Hz, kHz,
  MHz, dB, dBFS, dBm, kbit/s, S/s, UTC), callsigns and station names,
  and key names in the key help (`m`, `v`, `?`).
* **Radio terms**: use what the radio amateurs and SWL listeners of your
  language use, not a dictionary's word; when the English term is what
  they say (AGC, squelch in some languages), keep it. Add your choices to
  `GLOSSARY.md` so the next translator says the same.
* Short texts on buttons must stay short: the header has little room.
  Longer explanations are in tooltips (`title`), where a full sentence
  is fine.

## For developers

Everything the user reads goes through `src/i18n.ts`:

```tsx
import { N_, t, tc, tn, tx } from "../i18n.ts";

<button title={t("Automatic notch: steady carriers out of the audio")}>{t("Notch")}</button>
t("{n} kbit/s", { n: rate })                       // placeholders
tn("{n} listener", "{n} listeners", count)         // plural; {n} is count
tc("spectrum", "Peak")                             // a word in one sense
tx("Read the {guide} first.", { guide: <a href="...">{t("guide")}</a> })   // elements in a sentence
const MODES = [{ label: N_("Fast") }];             // in a constant: mark it...
{t(m.label)}                                       // ...translate it where shown
```

* The message is the whole English sentence, never pieces glued
  together: word order differs between languages. Numbers, names and
  frequencies go in as placeholders.
* A comment `// i18n: ...` on the line before the call (or at its end)
  reaches the translators as a note: say what a technical word means
  there ("SAM: synchronous AM", "bin: one channel of the
  channelizer, the FPGA's filter bank").
* Not translated: mode names on their own, units on their own, data
  from the receiver or other stations (spots, decoded text, station
  names), log messages, protocol strings, CSS classes, storage keys.
* After changing texts: `node scripts/i18n.mjs extract` then
  `node scripts/i18n.mjs update` (the template, then every language:
  new texts untranslated, removed ones kept commented out for reuse).
  CI runs `check`, which fails when the template is out of date.
* Components re-render when the language changes, because t() reads a
  signal: call t() during render, not once at module load.
