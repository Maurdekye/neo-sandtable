"""Sanitize a recorded CLI stream into a committable fixture.

Usage: python sanitize.py <in.jsonl> <out.jsonl>

Drops machine-specific and account-specific fields (paths, socket names, ids that identify the
machine), blanks reasoning signatures, and keeps everything the stream parsers read. Never commit a
raw recording: run it through this script and read the result first.
"""
import json
import sys

DROP = {"cwd", "memory_paths", "messaging_socket_path", "powershell_path", "request_id",
        "uuid", "capabilities", "plugins", "agents", "output_style", "tool_use_meta",
        "wire_tool_inputs", "analytics_disabled", "product_feedback_disabled",
        "overageDisabledReason", "subagent_stats", "modelUsage", "projectsDirectory"}


def clean(v):
    if isinstance(v, dict):
        out = {}
        for k, x in v.items():
            if k in DROP:
                continue
            if k == "signature":
                x = "SIG"
            out[k] = clean(x)
        return out
    if isinstance(v, list):
        return [clean(x) for x in v]
    if isinstance(v, str):
        return v.replace("\\\\", "/").replace("\\", "/")
    return v


with open(sys.argv[1], encoding="utf-8") as src, open(sys.argv[2], "w", encoding="utf-8") as dst:
    for line in src:
        line = line.strip()
        if not line:
            continue
        try:
            dst.write(json.dumps(clean(json.loads(line)), ensure_ascii=False) + "\n")
        except json.JSONDecodeError:
            continue
