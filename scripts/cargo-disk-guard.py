#!/usr/bin/env python3
"""Run Cargo with a free-space floor and stop it before the floor is crossed."""

from __future__ import annotations

import argparse
import os
import shutil
import signal
import subprocess
import sys
import time

LOW_SPACE_EXIT = 74


def existing_path(raw_path: str) -> str:
    path = os.path.abspath(os.path.expanduser(raw_path))
    while not os.path.exists(path) and path != os.path.dirname(path):
        path = os.path.dirname(path)
    return path


def filesystem_paths(raw_paths: list[str]) -> list[str]:
    paths: list[str] = []
    devices: set[int] = set()
    for raw_path in raw_paths:
        path = existing_path(raw_path)
        device = os.stat(path).st_dev
        if device not in devices:
            devices.add(device)
            paths.append(path)
    return paths


def free_bytes(path: str) -> int:
    return shutil.disk_usage(path).free


def low_space(paths: list[str], floor: int) -> tuple[str, int] | None:
    for path in paths:
        available = free_bytes(path)
        if available < floor:
            return path, available
    return None


def stop_process(process: subprocess.Popen[object]) -> None:
    try:
        if os.name == "posix":
            os.killpg(process.pid, signal.SIGTERM)
        else:
            process.terminate()
    except ProcessLookupError:
        return

    try:
        process.wait(timeout=5)
    except subprocess.TimeoutExpired:
        try:
            if os.name == "posix":
                os.killpg(process.pid, signal.SIGKILL)
            else:
                process.kill()
        except ProcessLookupError:
            pass
        process.wait()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--path",
        action="append",
        required=True,
        help="Filesystem containing Cargo output or caches; repeat for more filesystems",
    )
    parser.add_argument("--min-free-gib", required=True, type=int)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()

    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("a command is required after --")
    if args.min_free_gib < 1:
        parser.error("--min-free-gib must be at least 1")

    floor = args.min_free_gib * 1024**3
    paths = filesystem_paths(args.path)
    blocked = low_space(paths, floor)
    if blocked:
        path, available = blocked
        print(
            f"error: Cargo blocked; only {available / 1024**3:.1f} GiB is free "
            f"(safety floor: {args.min_free_gib} GiB) on {path}",
            file=sys.stderr,
        )
        print("free space first, or clean only generated Cargo output", file=sys.stderr)
        return LOW_SPACE_EXIT

    process = subprocess.Popen(command, start_new_session=(os.name == "posix"))
    try:
        while process.poll() is None:
            blocked = low_space(paths, floor)
            if blocked:
                path, available = blocked
                print(
                    f"error: Cargo stopped; {available / 1024**3:.1f} GiB remains "
                    f"on {path}, below the {args.min_free_gib} GiB free-space floor",
                    file=sys.stderr,
                )
                stop_process(process)
                return LOW_SPACE_EXIT
            time.sleep(0.5)
    except KeyboardInterrupt:
        stop_process(process)
        return 130

    result = process.returncode
    blocked = low_space(paths, floor)
    if result == 0 and blocked:
        path, available = blocked
        print(
            f"error: Cargo finished below the {args.min_free_gib} GiB free-space floor "
            f"({available / 1024**3:.1f} GiB on {path})",
            file=sys.stderr,
        )
        return LOW_SPACE_EXIT
    return result


if __name__ == "__main__":
    raise SystemExit(main())
