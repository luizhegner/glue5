#!/usr/bin/env python3
"""Summarise GLUE5 performance/audio log lines and optionally compare two captures."""

from __future__ import annotations

import argparse
import json
import math
import re
from pathlib import Path
from statistics import mean
from typing import Any

RUNTIME_RE = re.compile(
    r"game runtime: build (?P<build>\S+) variant (?P<variant>\S+) sha256 (?P<sha>[0-9a-fA-F]{64})"
)
GPU_RE = re.compile(
    r"gpu: (?P<fps>[0-9.]+) fps .*?, (?P<draws>\d+) draws/frame, "
    r"(?P<dispatches>\d+) dispatches, (?P<passes>[0-9.]+) passes/frame .*?"
    r"draws waiting for one (?P<pending>\d+), critical lighting waits (?P<lighting>\d+) "
    r"\((?P<lighting_total>[0-9.]+) ms total, longest (?P<lighting_max>[0-9.]+)\), "
    r"dispatches waiting for one (?P<late_compute>\d+)"
)
BC_RE = re.compile(
    r"BC decode (?P<uploads>\d+) uploads, (?P<blocks>\d+) blocks "
    r"\((?P<mib>[0-9.]+) MiB compressed; max (?P<threads>\d+) inner threads; "
    r"(?P<etc2>\d+) blocks re-encoded ETC2\), held flushes "
    r"(?P<wait>[0-9.]+) ms \(longest (?P<wait_max>[0-9.]+), "
    r"(?P<command_thread>\d+) decoded on the command thread\)"
)
SRV_RE = re.compile(
    r"gpu srv: (?P<requests>\d+) view requests, (?P<hits>\d+) cached "
    r"\((?P<hit_pct>[0-9.]+)%\), (?P<resolves>\d+) DXGI format resolves"
)
AUDIO_RE = re.compile(
    r"audio: frames waited for the game's audio update (?P<avg_wait>[0-9.]+) ms each "
    r"\((?P<waits>\d+) waits, longest (?P<longest>[0-9.]+) ms\), "
    r"the device played (?P<silence>[0-9.]+) ms of silence "
    r"\((?P<underruns>\d+) reads short\)"
)
VOICE_RE = re.compile(
    r"audio voices: (?P<active>\d+)/(?P<limit>\d+) physical "
    r"\(target (?P<target>\d+), reserve (?P<reserve>\d+)\), "
    r"mixing budget (?P<budget>\d+), cpu limited (?P<cpu>true|false)"
)
PASS_END_RE = re.compile(r"passes/frame \(ended by (?P<ends>[^)]*)\)")
PASS_CAUSE_RE = re.compile(r"^(?P<name>.+) (?P<per_frame>[0-9.]+)$")
THERMAL_RE = re.compile(
    r"thermal: level (?P<level>\d+) (?P<state>.+?) cap (?P<cap>\d+)$"
)
HEADROOM_RE = re.compile(r"headroom (?P<headroom>-?[0-9.]+)")


def floats(records: list[dict[str, Any]], key: str) -> list[float]:
    return [float(r[key]) for r in records if key in r]


def ints(records: list[dict[str, Any]], key: str) -> list[int]:
    return [int(r[key]) for r in records if key in r]


def safe_mean(values: list[float | int]) -> float | None:
    return mean(values) if values else None


def safe_max(values: list[float | int]) -> float | int | None:
    return max(values) if values else None


