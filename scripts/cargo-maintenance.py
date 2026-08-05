#!/usr/bin/env python3
"""Periodically remove only stale Cargo incremental cache directories."""

from __future__ import annotations

import argparse
import os
import shutil
import sys
import time
from pathlib import Path

try:
    import fcntl
except ImportError:  # pragma: no cover - the supported dev hosts are POSIX
    fcntl = None


def positive_int(value: str) -> int:
    parsed = int(value)
    if parsed < 1:
        raise argparse.ArgumentTypeError("must be a positive integer")
    return parsed


def disk_free_bytes(path: Path) -> int:
    probe = path
    while not probe.exists() and probe != probe.parent:
        probe = probe.parent
    return shutil.disk_usage(probe).free


def incremental_roots(target: Path) -> list[Path]:
    roots: list[Path] = []
    for path in target.rglob("incremental"):
        if path.is_symlink() or not path.is_dir():
            continue
        try:
            if path.resolve().is_relative_to(target.resolve()):
                roots.append(path)
        except (OSError, ValueError):
            continue
    return roots


def stale_entries(target: Path, cutoff: float) -> list[Path]:
    candidates: list[Path] = []
    for root in incremental_roots(target):
        try:
            entries = root.iterdir()
        except OSError:
            continue
        for entry in entries:
            if entry.is_symlink() or not entry.is_dir():
                continue
            try:
                if entry.stat().st_mtime <= cutoff:
                    candidates.append(entry)
            except OSError:
                continue
    return sorted(candidates, key=lambda path: path.stat().st_mtime)


def write_marker(marker: Path, timestamp: float) -> None:
    temporary = marker.with_name(f"{marker.name}.{os.getpid()}.tmp")
    temporary.write_text(f"{timestamp:.6f}\n", encoding="ascii")
    os.replace(temporary, marker)


def should_run(marker: Path, now: float, interval_seconds: int) -> bool:
    try:
        last_run = float(marker.read_text(encoding="ascii").strip())
    except (FileNotFoundError, ValueError, OSError):
        return True
    return now - last_run >= interval_seconds


def run(args: argparse.Namespace) -> int:
    target = Path(os.path.abspath(os.path.expanduser(args.target_dir)))
    if not target.is_dir():
        return 0

    lock_path = target / ".cargo-maintenance.lock"
    marker = target / ".cargo-maintenance.last"
    try:
        lock_file = lock_path.open("a+")
    except OSError:
        return 0

    try:
        if fcntl is not None:
            try:
                fcntl.flock(lock_file.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError:
                return 0

        now = time.time()
        if not args.force and not should_run(marker, now, args.interval_hours * 3600):
            return 0

        available = disk_free_bytes(target)
        age_days = args.stale_days
        if available < args.soft_free_gib * 1024**3:
            age_days = min(age_days, 1)
        cutoff = now - age_days * 24 * 60 * 60
        candidates = stale_entries(target, cutoff)[: args.max_dirs]

        removed = 0
        for entry in candidates:
            if args.dry_run:
                print(f"cargo maintenance: would remove {entry}")
                removed += 1
                continue
            try:
                shutil.rmtree(entry)
                removed += 1
            except OSError as error:
                print(f"cargo maintenance: skipped {entry}: {error}", file=sys.stderr)

        if not args.dry_run:
            try:
                write_marker(marker, now)
            except OSError:
                pass
        if removed:
            action = "would remove" if args.dry_run else "removed"
            print(f"cargo maintenance: {action} {removed} stale incremental cache(s)")
        return 0
    finally:
        lock_file.close()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target-dir", required=True)
    parser.add_argument("--interval-hours", type=positive_int, default=6)
    parser.add_argument("--stale-days", type=positive_int, default=7)
    parser.add_argument("--soft-free-gib", type=positive_int, default=40)
    parser.add_argument("--max-dirs", type=positive_int, default=8)
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    return run(parser.parse_args())


if __name__ == "__main__":
    raise SystemExit(main())
