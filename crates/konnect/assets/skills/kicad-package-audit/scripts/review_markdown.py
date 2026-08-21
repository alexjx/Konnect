#!/usr/bin/env python3
"""Read and write the human-maintained Markdown component review registry."""

from __future__ import annotations

import argparse
import html
import json
import re
from pathlib import Path


SECTION_RE = re.compile(r"^## `([^`]+)`\s*$", re.MULTILINE)


def _escape(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, (dict, list)):
        value = json.dumps(value, ensure_ascii=False, separators=(",", ":"))
    return str(value).replace("\\", "\\\\").replace("|", "\\|").replace("\n", "<br>")


def _split_row(line: str) -> list[str]:
    cells, current, escaped = [], [], False
    for char in line.strip().strip("|"):
        if escaped:
            current.append(char)
            escaped = False
        elif char == "\\":
            escaped = True
        elif char == "|":
            cells.append(html.unescape("".join(current).strip()).replace("<br>", "\n"))
            current = []
        else:
            current.append(char)
    cells.append(html.unescape("".join(current).strip()).replace("<br>", "\n"))
    return cells


def _table(block: str, heading: str) -> list[list[str]]:
    match = re.search(rf"^### {re.escape(heading)}\s*$", block, re.MULTILINE)
    if not match:
        return []
    lines = block[match.end():].lstrip().splitlines()
    rows = []
    for line in lines:
        if line.startswith("### ") or line.startswith("## "):
            break
        if not line.lstrip().startswith("|"):
            if rows and line.strip():
                break
            continue
        cells = _split_row(line)
        if all(re.fullmatch(r":?-+:?", cell) for cell in cells):
            continue
        rows.append(cells)
    return rows[1:] if rows else []


def load_review_markdown(path: Path) -> dict:
    """Parse the canonical Markdown registry into the audit library shape."""
    text = path.read_text(encoding="utf-8")
    matches = list(SECTION_RE.finditer(text))
    reviews = {}
    for index, match in enumerate(matches):
        review_id = match.group(1)
        block = text[match.end():matches[index + 1].start() if index + 1 < len(matches) else len(text)]
        contract = {row[0]: row[1] for row in _table(block, "Contract") if len(row) >= 2}
        item = {}
        field_map = {
            "Class": "class", "Status": "status", "Symbol": "symbol",
            "Footprint": "footprint", "Datasheet": "datasheet",
            "Reuse decision": "reuse_status",
            "Note": "note",
        }
        for source, target in field_map.items():
            if contract.get(source):
                item[target] = contract[source]

        evidence_rows = _table(block, "Pinout evidence")
        if evidence_rows:
            pdf = contract.get("Pinout PDF") or contract.get("Datasheet")
            evidence = []
            for row in evidence_rows:
                if len(row) < 4:
                    raise ValueError(f"{path}: {review_id}: malformed pinout evidence row")
                evidence.append({"title": row[0], "page": int(row[1]),
                                 "crop": json.loads(row[2]), **({"note": row[3]} if row[3] else {})})
            item["pinout"] = {"pdf": pdf, "evidence": evidence}

        pin_rows = _table(block, "Datasheet pins")
        if pin_rows:
            pins = {}
            for row in pin_rows:
                pin, function = row[0], row[1]
                aliases = [value.strip() for value in row[2].split(",") if value.strip()] if len(row) > 2 else []
                pins[pin] = {"function": function, "symbol_functions": aliases} if aliases else function
            item["datasheet_pins"] = pins

        physical_rows = _table(block, "Physical correspondence")
        if physical_rows:
            item["physical_correspondence"] = [
                {"position": row[0], "datasheet": row[1], "kicad": row[2], "result": row[3]}
                for row in physical_rows
            ]

        side_rows = _table(block, "Pin table layout")
        if side_rows:
            item["pin_table_layout"] = {"sides": [
                {"label": row[0], "pins": [pin.strip() for pin in row[1].split(",") if pin.strip()]}
                for row in side_rows
            ]}
        reviews[review_id] = item
    if not reviews:
        raise ValueError(f"Markdown review library has no `## `review-id`` sections: {path}")
    return {"schema_version": 1, "scope": "repository", "reviews": reviews}


def render_review_markdown(library: dict) -> str:
    lines = [
        "# Shared KiCad component registry", "",
        "这是工作区内可复用 KiCad 元件契约的唯一人工维护清单，封装审计工具直接读取本文件。", "",
        "本清单维护 IC、封装或极性敏感的分立器件、使用非标准 footprint 的电感/晶振，以及需要准确专用封装的非常见连接器。普通电阻、电容、LED、常见连接器、排针、pogo 和所有裸焊盘接口由各项目自行管理，不进入共享清单。", "",
    ]
    for review_id, item in library.get("reviews", {}).items():
        lines += [f"## `{review_id}`", "", "### Contract", "", "| Field | Value |", "| --- | --- |"]
        pinout = item.get("pinout") if isinstance(item.get("pinout"), dict) else {}
        fields = [
            ("Class", item.get("class", "")), ("Status", item.get("status", "PENDING")),
            ("Reuse decision", item.get("reuse_status", "")),
            ("Symbol", item.get("symbol", "")), ("Footprint", item.get("footprint", "")),
            ("Datasheet", item.get("datasheet", "")), ("Pinout PDF", pinout.get("pdf", "")),
            ("Note", item.get("note", "")),
        ]
        lines += [f"| {_escape(key)} | {_escape(value)} |" for key, value in fields if value]

        entries = pinout.get("evidence") or ([pinout] if pinout and pinout.get("page") else [])
        if entries:
            lines += ["", "### Pinout evidence", "", "| Title | Page | Crop `[x0,y0,x1,y1]` | Note |", "| --- | ---: | --- | --- |"]
            for entry in entries:
                lines.append(f"| {_escape(entry.get('title', 'Pinout evidence'))} | {entry.get('page', 1)} | {_escape(entry.get('crop', []))} | {_escape(entry.get('note', ''))} |")

        pins = item.get("datasheet_pins", {})
        if pins:
            lines += ["", "### Datasheet pins", "", "| Pin | Manufacturer function | Accepted symbol aliases |", "| --- | --- | --- |"]
            for pin, definition in pins.items():
                if isinstance(definition, dict):
                    function = definition.get("function", "")
                    aliases = ", ".join(definition.get("symbol_functions", []))
                else:
                    function, aliases = definition, ""
                lines.append(f"| {_escape(pin)} | {_escape(function)} | {_escape(aliases)} |")

        physical = item.get("physical_correspondence", [])
        if physical:
            lines += ["", "### Physical correspondence", "", "| Position | Datasheet | KiCad | Result |", "| --- | --- | --- | --- |"]
            for row in physical:
                lines.append(f"| {_escape(row.get('position'))} | {_escape(row.get('datasheet'))} | {_escape(row.get('kicad'))} | {_escape(row.get('result'))} |")

        sides = item.get("pin_table_layout", {}).get("sides", [])
        if sides:
            lines += ["", "### Pin table layout", "", "| Side | Pins in top-view traversal order |", "| --- | --- |"]
            for side in sides:
                lines.append(f"| {_escape(side.get('label'))} | {_escape(', '.join(side.get('pins', [])))} |")
        lines.append("")
    return "\n".join(lines).rstrip() + "\n"


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--from-json", type=Path, required=True)
    parser.add_argument("--to-markdown", type=Path, required=True)
    args = parser.parse_args()
    library = json.loads(args.from_json.read_text(encoding="utf-8"))
    args.to_markdown.write_text(render_review_markdown(library), encoding="utf-8")


if __name__ == "__main__":
    main()
