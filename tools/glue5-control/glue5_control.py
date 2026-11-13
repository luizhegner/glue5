#!/usr/bin/env python3
"""GLUE5 diagnostic client. Local-only, explicit protocol v1, stdlib only."""
from __future__ import annotations

import argparse
import json
import os
import socket
import sys
import time
from typing import Any


class ControlError(Exception):
    pass


def request(port: int, token: str, op: str, args: dict[str, Any] | None = None) -> dict[str, Any]:
    """One authenticated request, one bounded JSON-line response."""
    if not (1 <= port <= 65535):
        raise ControlError("port out of range")
    if len(token) < 16:
        raise ControlError("GLUE5_DIAGNOSTIC_TOKEN is missing or too short")
    payload = {"v": 1, "id": "cli", "token": token, "op": op, "args": args or {}}
    data = (json.dumps(payload, separators=(",", ":")) + "\n").encode("utf-8")
    if len(data) > 8192:
        raise ControlError("request exceeds protocol limit")
    with socket.create_connection(("127.0.0.1", port), timeout=3) as sock:
        sock.settimeout(5)
        sock.sendall(data)
        buf = bytearray()
        while not buf.endswith(b"\n"):
            fragment = sock.recv(8192)
            if not fragment:
                raise ControlError("server closed the connection")
            buf.extend(fragment)
            if len(buf) > 1_048_576:
                raise ControlError("response is too large")
    try:
        response = json.loads(buf)
    except ValueError as exc:
        raise ControlError("invalid JSON response") from exc
    if response.get("v") != 1 or response.get("id") not in ("cli", None):
        raise ControlError("unexpected protocol version or request id")
    if not response.get("ok"):
        raise ControlError(str(response.get("error", "unknown error")))
    return response["result"]


def run(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--port", type=int, default=49355, help="GLUE5 localhost diagnostic port")
    sub = parser.add_subparsers(dest="command", required=True)
    sub.add_parser("status")
    sub.add_parser("settings")
    sub.add_parser("metrics")
    press = sub.add_parser("press", help="whitelisted semantic button or pad button")
    press.add_argument("control")
    press.add_argument("--ms", type=int, default=100)
    axis = sub.add_parser("axis")
    axis.add_argument("stick", choices=("left", "right"))
    axis.add_argument("x", type=float)
    axis.add_argument("y", type=float)
    axis.add_argument("--ms", type=int, default=500)
    capture = sub.add_parser("capture", help="request next rendered frame and optionally poll")
    capture.add_argument("--wait", type=float, default=0, metavar="SECONDS")
    shot = sub.add_parser("capture-status")
    shot.add_argument("capture_id", type=int)
    opts = parser.parse_args(argv)
    token = os.environ.get("GLUE5_DIAGNOSTIC_TOKEN", "")
    try:
        if opts.command in ("status", "settings", "metrics"):
            op = {"status": "status", "settings": "get_settings", "metrics": "get_metrics"}[opts.command]
            result = request(opts.port, token, op)
        elif opts.command == "press":
            result = request(opts.port, token, "input_press", {
                "control": opts.control, "duration_ms": opts.ms,
            })
        elif opts.command == "axis":
            result = request(opts.port, token, "input_axis", {
                "stick": opts.stick, "x": opts.x, "y": opts.y, "duration_ms": opts.ms,
            })
        elif opts.command == "capture-status":
            result = request(opts.port, token, "capture_status", {"capture_id": opts.capture_id})
        else:
            result = request(opts.port, token, "request_capture")
            if opts.wait > 0:
                deadline = time.monotonic() + min(opts.wait, 60.0)
                while time.monotonic() < deadline:
                    status = request(opts.port, token, "capture_status", {
                        "capture_id": result["capture_id"],
                    })
                    if status["ready"]:
                        result = status
                        break
                    time.sleep(0.2)
        print(json.dumps(result, indent=2, ensure_ascii=False))
        return 0
    except (ControlError, OSError, ValueError) as exc:
        print(f"glue5-control: {exc}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(run())
