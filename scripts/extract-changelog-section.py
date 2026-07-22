#!/usr/bin/env python3
"""Extract one Keep-a-Changelog section from CHANGELOG.MD for GitHub Releases.

Usage:
  extract-changelog-section.py <version> [changelog_path] [out_path]

version may be "0.0.8" or "v0.0.8". Writes the matching "## [x.y.z] ..." section
(including heading) to out_path (default stdout). Falls back to a short link
when the section is missing so release publish never hard-fails.
"""

from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path


def normalize_version(raw: str) -> str:
    v = raw.strip()
    if v.startswith("v") or v.startswith("V"):
        v = v[1:]
    if not v:
        raise SystemExit("version is empty")
    return v


def extract_section(text: str, version: str) -> str | None:
    pat = rf"(?ms)^(## \[{re.escape(version)}\][^\n]*\n.*?)(?=^## \[|\Z)"
    m = re.search(pat, text)
    if not m:
        return None
    body = m.group(1).rstrip()
    # Drop trailing Keep-a-Changelog section separators.
    while body.endswith("\n---") or body.endswith("\n***"):
        body = body[:-4].rstrip()
    return body + "\n"


def fallback_notes(version: str, repo: str) -> str:
    return (
        f"## [{version}]\n\n"
        f"No matching section found in `CHANGELOG.MD` for this tag.\n\n"
        f"See the full changelog: "
        f"https://github.com/{repo}/blob/main/CHANGELOG.MD\n"
    )


def main() -> int:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument("version", help="Release version (e.g. 0.0.8 or v0.0.8)")
    p.add_argument(
        "changelog",
        nargs="?",
        default="CHANGELOG.MD",
        help="Path to changelog (default: CHANGELOG.MD)",
    )
    p.add_argument(
        "-o",
        "--output",
        default="-",
        help="Output path (default: stdout)",
    )
    p.add_argument(
        "--repo",
        default="Dwsy/grok-pi",
        help="GitHub repo for fallback link (default: Dwsy/grok-pi)",
    )
    p.add_argument(
        "--strict",
        action="store_true",
        help="Exit non-zero when the section is missing",
    )
    args = p.parse_args()

    version = normalize_version(args.version)
    path = Path(args.changelog)
    if not path.is_file():
        print(f"error: changelog not found: {path}", file=sys.stderr)
        return 2

    text = path.read_text(encoding="utf-8")
    body = extract_section(text, version)
    if body is None:
        print(
            f"warning: no CHANGELOG section for [{version}]",
            file=sys.stderr,
        )
        if args.strict:
            return 1
        body = fallback_notes(version, args.repo)
    else:
        print(
            f"extracted CHANGELOG section [{version}] ({len(body)} bytes)",
            file=sys.stderr,
        )

    if args.output == "-":
        sys.stdout.write(body)
    else:
        out = Path(args.output)
        out.parent.mkdir(parents=True, exist_ok=True)
        out.write_text(body, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
