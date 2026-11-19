#!/usr/bin/env python3
"""Shared helpers for tests/e2e acceptance + latency probes.

Pure test-side code: it talks to notchd over the SAME NDJSON socket the CLI uses
(`NOTCH_SOCK`) and to herdr with the same read-only commands the daemon uses.
No project source is imported or modified.

Usage:
  python3 e2e_lib.py probe --sock S --state D --cwd C --task T --dod D \
      --marker M --out FILE --budget-ms N [--no-submit] [--expect marker|any]
  python3 e2e_lib.py req --sock S --json '{"cmd":"snapshot"}'
"""
from __future__ import annotations

import argparse
import json
import os
import socket
import subprocess
import sys
import time

MS = lambda: int(time.time() * 1000)


def now_iso() -> str:
    return time.strftime("%Y-%m-%dT%H:%M:%S%z")


def sock_request(sock_path: str, obj: dict, timeout: float = 180.0) -> dict:
    """One NDJSON request/response over a fresh connection (like the CLI)."""
    s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    s.settimeout(timeout)
    try:
        s.connect(sock_path)
        buf = b""
        while b"\n" not in buf and len(buf) < 16384:
            chunk = s.recv(16384)
            if not chunk:
                break
            buf += chunk
        s.sendall((json.dumps(obj, separators=(",", ":")) + "\n").encode())
        resp = b""
        while b"\n" not in resp and len(resp) < 8 * 1024 * 1024:
            chunk = s.recv(65536)
            if not chunk:
                break
            resp += chunk
        line = resp.split(b"\n", 1)[0]
        if not line:
            raise RuntimeError("empty socket response")
        return json.loads(line)
    finally:
        s.close()


def snapshot(sock_path: str) -> dict:
    v = sock_request(sock_path, {"cmd": "snapshot"})
    snap = v.get("snapshot", v)
    return snap


def session_of(snap: dict, sid: str) -> dict | None:
    return next((s for s in snap.get("sessions", []) if s.get("id") == sid), None)


def herdr_agents() -> list:
    try:
        out = subprocess.run(
            ["herdr", "agent", "list"], capture_output=True, text=True, timeout=15
        ).stdout
        d = json.loads(out)
        return d.get("result", {}).get("agents", [])
    except Exception:
        return []


def herdr_panes() -> set:
    return {a.get("pane_id") for a in herdr_agents() if a.get("pane_id")}


def tail(sid: str, lines: int = 200, pane: str = "") -> str:
    """Pane tail via the same read path the daemon uses (lowercased agent name)."""
    for target in (sid.lower(), pane, sid):
        if not target:
            continue
        try:
            out = subprocess.run(
                ["herdr", "agent", "read", target, "--lines", str(lines)],
                capture_output=True,
                text=True,
                timeout=12,
            )
            if out.stdout and out.stdout.strip():
                return out.stdout
        except Exception:
            continue
    return ""


def events(state_dir: str) -> list:
    try:
        with open(os.path.join(state_dir, "events.jsonl")) as f:
            return [json.loads(l) for l in f if l.strip()]
    except Exception:
        return []


def decisions_for(state_dir: str, sid: str) -> list:
    return [
        e for e in events(state_dir)
        if e.get("kind") == "decision" and e.get("session") == sid
    ]


def wait_until(pred, deadline_ms: int, interval: float = 0.2):
    """Return (ok, value, t_ms) where t_ms is the observation time."""
    while MS() < deadline_ms:
        v = pred()
        if v:
            return True, v, MS()
        time.sleep(interval)
    return False, None, MS()


def emit(out_path: str, rec: dict) -> None:
    line = json.dumps(rec, separators=(",", ":"))
    print(line, flush=True)
    if out_path:
        with open(out_path, "a") as f:
            f.write(line + "\n")


