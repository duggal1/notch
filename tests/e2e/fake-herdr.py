#!/usr/bin/env python3
"""Scripted herdr stand-in for CONTROL-PLANE proofs only (not a live-herdr claim).

Instant, deterministic pane lifecycle so the supervisor decision loop can be
driven through loop detection / verifier-DONE without waiting on a real agent.
All state is in FAKE_HERDR_STATE. No real herdr command is ever executed.
"""
from __future__ import annotations

import json
import os
import sys

STATE = os.environ["FAKE_HERDR_STATE"]
DB = os.path.join(STATE, "agents.json")
CTR = os.path.join(STATE, "counter")
WS = os.environ.get("FAKE_HERDR_WS", "zz")


def load():
    try:
        return json.load(open(DB))
    except Exception:
        return {}


def save(db):
    json.dump(db, open(DB, "w"))


def counter():
    try:
        n = int(open(CTR).read())
    except Exception:
        n = 0
    n += 1
    open(CTR, "w").write(str(n))
    return n


def main():
    argv = sys.argv[1:]
    if not argv:
        return 2
    group, cmd = argv[0], argv[1] if len(argv) > 1 else ""
    db = load()

    if group == "pane" and cmd == "split":
        pane = f"{WS}:p{counter()}"
        print(json.dumps({"result": {"pane": {"pane_id": pane}}}))
        return 0
    if group == "pane" and cmd == "list":
        print(json.dumps({"result": {"panes": [
            {"pane_id": a["pane"], "agent_status": a["status"], "revision": a["rev"],
             "name": n, "cwd": a.get("cwd", "")} for n, a in db.items()]}}))
        return 0
    if group == "pane" and cmd == "close":
        pane = argv[2]
        for n in [n for n, a in db.items() if a["pane"] == pane]:
            del db[n]
        save(db)
        print(json.dumps({"result": {"type": "ok"}}))
        return 0
    if group == "agent" and cmd == "list":
        print(json.dumps({"result": {"agents": [
            {"pane_id": a["pane"], "agent_status": a["status"], "revision": a["rev"],
             "name": n, "cwd": a.get("cwd", "")} for n, a in db.items()]}}))
        return 0
    if group == "agent" and cmd == "start":
        name = argv[2]
        pane = ""
        if "--pane" in argv:
            pane = argv[argv.index("--pane") + 1]
        db[name] = {"pane": pane, "status": "idle", "rev": 1, "tail": "seed"}
        save(db)
        print(json.dumps({"result": {"type": "ok"}}))
        return 0
    if group == "agent" and cmd == "read":
        target = argv[2]
        a = db.get(target) or next((a for a in db.values() if a["pane"] == target), None)
        lines = 200
        if "--lines" in argv:
            lines = int(argv[argv.index("--lines") + 1])
        print("\n".join((a or {}).get("tail", "").splitlines()[-lines:]))
        return 0
    if group == "agent" and cmd == "prompt":
        target, text = argv[2], " ".join(argv[3:])
        a = db.get(target) or next((a for a in db.values() if a["pane"] == target), None)
        if a:
            a["tail"] += "\n" + text
            a["rev"] += 1
            a["status"] = "idle"
            save(db)
        print(json.dumps({"result": {"type": "ok"}}))
        return 0
    if group == "agent" and cmd in ("focus", "wait"):
        print(json.dumps({"result": {"type": "ok"}}))
        return 0
    print(json.dumps({"result": {"type": "ok"}}))
    return 0


if __name__ == "__main__":
    sys.exit(main())
