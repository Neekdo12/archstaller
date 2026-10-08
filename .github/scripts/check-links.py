#!/usr/bin/env python3
"""Fails when a relative link in a Markdown file points to a file that does not exist.

Checks `[text](path)` links (ignoring http(s), mailto and pure #anchors) in every tracked *.md file.
Anchors are not verified; only that the file or directory exists. Links inside fenced code blocks and
inline code are ignored.
"""
import re
import subprocess
import sys
from pathlib import Path

root = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
files = subprocess.check_output(["git", "ls-files", "-z", "--", "*.md"], cwd=root).decode().split("\0")
link = re.compile(r"(?<!\!)\[[^\]\n]*\]\(([^)\s]+)(?:\s+\"[^\"]*\")?\)")
bad = []
for name in filter(None, files):
    path = root / name
    text = path.read_text(encoding="utf-8", errors="replace")
    text = re.sub(r"^```.*?^```", "", text, flags=re.S | re.M)
    text = re.sub(r"`[^`\n]*`", "", text)
    for m in link.finditer(text):
        target = m.group(1)
        if re.match(r"^(https?:|mailto:|#)", target):
            continue
        rel = target.split("#", 1)[0]
        if not rel:
            continue
        dest = (root / rel.lstrip("/")) if rel.startswith("/") else (path.parent / rel)
        if not dest.exists():
            bad.append(f"{name}: broken link -> {target}")
if bad:
    print("\n".join(bad))
    sys.exit(1)
print(f"checked {len(files) - 1} Markdown files, all relative links resolve")
