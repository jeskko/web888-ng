// SPDX-License-Identifier: GPL-3.0-or-later
// The configuration form, built from the schema the server serves: one
// fieldset per section, an editor per field by its type, the field's doc
// comment as help. Edits are checked by the server as you go (what
// applies at once, what needs a restart); Save writes the file.

import { useEffect, useMemo, useState } from "preact/hooks";
import { t } from "../i18n.ts";
import { call, get, type Changes, type ConfigView, type Json } from "./api.ts";
import { getIn, kindOf, label, parse, resolve, same, setIn, show, type Schema } from "./schema.ts";

interface Ctx {
  root: Schema;
  file: Json;
  running: Json;
  draft: Json;
  readOnly: boolean;
  set(path: string[], v: Json): void;
  error(path: string[], msg: string | null): void;
}

function Help({ s }: { s: Schema }) {
  return s.description ? <div class="help">{s.description.replace(/`/g, "")}</div> : null;
}

/** Notes beside a field: changed in the form, or running with another value. */
function Notes({ ctx, path }: { ctx: Ctx; path: string[] }) {
  const f = getIn(ctx.file, path);
  const r = getIn(ctx.running, path);
  const d = getIn(ctx.draft, path);
  return (
    <>
      {!same(d, f) && <span class="tag edit">{t("changed")}</span>}
      {!same(r, f) && (
        <span class="tag" title="The server runs with another value: set on its command line, or saved and waiting for a restart">
          running: {show(r ?? null) || "none"}
        </span>
      )}
    </>
  );
}

function TextField({ ctx, path, s }: { ctx: Ctx; path: string[]; s: Schema }) {
  const k = kindOf(s);
  const v = getIn(ctx.draft, path);
  const [text, setText] = useState(k.kind === "json" ? JSON.stringify(v ?? null) : show(v ?? null));
  const [err, setErr] = useState<string | null>(null);
  const onInput = (text: string) => {
    setText(text);
    const p = parse(k, text);
    const e = "error" in p ? p.error : null;
    setErr(e);
    ctx.error(path, e);
    if ("value" in p) ctx.set(path, p.value);
  };
  const nullable = (k.kind === "number" || k.kind === "text") && k.nullable;
  return (
    <>
      <input
        class={err ? "bad" : ""}
        aria-labelledby={`${fieldId(path)}-l`}
        aria-invalid={err ? true : undefined}
        aria-describedby={err ? `${fieldId(path)}-err` : undefined}
        // secrets in the config (e.g. [mqtt] password) aren't shown
        type={(path[path.length - 1] === "password" ? "password" : "text") as "text"}
        autocomplete="off"
        value={text}
        disabled={ctx.readOnly}
        placeholder={nullable ? t("(none)") : ""}
        spellcheck={false}
        onInput={(e) => onInput((e.target as HTMLInputElement).value)}
      />
      {err && (
        <span class="warn" id={`${fieldId(path)}-err`}>
          {err}
        </span>
      )}
    </>
  );
}

function Field({ ctx, path, schema }: { ctx: Ctx; path: string[]; schema: Schema }) {
  const s = resolve(schema, ctx.root);
  const k = kindOf(s);
  const key = path[path.length - 1];
  if (k.kind === "object") {
    return (
      <fieldset class="sub">
        <legend>{label(key)}</legend>
        <Help s={s} />
        {k.fields.map(([name, fs]) => (
          <Field key={name} ctx={ctx} path={[...path, name]} schema={fs} />
        ))}
      </fieldset>
    );
  }
  const v = getIn(ctx.draft, path);
  let editor;
  if (k.kind === "bool") {
    editor = (
      <input
        type="checkbox"
        checked={v === true}
        disabled={ctx.readOnly}
        onChange={(e) => ctx.set(path, (e.target as HTMLInputElement).checked)}
      />
    );
  } else if (k.kind === "choice") {
    editor = (
      <select
        value={typeof v === "string" ? v : ""}
        disabled={ctx.readOnly}
        onChange={(e) => {
          const x = (e.target as HTMLSelectElement).value;
          ctx.set(path, x === "" ? null : x);
        }}
      >
        {k.nullable && <option value="">{t("(none)")}</option>}
        {k.options.map((o) => (
          <option value={o.value} title={o.help}>
            {o.value}
          </option>
        ))}
      </select>
    );
  } else {
    editor = <TextField ctx={ctx} path={path} s={s} />;
  }
  const choiceHelp = k.kind === "choice" ? k.options.find((o) => o.value === v)?.help : undefined;
  return (
    <div class="field">
      <label class="name" id={`${fieldId(path)}-l`} title={path.join(".")}>
        {label(key)}
      </label>
      <div class="value" role="group" aria-labelledby={`${fieldId(path)}-l`}>
        <div class="line">
          {editor}
          <Notes ctx={ctx} path={path} />
        </div>
        <Help s={s} />
        {choiceHelp && <div class="help">{choiceHelp.replace(/`/g, "")}</div>}
      </div>
    </div>
  );
}