def parse(path: Path) -> dict[str, Any]:
    runtime: dict[str, str] | None = None
    gpu: list[dict[str, Any]] = []
    bc: list[dict[str, Any]] = []
    srv: list[dict[str, Any]] = []
    audio: list[dict[str, Any]] = []
    voices: list[dict[str, Any]] = []
    pass_ends: list[dict[str, float]] = []
    thermal: list[dict[str, Any]] = []

    for line in path.read_text(encoding="utf-8", errors="replace").splitlines():
        if m := RUNTIME_RE.search(line):
            runtime = m.groupdict()
        if m := GPU_RE.search(line):
            gpu.append(m.groupdict())
            causes: dict[str, float] = {}
            if e := PASS_END_RE.search(line):
                for item in e.group("ends").split(","):
                    item = item.strip()
                    if not item:
                        continue
                    if cause := PASS_CAUSE_RE.match(item):
                        causes[cause.group("name")] = float(cause.group("per_frame"))
            pass_ends.append(causes)
        if m := BC_RE.search(line):
            bc.append(m.groupdict())
        if m := SRV_RE.search(line):
            srv.append(m.groupdict())
        if m := AUDIO_RE.search(line):
            audio.append(m.groupdict())
        if m := VOICE_RE.search(line):
            voices.append(m.groupdict())
        if m := THERMAL_RE.search(line):
            record: dict[str, Any] = m.groupdict()
            if h := HEADROOM_RE.search(record["state"]):
                record["headroom"] = h.group("headroom")
            thermal.append(record)

    requests = sum(ints(srv, "requests"))
    hits = sum(ints(srv, "hits"))
    voice_ratios = [
        int(v["active"]) / int(v["limit"])
        for v in voices
        if int(v["limit"]) > 0
    ]
    pass_names = sorted({name for sample in pass_ends for name in sample})
    pass_end_means = {
        name: mean(sample.get(name, 0.0) for sample in pass_ends)
        for name in pass_names
    }
    pass_end_means = dict(sorted(pass_end_means.items(), key=lambda item: item[1], reverse=True))

    summary: dict[str, Any] = {
        "file": str(path),
        "runtime": runtime,
        "samples": {
            "gpu": len(gpu),
            "bc": len(bc),
            "srv": len(srv),
            "audio": len(audio),
            "voices": len(voices),
            "thermal": len(thermal),
        },
        "gpu": {
            "fps_mean": safe_mean(floats(gpu, "fps")),
            "draws_per_frame_mean": safe_mean(floats(gpu, "draws")),
            "dispatches_per_frame_mean": safe_mean(floats(gpu, "dispatches")),
            "passes_per_frame_mean": safe_mean(floats(gpu, "passes")),
            "pending_pipeline_skips_total": sum(ints(gpu, "pending")),
            "critical_lighting_waits_total": sum(ints(gpu, "lighting")),
            "critical_lighting_wait_ms_total": sum(floats(gpu, "lighting_total")),
            "critical_lighting_wait_ms_max": safe_max(floats(gpu, "lighting_max")),
            "late_compute_total": sum(ints(gpu, "late_compute")),
            "pass_end_causes_per_frame_mean": pass_end_means,
        },
        "bc": {
            "uploads_total": sum(ints(bc, "uploads")),
            "blocks_total": sum(ints(bc, "blocks")),
            "source_mib_total": sum(floats(bc, "mib")),
            "etc2_blocks_total": sum(ints(bc, "etc2")),
            "wait_ms_total": sum(floats(bc, "wait")),
            "wait_ms_max": safe_max(floats(bc, "wait_max")),
            "max_inner_threads": safe_max(ints(bc, "threads")),
            "command_thread_decodes_total": sum(ints(bc, "command_thread")),
        },
        "srv": {
            "requests_total": requests,
            "cache_hits_total": hits,
            "cache_hit_percent_weighted": (100.0 * hits / requests) if requests else None,
            "format_resolves_total": sum(ints(srv, "resolves")),
        },
        "audio": {
            "update_wait_ms_mean": safe_mean(floats(audio, "avg_wait")),
            "update_wait_ms_max": safe_max(floats(audio, "longest")),
            "update_waits_total": sum(ints(audio, "waits")),
            "silence_ms_total": sum(floats(audio, "silence")),
            "underruns_total": sum(ints(audio, "underruns")),
        },
        "voices": {
            "active_to_limit_peak": safe_max(voice_ratios),
            "cpu_limited_samples": sum(1 for v in voices if v["cpu"] == "true"),
            "physical_active_peak": safe_max(ints(voices, "active")),
            "physical_limit_max": safe_max(ints(voices, "limit")),
        },
        "thermal": {
            "max_level": safe_max(ints(thermal, "level")),
            "min_cap": min(ints(thermal, "cap")) if thermal else None,
            "min_headroom": min(floats(thermal, "headroom")) if any("headroom" in t for t in thermal) else None,
            "last_state": thermal[-1]["state"] if thermal else None,
        },
    }
    return summary


COMPARE_KEYS = [
    ("gpu.fps_mean", "FPS mean"),
    ("gpu.passes_per_frame_mean", "passes/frame"),
    ("gpu.pending_pipeline_skips_total", "pipeline skips"),
    ("gpu.critical_lighting_wait_ms_total", "lighting wait ms"),
    ("bc.wait_ms_total", "BC wait ms"),
    ("srv.cache_hit_percent_weighted", "SRV cache hit %"),
    ("audio.underruns_total", "audio underruns"),
    ("audio.silence_ms_total", "audio silence ms"),
    ("thermal.max_level", "thermal max level"),
    ("thermal.min_cap", "thermal minimum cap"),
]


