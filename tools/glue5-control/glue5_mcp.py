#!/usr/bin/env python3
"""stdio MCP adapter for the GLUE5 local diagnostic control protocol (v1).

No shell, arbitrary-file, arbitrary-address or network-facing operations.
Only registered operations are proxied to the token-authenticated local harness.
"""
from __future__ import annotations

import json
import os
import sys
from typing import Any

from glue5_control import ControlError, request


def tool(name: str, description: str, properties: dict, required: list[str], readonly: bool) -> dict:
    return {
        "name": name,
        "description": description,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": False,
        },
        "annotations": {"readOnlyHint": readonly, "destructiveHint": False},
    }


STRING = {"type": "string"}
INT = {"type": "integer"}
NUMBER = {"type": "number"}
TOOLS = [
    tool("glue5_status", "Local harness readiness and permitted capabilities.", {}, [], True),
    tool("glue5_get_settings", "Read the harness startup settings (read-only).", {}, [], True),
    tool("glue5_get_metrics", "GPU frame/draw counters and audio metrics since the previous query.", {}, [], True),
    tool("glue5_input_press", "Press an allowlisted pad/semantic button for bounded milliseconds.",
         {"control": STRING, "duration_ms": INT}, ["control"], False),
    tool("glue5_input_axis", "Set the left or right stick for bounded milliseconds (-1..1).",
         {"stick": {"type": "string", "enum": ["left", "right"]},
          "x": NUMBER, "y": NUMBER, "duration_ms": INT}, ["stick", "x", "y"], False),
    tool("glue5_request_capture", "Request one PNG screenshot on the next rendered frame.", {}, [], False),
    tool("glue5_capture_status", "Check whether a queued capture is written on the local machine.",
         {"capture_id": INT}, ["capture_id"], True),
]

OPERATIONS = {
    "glue5_status": "status",
    "glue5_get_settings": "get_settings",
    "glue5_get_metrics": "get_metrics",
    "glue5_input_press": "input_press",
    "glue5_input_axis": "input_axis",
    "glue5_request_capture": "request_capture",
    "glue5_capture_status": "capture_status",
}


def dispatch(message: dict[str, Any], *, port: int, token: str) -> dict | None:
    """Pure JSON-RPC dispatch apart from the explicitly allowlisted protocol call."""
    if "id" not in message:
        return None
    identity = message["id"]
    method = message.get("method", "")
    params = message.get("params") or {}
    try:
        if method == "initialize":
            result = {
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {"listChanged": False}},
                "serverInfo": {"name": "glue5-diagnostic", "version": "0.1.0"},
                "instructions": "Local GLUE5 harness. Poll status before input; screenshots are local paths, not embedded images.",
            }
        elif method == "ping":
            result = {}
        elif method == "tools/list":
            result = {"tools": TOOLS}
        elif method == "tools/call":
            name = params.get("name")
            if name not in OPERATIONS:
                result = {
                    "content": [{"type": "text", "text": f"Unknown GLUE5 tool: {name}"}],
                    "isError": True,
                }
            else:
                arguments = params.get("arguments", {})
                if not isinstance(arguments, dict):
                    raise ValueError("tool arguments must be an object")
                try:
                    output = request(port, token, OPERATIONS[name], arguments)
                    result = {"content": [{"type": "text", "text": json.dumps(output, ensure_ascii=False)}]}
                except (ControlError, OSError) as exc:
                    result = {"content": [{"type": "text", "text": str(exc)}], "isError": True}
        else:
            return {"jsonrpc": "2.0", "id": identity, "error": {"code": -32601, "message": "method not found"}}
        return {"jsonrpc": "2.0", "id": identity, "result": result}
    except (TypeError, ValueError) as exc:
        return {"jsonrpc": "2.0", "id": identity, "error": {"code": -32602, "message": str(exc)}}


def main() -> None:
    port = int(os.getenv("GLUE5_DIAGNOSTIC_PORT", "49355"))
    token = os.getenv("GLUE5_DIAGNOSTIC_TOKEN", "")
    for line in sys.stdin:
        try:
            message = json.loads(line)
            if not isinstance(message, dict):
                raise ValueError("expected JSON-RPC object")
            response = dispatch(message, port=port, token=token)
        except (TypeError, ValueError) as exc:
            response = {"jsonrpc": "2.0", "id": None, "error": {"code": -32700, "message": str(exc)}}
        if response is not None:
            sys.stdout.write(json.dumps(response, separators=(",", ":")) + "\n")
            sys.stdout.flush()


if __name__ == "__main__":
    main()