/** An id for a settings field's elements (its label, its error). */
const fieldId = (path: (string | number)[]) => `cfg-${path.join("-").replace(/[^\w-]/g, "_")}`;

export function ConfigPage({ onRestart }: { onRestart(): void }) {
  const [schema, setSchema] = useState<Schema | null>(null);
  const [cfg, setCfg] = useState<ConfigView | null>(null);
  const [draft, setDraft] = useState<Json>(null);
  /** Bumped to rebuild the fields (load, revert). */
  const [gen, setGen] = useState(0);
  const [errors, setErrors] = useState<Record<string, string>>({});
  const [check, setCheck] = useState<Changes | string | null>(null);
  const [saved, setSaved] = useState<Changes | null>(null);
  const [fail, setFail] = useState("");

  const load = async () => {
    try {
      const [s, c] = await Promise.all([get<Schema>("api/admin/schema"), get<ConfigView>("api/admin/config")]);
      setSchema(s);
      setCfg(c);
      setDraft(c.file);
      setErrors({});
      setGen((g) => g + 1);
    } catch (e) {
      setFail((e as Error).message);
    }
  };
  useEffect(() => void load(), []);

  const dirty = cfg !== null && !same(draft, cfg.file);
  const hasErrors = Object.keys(errors).length > 0;

  // the server says what the edits would need
  useEffect(() => {
    if (!dirty || hasErrors) {
      setCheck(null);
      return;
    }
    const timer = window.setTimeout(() => {
      call<Changes>("POST", "api/admin/config/validate", draft)
        .then(setCheck)
        .catch((e: Error) => setCheck(e.message));
    }, 400);
    return () => clearTimeout(timer);
  }, [draft, dirty, hasErrors]);

  const ctx = useMemo<Ctx | null>(
    () =>
      schema && cfg
        ? {
            root: schema,
            file: cfg.file,
            running: cfg.running,
            draft,
            readOnly: !cfg.writable,
            set: (path, v) => {
              setSaved(null);
              setDraft((d) => setIn(d, path, v));
            },
            error: (path, msg) =>
              setErrors((es) => {
                const k = path.join(".");
                if ((es[k] ?? null) === msg) return es;
                const n = { ...es };
                if (msg === null) delete n[k];
                else n[k] = msg;
                return n;
              }),
          }
        : null,
    [schema, cfg, draft],
  );

  const save = async () => {
    setFail("");
    try {
      const r = await call<Changes>("PUT", "api/admin/config", draft);
      await load();
      setSaved(r);
    } catch (e) {
      setFail((e as Error).message);
    }
  };

  if (fail && !ctx) return <p class="warn">{fail}</p>;
  if (!ctx || !schema) return <p class="dim">{t("Loading…")}</p>;
  const sections = Object.entries(schema.properties ?? {});
  return (
    <div class="config">
      <p class="dim">
        {cfg?.path ? <>{t("File: {path}.", { path: cfg.path })} </> : null}
        {cfg?.writable ? "" : t("This server has no config file to write; the form is read-only.") + " "}
        {t("Values marked “running” differ in the running server: given on its command line (which overrides the file), or saved and waiting for a restart.")}
      </p>
      <div class="savebar">
        <button class="on" disabled={!dirty || hasErrors || !cfg?.writable} onClick={save}>
          {t("Save")}
        </button>
        <button disabled={!dirty} onClick={() => void load()}>
          {t("Revert")}
        </button>
        {hasErrors && <span class="warn" role="alert">{t("Fix the marked fields first.")}</span>}
        {!hasErrors && typeof check === "string" && <span class="warn">{check}</span>}
        {!hasErrors && check && typeof check !== "string" && <ChangeNote c={check} />}
        {saved && (
          <>
            <span class="ok" role="status">{t("Saved.")}</span>
            <ChangeNote c={saved} />
            {saved.restart.length > 0 && <button onClick={onRestart}>{t("Restart now")}</button>}
          </>
        )}
        {fail && <span class="warn" role="alert">{fail}</span>}
      </div>
      <div key={gen}>
        {sections.map(([name, s]) => {
          const r = resolve(s, schema);
          return (
            <fieldset key={name}>
              <legend>{label(name)}</legend>
              <Help s={r} />
              {kindOf(r).kind === "object" ? (
                Object.entries(r.properties ?? {}).map(([f, fs]) => (
                  <Field key={f} ctx={ctx} path={[name, f]} schema={fs} />
                ))
              ) : (
                <Field ctx={ctx} path={[name]} schema={s} />
              )}
            </fieldset>
          );
        })}
      </div>
    </div>
  );
}

function ChangeNote({ c }: { c: Changes }) {
  if (c.restart.length > 0) {
    return <span class="warn">{t("Needs a restart: {what}.", { what: c.restart.join(", ") })}</span>;
  }
  return <span class="dim">{c.live ? t("Applies at once.") : t("No change for the running server.")}</span>;
}