def probe(args) -> int:
    sock, state, sid = args.sock, args.state, None
    baseline = herdr_panes()
    t_call = MS()
    ack = sock_request(
        sock,
        {
            "cmd": "submit_task",
            "task": args.task,
            "cwd": args.cwd,
            "definitionOfDone": args.dod,
        },
        timeout=args.timeout,
    )
    t_ack = MS()
    sid = ack.get("sessionId", "")
    emit(args.out, {"hop": "a_submit_to_ack", "sid": sid, "ms": t_ack - t_call,
                    "t": t_ack, "ok": bool(ack.get("ok")), "ack": ack})
    if not sid or not ack.get("ok"):
        emit(args.out, {"hop": "fatal", "ms": 0, "t": t_ack, "error": ack})
        return 2

    deadline = t_ack + args.budget_ms
    pane_id = None

    def got_pane():
        nonlocal pane_id
        try:
            s = session_of(snapshot(sock), sid)
        except Exception:
            return None
        p = (s or {}).get("paneId")
        if p and p not in baseline:
            pane_id = p
            return p
        # also accept a brand-new herdr pane owned by this session name
        for a in herdr_agents():
            if a.get("pane_id") not in baseline and sid in (a.get("agent") or ""):
                pane_id = a.get("pane_id")
                return pane_id
        return None

    ok, v, t_pane = wait_until(got_pane, deadline)
    emit(args.out, {"hop": "b_ack_to_pane", "sid": sid, "ms": t_pane - t_ack,
                    "t": t_pane, "ok": ok, "pane": pane_id})

    ok_w, v_w, t_work = wait_until(
        lambda: True if (session_of(snapshot(sock), sid) or {}).get("status") == "working" else None,
        deadline,
    )
    emit(args.out, {"hop": "c_pane_to_working", "sid": sid, "ms": t_work - t_pane,
                    "t": t_work, "ok": ok_w, "pane": pane_id})

    def idle_seen():
        s = session_of(snapshot(sock), sid)
        if not s:
            return None
        st = s.get("status")
        if st in ("idle", "done", "failed"):
            return st
        return None

    ok_i, st_i, t_idle_poll = wait_until(idle_seen, deadline)

    def idle_signal_ms():
        for e in events(state):
            if e.get("kind") == "idle_signal" and e.get("session") == sid:
                return e.get("ts")
        return None

    t_idle = idle_signal_ms() or t_idle_poll
    emit(args.out, {"hop": "d_working_to_idle", "sid": sid, "ms": t_idle - t_work,
                    "t": t_idle, "ok": ok_i or t_idle != t_idle_poll, "status": st_i,
                    "observed_ms": t_idle_poll, "signal": "idle_signal" if t_idle != t_idle_poll else "snapshot"})

    ok_e, decs, t_dec_poll = wait_until(
        lambda: decisions_for(state, sid) or None, deadline
    )
    t_dec = (decs[0].get("ts") or t_dec_poll) if decs else t_dec_poll
    emit(args.out, {"hop": "e_idle_to_decision", "sid": sid, "ms": t_dec - t_idle,
                    "t": t_dec, "ok": ok_e,
                    "decisions": [d.get("detail") for d in (decs or [])]})

    marker = args.marker
    def prompt_delivered():
        if not decisions_for(state, sid):
            return None
        t = tail(sid, pane=pane_id)
        if args.expect == "marker":
            return marker if marker in t else None
        return t[-200:] or None

    ok_f, found, t_prompt = wait_until(prompt_delivered, deadline, interval=0.5)
    emit(args.out, {"hop": "f_decision_to_prompt", "sid": sid, "ms": t_prompt - t_dec,
                    "t": t_prompt, "ok": ok_f, "expect": args.expect,
                    "marker": marker, "tail_head": (found or "")[:300]})

    final = snapshot(sock)
    emit(args.out, {"hop": "final", "sid": sid, "t": MS(),
                    "session": session_of(final, sid),
                    "decisions": decisions_for(state, sid),
                    "escalations": [e for e in final.get("escalations", [])
                                    if e.get("session") == sid],
                    "baseline_panes": sorted(p for p in baseline if p),
                    "pane": pane_id})
    return 0 if all([ok, ok_w, ok_i]) else 1


def main() -> int:
    p = argparse.ArgumentParser()
    sub = p.add_subparsers(dest="cmd", required=True)
    r = sub.add_parser("req")
    r.add_argument("--sock", required=True)
    r.add_argument("--json", required=True)
    q = sub.add_parser("probe")
    q.add_argument("--sock", required=True)
    q.add_argument("--state", required=True)
    q.add_argument("--cwd", required=True)
    q.add_argument("--task", required=True)
    q.add_argument("--dod", default="")
    q.add_argument("--marker", default="")
    q.add_argument("--out", default="")
    q.add_argument("--budget-ms", type=int, default=180000)
    q.add_argument("--timeout", type=float, default=180.0)
    q.add_argument("--expect", choices=["marker", "any"], default="any")
    a = p.parse_args()
    if a.cmd == "req":
        print(json.dumps(sock_request(a.sock, json.loads(a.json)), separators=(",", ":")))
        return 0
    return probe(a)


if __name__ == "__main__":
    sys.exit(main())
