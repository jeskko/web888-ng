// SPDX-License-Identifier: GPL-3.0-or-later
// The config form's reading of the server's schema (fixture checked
// against the server by crates/web/tests/admin.rs).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { getIn, kindOf, label, parse, resolve, same, setIn, type Schema } from "../src/admin/schema.ts";

const root = JSON.parse(
  readFileSync(new URL("../../crates/web/tests/proto/schema.json", import.meta.url), "utf8"),
) as Schema;

/** The resolved schema of the field at `path`. */
function field(path: string[]): Schema {
  let s: Schema = root;
  for (const k of path) s = resolve(resolve(s, root).properties![k], root);
  return s;
}

test("every field gets an editor, and the plain ones a plain one", () => {
  const seen: Record<string, string> = {};
  const walk = (s: Schema, path: string[]) => {
    const k = kindOf(resolve(s, root));
    if (k.kind === "object") for (const [n, f] of k.fields) walk(f, [...path, n]);
    else seen[path.join(".")] = k.kind;
  };
  walk(root, []);
  assert.equal(seen["radio.backend"], "choice");
  assert.equal(seen["web.tls"], "choice");
  assert.equal(seen["web.metrics"], "bool");
  assert.equal(seen["limits.per_ip"], "number");
  assert.equal(seen["web.default_freq"], "number");
  assert.equal(seen["web.listen"], "text");
  assert.equal(seen["web.tls_cert"], "text");
  assert.equal(seen["radio.iq.loop"], "bool");
  // tuples and maps: JSON
  assert.equal(seen["radio.sim.tones"], "json");
  assert.equal(seen["radio.iq.gains"], "json");
  assert.ok(!("auth" in (root.properties ?? {})));
});

test("choices carry their doc comments; numbers know null and integers", () => {
  const b = kindOf(field(["radio", "backend"]));
  assert.ok(b.kind === "choice");
  assert.deepEqual(
    b.options.map((o) => o.value),
    ["sim", "zynqsdr", "iq"],
  );
  assert.match(b.options[0].help ?? "", /Simulated/);
  const f = kindOf(field(["web", "default_freq"]));
  assert.deepEqual(f, { kind: "number", integer: false, nullable: true });
  const n = kindOf(field(["limits", "per_ip"]));
  assert.deepEqual(n, { kind: "number", integer: true, nullable: false });
  assert.match(field(["limits", "per_ip"]).description ?? "", /per client address/);
});

test("parsing input text", () => {
  const num = { kind: "number", integer: false, nullable: true } as const;
  const int = { kind: "number", integer: true, nullable: false } as const;
  assert.deepEqual(parse(num, " 7.074e6 "), { value: 7.074e6 });
  assert.deepEqual(parse(num, ""), { value: null });
  assert.ok("error" in parse(int, ""));
  assert.ok("error" in parse(int, "1.5"));
  assert.ok("error" in parse(int, "-1"));
  assert.ok("error" in parse(num, "abc"));
  assert.deepEqual(parse({ kind: "text", nullable: true }, ""), { value: null });
  assert.deepEqual(parse({ kind: "text", nullable: false }, ""), { value: "" });
  assert.deepEqual(parse({ kind: "json" }, '{"LNA": 16}'), { value: { LNA: 16 } });
  assert.ok("error" in parse({ kind: "json" }, "[1,"));
});

test("paths, equality and labels", () => {
  const c = { web: { listen: "a", wf_budget: 30 }, limits: { per_ip: 4 } };
  const d = setIn(c, ["web", "wf_budget"], 6);
  assert.equal(getIn(d, ["web", "wf_budget"]), 6);
  assert.equal(getIn(c, ["web", "wf_budget"]), 30, "the original is untouched");
  assert.equal(getIn(d, ["limits", "per_ip"]), 4);
  assert.equal(getIn(d, ["nope", "x"]), undefined);
  assert.ok(same({ a: 1, b: [1, 2] }, { b: [1, 2], a: 1 }));
  assert.ok(!same({ a: 1 }, { a: 1, b: null }));
  assert.ok(!same([1, 2], [2, 1]));
  assert.equal(label("wf_budget"), "WF budget");
  assert.equal(label("per_ip"), "Per IP");
});
