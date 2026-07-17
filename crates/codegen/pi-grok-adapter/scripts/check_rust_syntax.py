#!/usr/bin/env python3
"""Parse every Pi integration Rust seam with tree-sitter-rust."""
from __future__ import annotations

import argparse
import json
from pathlib import Path

from tree_sitter import Language, Parser
import tree_sitter_rust


def collect_error_nodes(node, output: list[dict[str, object]]) -> None:
    if node.is_error or node.is_missing:
        output.append(
            {
                "kind": node.type,
                "start": [node.start_point.row + 1, node.start_point.column + 1],
                "end": [node.end_point.row + 1, node.end_point.column + 1],
                "missing": node.is_missing,
                "error": node.is_error,
            }
        )
    for child in node.children:
        collect_error_nodes(child, output)


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--workspace", type=Path, required=True)
    parser.add_argument("--json-out", type=Path)
    args = parser.parse_args()

    workspace = args.workspace.resolve()
    adapter = workspace / "crates/codegen/pi-grok-adapter"
    manifest = json.loads(
        (adapter / "docs/grok_uploaded_baseline_sha256.json").read_text(encoding="utf-8")
    )

    paths: set[Path] = {
        workspace / rel
        for rel in manifest["allowedModifiedFiles"] + manifest["allowedAddedFiles"]
        if rel.endswith(".rs")
    }
    paths.update((adapter / "src").glob("*.rs"))

    language = Language(tree_sitter_rust.language())
    rust_parser = Parser(language)
    failures: dict[str, list[dict[str, object]]] = {}
    parsed: list[str] = []

    for path in sorted(paths):
        if not path.exists():
            failures[path.relative_to(workspace).as_posix()] = [
                {"kind": "missing_file", "start": [0, 0], "end": [0, 0]}
            ]
            continue
        relative = path.relative_to(workspace).as_posix()
        tree = rust_parser.parse(path.read_bytes())
        errors: list[dict[str, object]] = []
        collect_error_nodes(tree.root_node, errors)
        parsed.append(relative)
        if tree.root_node.has_error or errors:
            failures[relative] = errors

    report = {
        "schemaVersion": 1,
        "passed": not failures,
        "parsedFileCount": len(parsed),
        "parsedFiles": parsed,
        "failures": failures,
    }
    output = args.json_out or adapter / "docs/rust-syntax-verification.json"
    output.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")

    for relative in parsed:
        print(f"[{'FAIL' if relative in failures else 'PASS'}] {relative}")
    print(f"Result: {'PASS' if report['passed'] else 'FAIL'}")
    print(f"Report: {output}")
    return 0 if report["passed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