def at(summary: dict[str, Any], dotted: str) -> Any:
    cur: Any = summary
    for part in dotted.split("."):
        if not isinstance(cur, dict):
            return None
        cur = cur.get(part)
    return cur


def fmt(value: Any) -> str:
    if value is None:
        return "n/a"
    if isinstance(value, float):
        return f"{value:.2f}"
    return str(value)


def text_summary(summary: dict[str, Any]) -> str:
    rt = summary.get("runtime") or {}
    lines = [
        f"file: {summary['file']}",
        f"runtime: {rt.get('build', '?')} / {rt.get('variant', '?')} / {rt.get('sha', '?')[:12]}",
        f"samples: {summary['samples']}",
        f"fps mean: {fmt(at(summary, 'gpu.fps_mean'))}",
        f"passes/frame mean: {fmt(at(summary, 'gpu.passes_per_frame_mean'))}",
        "pass ends: " + (
            ", ".join(
                f"{name} {value:.2f}/frame"
                for name, value in list((at(summary, "gpu.pass_end_causes_per_frame_mean") or {}).items())[:6]
            ) or "n/a"
        ),
        f"pipeline skips: {fmt(at(summary, 'gpu.pending_pipeline_skips_total'))}",
        f"critical lighting wait: {fmt(at(summary, 'gpu.critical_lighting_wait_ms_total'))} ms total; "
        f"{fmt(at(summary, 'gpu.critical_lighting_wait_ms_max'))} ms max",
        f"BC: {fmt(at(summary, 'bc.uploads_total'))} uploads, "
        f"{fmt(at(summary, 'bc.source_mib_total'))} MiB input, "
        f"{fmt(at(summary, 'bc.wait_ms_total'))} ms wait, "
        f"max {fmt(at(summary, 'bc.max_inner_threads'))} inner threads",
        f"SRV: {fmt(at(summary, 'srv.requests_total'))} requests, "
        f"{fmt(at(summary, 'srv.cache_hit_percent_weighted'))}% cached, "
        f"{fmt(at(summary, 'srv.format_resolves_total'))} format resolves",
        f"audio: {fmt(at(summary, 'audio.underruns_total'))} underruns, "
        f"{fmt(at(summary, 'audio.silence_ms_total'))} ms silence",
        f"voices: peak {fmt(at(summary, 'voices.active_to_limit_peak'))} of physical limit, "
        f"cpu-limited samples {fmt(at(summary, 'voices.cpu_limited_samples'))}",
        f"thermal: max level {fmt(at(summary, 'thermal.max_level'))}, "
        f"min cap {fmt(at(summary, 'thermal.min_cap'))}, "
        f"min headroom {fmt(at(summary, 'thermal.min_headroom'))}, "
        f"last {fmt(at(summary, 'thermal.last_state'))}",
    ]
    return "\n".join(lines)


def comparison(a: dict[str, Any], b: dict[str, Any]) -> str:
    lines = ["comparison (second - first):"]
    for key, label in COMPARE_KEYS:
        av, bv = at(a, key), at(b, key)
        if av is None or bv is None:
            lines.append(f"  {label}: n/a")
            continue
        delta = float(bv) - float(av)
        pct = (delta / float(av) * 100.0) if float(av) != 0 else math.nan
        pct_text = "n/a" if math.isnan(pct) else f"{pct:+.1f}%"
        lines.append(f"  {label}: {fmt(av)} -> {fmt(bv)} ({delta:+.2f}, {pct_text})")
    return "\n".join(lines)


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("logs", type=Path, nargs="+", help="GLUE5 logcat/game log captures")
    ap.add_argument("--json", action="store_true", help="print machine-readable JSON")
    args = ap.parse_args()

    summaries = [parse(p) for p in args.logs]
    if args.json:
        print(json.dumps(summaries if len(summaries) > 1 else summaries[0], indent=2))
        return 0

    for i, summary in enumerate(summaries):
        if i:
            print()
        print(text_summary(summary))
    if len(summaries) == 2:
        print()
        print(comparison(summaries[0], summaries[1]))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
