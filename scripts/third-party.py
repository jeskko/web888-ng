#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-3.0-or-later
"""The third-party libraries in web888d and its web UI, with their
licences: `crates/web/src/third-party.json`, which the server embeds and
the info page's Licences view lists (`/api/info/licences`).

* Rust: the crates `web888d` builds with for the device
  (armv7-unknown-linux-musleabihf; normal and build dependencies), from
  `cargo metadata`.
* Browser: the UI's `dependencies` (ui/package.json) and theirs, from
  `ui/node_modules` (run `pnpm install` first).

  scripts/third-party.py          # write the file
  scripts/third-party.py --check  # fail if it is out of date (CI)
"""

import json
import os
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
OUT = os.path.join(ROOT, "crates/web/src/third-party.json")
TARGET = "armv7-unknown-linux-musleabihf"


def git_url(source):
    """A git dependency's repository (what we build from)."""
    if not source.startswith("git+"):
        return None
    return source.removeprefix("git+").split("?")[0].split("#")[0]


def rust():
    m = json.loads(
        subprocess.check_output(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", TARGET],
            cwd=ROOT,
        )
    )
    pk = {p["id"]: p for p in m["packages"]}
    nodes = {n["id"]: n for n in m["resolve"]["nodes"]}
    root = next(p["id"] for p in m["packages"] if p["name"] == "web888d")
    seen, todo = set(), [root]
    while todo:
        i = todo.pop()
        if i in seen:
            continue
        seen.add(i)
        for d in nodes[i]["deps"]:
            if any(k["kind"] in (None, "build") for k in d["dep_kinds"]):
                todo.append(d["pkg"])
    out = []
    for i in seen:
        p = pk[i]
        if not p["source"]:  # our own crates
            continue
        out.append(
            {
                "name": p["name"],
                "version": p["version"],
                "license": p["license"] or "",
                "url": git_url(p["source"])
                or p["repository"]
                or p["homepage"]
                or f"https://crates.io/crates/{p['name']}",
            }
        )
    return sorted(out, key=lambda p: (p["name"], p["version"]))


def npm_dir(name, start):
    """The package's directory as Node finds it from `start`."""
    d = start
    while True:
        cand = os.path.join(d, "node_modules", name)
        if os.path.isfile(os.path.join(cand, "package.json")):
            return os.path.realpath(cand)
        up = os.path.dirname(d)
        if up == d:
            raise SystemExit(f"{name}: not in node_modules (pnpm install in ui/?)")
        d = up


def npm():
    ui = os.path.join(ROOT, "ui")
    deps = json.load(open(os.path.join(ui, "package.json")))["dependencies"]
    seen, out, todo = set(), [], [(n, ui) for n in deps]
    while todo:
        name, start = todo.pop()
        d = npm_dir(name, start)
        if d in seen:
            continue
        seen.add(d)
        p = json.load(open(os.path.join(d, "package.json")))
        repo = p.get("repository")
        url = (repo.get("url") if isinstance(repo, dict) else repo) or p.get("homepage") or ""
        url = url.removeprefix("git+").removesuffix(".git").replace("git://", "https://")
        out.append({"name": p["name"], "version": p["version"], "license": p.get("license", ""), "url": url})
        # a pnpm package's own dependencies sit beside it in its store dir
        base = os.path.dirname(os.path.dirname(d)) if name.startswith("@") else os.path.dirname(d)
        todo += [(n, base) for n in p.get("dependencies", {})]
    return sorted(out, key=lambda p: (p["name"], p["version"]))


def main():
    text = json.dumps({"rust": rust(), "npm": npm()}, indent=1, ensure_ascii=False) + "\n"
    if "--check" in sys.argv:
        old = open(OUT).read() if os.path.exists(OUT) else ""
        if old != text:
            sys.exit("crates/web/src/third-party.json is out of date: run scripts/third-party.py")
        return
    open(OUT, "w").write(text)
    print(f"{OUT}: written")


if __name__ == "__main__":
    main()
