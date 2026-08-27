#!/usr/bin/env python3
"""Generate a deterministic KiCad symbol/footprint/datasheet audit package."""

from __future__ import annotations

import argparse
import csv
import html
import json
import os
import re
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from collections import defaultdict
from pathlib import Path

from review_markdown import load_review_markdown

try:
    import pymupdf
except ImportError as exc:
    raise SystemExit("pymupdf is required; run with: uv run --with pymupdf ...") from exc


REF_RE = re.compile(r"^([A-Za-z]+)(\d+)$")


def natural_ref(ref: str) -> tuple[str, int, str]:
    match = REF_RE.match(ref)
    return (match.group(1), int(match.group(2)), "") if match else (ref, 0, ref)


def resolve(base: Path, value: str) -> Path:
    path = Path(value)
    return path if path.is_absolute() else (base / path).resolve()


def load_part_configs(base: Path, config: dict) -> tuple[dict, list[str]]:
    """Resolve explicit project reference mappings to repository reviews."""
    reviews = {}
    sources = []
    for value in config.get("review_libraries", []):
        library_path = resolve(base, value)
        library = (load_review_markdown(library_path) if library_path.suffix.lower() in {".md", ".markdown"}
                   else json.loads(library_path.read_text(encoding="utf-8")))
        library_reviews = library.get("reviews", {})
        if not isinstance(library_reviews, dict):
            raise ValueError(f"review library has no reviews map: {library_path}")
        for key, item in library_reviews.items():
            item = json.loads(json.dumps(item))
            if item.get("datasheet"):
                item["datasheet"] = str(resolve(library_path.parent, item["datasheet"]))
            pinout = item.get("pinout")
            if isinstance(pinout, dict) and pinout.get("pdf"):
                pinout["pdf"] = str(resolve(library_path.parent, pinout["pdf"]))
            reviews[key] = item
        sources.append(str(library_path))
    parts = {}
    for key, item in config.get("parts", {}).items():
        item = json.loads(json.dumps(item))
        review_id = item.pop("review", None)
        if review_id:
            if review_id not in reviews:
                raise ValueError(f"unknown shared review {review_id!r} for {key}")
            inherited = json.loads(json.dumps(reviews[review_id]))
            inherited.update(item)
            item = inherited
        parts[key] = item
    return parts, sources


def svg_numbered_pin_points(root: ET.Element) -> list[tuple[str, float, float, bool]]:
    """Return KiCad pad-label centers, respecting its rotated text groups."""
    parents = {child: parent for parent in root.iter() for child in parent}
    points = []
    for element in root.iter():
        if element.tag.rsplit("}", 1)[-1] != "text":
            continue
        label = "".join(element.itertext()).strip()
        if not label.isdigit() or "x" not in element.attrib or "y" not in element.attrib:
            continue
        x = float(element.attrib["x"])
        y = float(element.attrib["y"])
        rotated = False
        parent = parents.get(element)
        while parent is not None:
            transform = parent.attrib.get("transform", "")
            match = re.search(
                r"rotate\(\s*([-+\d.eE]+)(?:[ ,]+([-+\d.eE]+)[ ,]+([-+\d.eE]+))?\s*\)",
                transform)
            if match:
                rotated = True
                if match.group(2) is not None:
                    x, y = float(match.group(2)), float(match.group(3))
                break
            parent = parents.get(parent)
        if not rotated:
            y -= float(element.attrib.get("font-size", "0")) * 0.375
        points.append((label, x, y, rotated))
    return points


def strip_kicad_svg_labels(svg: str, labels: list[str]) -> str:
    """Remove exact KiCad label nodes without string-editing SVG structure."""
    root = ET.fromstring(svg)
    labels = set(labels)

    def local_name(element: ET.Element) -> str:
        return element.tag.rsplit("}", 1)[-1]

    parents = {child: parent for parent in root.iter() for child in parent}
    for element in list(root.iter()):
        if local_name(element) != "text" or "".join(element.itertext()).strip() not in labels:
            continue
        parent = parents[element]
        parent.remove(element)
        if local_name(parent) == "g" and len(parent) == 0 and not (parent.text or "").strip():
            grandparent = parents.get(parent)
            if grandparent is not None:
                grandparent.remove(parent)

    parents = {child: parent for parent in root.iter() for child in parent}
    for element in list(root.iter()):
        if local_name(element) != "g" or element.attrib.get("class") != "stroked-text":
            continue
        description = next((child for child in element if local_name(child) == "desc"), None)
        if description is not None and "".join(description.itertext()).strip() in labels:
            parents[element].remove(element)

    ET.register_namespace("", "http://www.w3.org/2000/svg")
    ET.register_namespace("svg", "http://www.w3.org/2000/svg")
    ET.register_namespace("xlink", "http://www.w3.org/1999/xlink")
    ET.register_namespace("inkscape", "http://www.inkscape.org/namespaces/inkscape")
    return ET.tostring(root, encoding="unicode")


def sexpr_blocks(text: str, token: str):
    pos = 0
    needle = f"({token} "
    while True:
        start = text.find(needle, pos)
        if start < 0:
            return
        depth = 0
        quoted = False
        escaped = False
        end = None
        for index in range(start, len(text)):
            char = text[index]
            if quoted:
                if escaped:
                    escaped = False
                elif char == "\\":
                    escaped = True
                elif char == '"':
                    quoted = False
            else:
                if char == '"':
                    quoted = True
                elif char == "(":
                    depth += 1
                elif char == ")":
                    depth -= 1
                    if depth == 0:
                        end = index + 1
                        break
        if end is None:
            raise ValueError(f"unterminated ({token}) block")
        yield text[start:end]
        pos = end


def first(pattern: str, text: str, default: str = "") -> str:
    match = re.search(pattern, text, re.MULTILINE)
    return match.group(1) if match else default


def named_sexpr_block(text: str, token: str) -> str:
    match = re.search(rf"\({re.escape(token)}(?=\s|\))", text)
    if not match:
        raise ValueError(f"missing ({token}) block")
    start = match.start()
    depth = 0
    quoted = False
    escaped = False
    for index in range(start, len(text)):
        char = text[index]
        if quoted:
            if escaped:
                escaped = False
            elif char == "\\":
                escaped = True
            elif char == '"':
                quoted = False
        else:
            if char == '"':
                quoted = True
            elif char == "(":
                depth += 1
            elif char == ")":
                depth -= 1
                if depth == 0:
                    return text[start:index + 1]
    raise ValueError(f"unterminated ({token}) block")


def hide_audit_properties(block: str) -> str:
    result = block
    for property_block in list(sexpr_blocks(block, "property")):
        name = first(r'\(property\s+"([^"]+)"', property_block)
        if name not in {"Reference", "Value"}:
            continue
        if re.search(r'\(hide\s+yes\)', property_block):
            continue
        insertion = property_block.rfind(")")
        hidden = property_block[:insertion] + "\n\t\t\t(hide yes)" + property_block[insertion:]
        result = result.replace(property_block, hidden, 1)
    return result


def footprint_library_map(base: Path, kicad_cli: Path) -> dict[str, Path]:
    libraries = {}
    table = base / "fp-lib-table"
    if table.exists():
        text = table.read_text(encoding="utf-8")
        for name, uri in re.findall(r'\(lib\s+\(name\s+"([^"]+)"\).*?\(uri\s+"([^"]+)"\)', text):
            uri = uri.replace("${KIPRJMOD}", str(base))
            for key, value in os.environ.items():
                uri = uri.replace(f"${{{key}}}", value)
            libraries[name] = Path(uri).resolve()
    standard_root = kicad_cli.parent.parent / "share" / "kicad" / "footprints"
    libraries["__standard_root__"] = standard_root
    return libraries


def resolve_library_footprint(base: Path, footprint_id: str,
                              kicad_cli: Path) -> tuple[Path, str, Path]:
    if ":" not in footprint_id:
        raise ValueError(f"footprint identifier must be Library:Name: {footprint_id}")
    library_name, footprint_name = footprint_id.split(":", 1)
    libraries = footprint_library_map(base, kicad_cli)
    library_path = libraries.get(library_name,
                                 libraries["__standard_root__"] / f"{library_name}.pretty")
    source = library_path / f"{footprint_name}.kicad_mod"
    if not source.exists():
        raise FileNotFoundError(f"library footprint missing: {footprint_id} -> {source}")
    return library_path, footprint_name, source


def render_kicad_footprint_svg(base: Path, footprint_id: str, target: Path,
                               kicad_cli: Path, staging_dir: Path) -> dict:
    library_path, footprint_name, source = resolve_library_footprint(base, footprint_id, kicad_cli)
    source_text = source.read_text(encoding="utf-8")
    source_pads = [parse_pad_block(block) for block in sexpr_blocks(source_text, "pad")]
    export_dir = staging_dir / target.stem
    export_dir.mkdir(parents=True, exist_ok=True)
    display_library = export_dir / "display.pretty"
    display_library.mkdir(parents=True, exist_ok=True)
    display_source = display_library / source.name
    display_source.write_text(hide_audit_properties(source_text), encoding="utf-8")
    display_pads = [parse_pad_block(block) for block in sexpr_blocks(
        display_source.read_text(encoding="utf-8"), "pad")]
    if display_pads != source_pads:
        raise RuntimeError(f"{footprint_id}: display copy changed pad definitions")
    command = [str(kicad_cli), "fp", "export", "svg", "--layers", "F.Fab",
               "--sketch-pads-on-fab-layers", "--footprint", footprint_name,
               "--output", str(export_dir), str(display_library)]
    subprocess.run(command, check=True, capture_output=True, text=True)
    exported = export_dir / f"{footprint_name}.svg"
    # KiCad 10 can return from `fp export svg` before the output file becomes
    # visible to a second process on Windows.  Wait briefly for the concrete
    # artifact rather than treating that filesystem-latency race as a package
    # audit failure.
    deadline = time.monotonic() + 5.0
    while not exported.exists() and time.monotonic() < deadline:
        time.sleep(0.05)
    if not exported.exists():
        raise RuntimeError(f"{footprint_id}: KiCad CLI completed without SVG output: {exported}")
    svg = exported.read_text(encoding="utf-8")
    svg = strip_kicad_svg_labels(svg, ["REF**", footprint_name])
    if "Image generated by PCBNEW" not in svg:
        raise RuntimeError(f"{footprint_id}: KiCad-native SVG marker missing")
    pad_numbers = {pad["number"] for pad in (parse_pad_block(block) for block in sexpr_blocks(source_text, "pad"))
                   if pad and pad["number"]}
    missing_pad_numbers = sorted(
        number for number in pad_numbers
        if not re.search(rf'<(?:[A-Za-z_][\w.-]*:)?desc>{re.escape(number)}</(?:[A-Za-z_][\w.-]*:)?desc>', svg))
    if missing_pad_numbers:
        raise RuntimeError(f"{footprint_id}: native pad numbers missing from KiCad SVG: {missing_pad_numbers}")
    presentation_note = (
        "KiCad CLI export from a temporary copy with only Reference and Value hidden; "
        "pad definitions verified identical to the original library source; any KiCad-exporter "
        "label residue removed after KiCad calculated the display viewBox")
    viewbox_match = re.search(r'viewBox="([-+\d.eE]+)\s+([-+\d.eE]+)\s+([-+\d.eE]+)\s+([-+\d.eE]+)"', svg)
    if not viewbox_match:
        raise RuntimeError(f"{reference}: KiCad SVG viewBox missing")
    view_width, view_height = float(viewbox_match.group(3)), float(viewbox_match.group(4))
    display_width = 1000
    display_height = max(160, round(display_width * view_height / view_width))
    svg = re.sub(r'width="[^"]+"\s+height="[^"]+"',
                 f'width="{display_width}px" height="{display_height}px"', svg, count=1)
    target.write_text(svg, encoding="utf-8")
    return {"library_source": str(source),
            "generator": "kicad-cli fp export svg",
            "presentation_only_change": presentation_note}


def parse_pad_block(block: str) -> dict:
    head = re.match(r'\(pad\s+"([^"]*)"\s+([^\s()]+)\s+([^\s()]+)', block)
    if not head:
        return {}
    at_match = re.search(r"\(at\s+([-+\d.eE]+)\s+([-+\d.eE]+)(?:\s+([-+\d.eE]+))?\)", block)
    size_match = re.search(r"\(size\s+([-+\d.eE]+)\s+([-+\d.eE]+)\)", block)
    return {
        "number": head.group(1),
        "type": head.group(2),
        "shape": head.group(3),
        "x": float(at_match.group(1)) if at_match else 0.0,
        "y": float(at_match.group(2)) if at_match else 0.0,
        "rotation": float(at_match.group(3) or 0) if at_match else 0.0,
        "w": float(size_match.group(1)) if size_match else 0.0,
        "h": float(size_match.group(2)) if size_match else 0.0,
        "net": first(r'\(net\s+(?:\d+\s+)?"([^"]+)"', block),
        "function": first(r'\(pinfunction\s+"([^"]+)"', block),
        "layers": first(r"\(layers\s+([^\)]+)\)", block),
        "drill": first(r"\(drill(?:\s+oval)?\s+([^\)]+)\)", block),
    }


def parse_model_block(block: str) -> dict:
    path = first(r'^\(model\s+"([^"]+)"', block)
    values = {}
    for name, default in (("offset", (0.0, 0.0, 0.0)),
                          ("scale", (1.0, 1.0, 1.0)),
                          ("rotate", (0.0, 0.0, 0.0))):
        match = re.search(
            rf"\({name}\s+\(xyz\s+([-+\d.eE]+)\s+([-+\d.eE]+)\s+([-+\d.eE]+)\)\)",
            block)
        values[name] = tuple(round(float(value), 6) for value in match.groups()) if match else default
    return {"path": path.replace("\\", "/"), **values}


def comparable_pads(pads: list[dict]) -> list[tuple]:
    """Normalize only library-owned pad geometry; ignore placed nets/functions."""
    def normalized_layers(value: str, pad_type: str) -> tuple[str, ...]:
        # A footprint placed on B.Cu is mirrored by KiCad; that is placement,
        # not a changed library land pattern. Layer-token order is immaterial.
        tokens = {
            token.strip('"').replace("B.", "F.")
            for token in value.split()}
        # KiCad serializes a through-hole library pad with *.Mask, but its
        # placed PCB instance may omit that implicit mask opening. This is not
        # a footprint-geometry change.
        if pad_type in {"thru_hole", "np_thru_hole"}:
            tokens.discard("*.Mask")
        return tuple(sorted(tokens))

    return sorted((
        pad["number"], pad["type"], pad["shape"],
        round(pad["x"], 6), round(pad["y"], 6), round(pad["rotation"] % 360, 6),
        round(pad["w"], 6), round(pad["h"], 6),
        normalized_layers(pad["layers"], pad["type"]), " ".join(pad["drill"].split())
    ) for pad in pads)


def library_instance_issues(base: Path, footprint_id: str, pcb_item: dict,
                            kicad_cli: Path) -> list[str]:
    """Find a placed instance that has not been refreshed from its named library footprint."""
    try:
        _, _, source = resolve_library_footprint(base, footprint_id, kicad_cli)
    except (ValueError, FileNotFoundError):
        return ["assigned library footprint cannot be resolved"]
    source_text = source.read_text(encoding="utf-8")
    source_pads = [parse_pad_block(block) for block in sexpr_blocks(source_text, "pad")]
    source_pads = [pad for pad in source_pads if pad]
    issues = []
    if comparable_pads(source_pads) != comparable_pads(pcb_item["all_pads"]):
        issues.append("PCB pad geometry differs from the assigned library footprint")
    source_models = [parse_model_block(block) for block in sexpr_blocks(source_text, "model")]
    if source_models != pcb_item.get("models", []):
        issues.append("PCB 3D model binding/transform differs from the assigned library footprint")
    return issues


def parse_pcb(path: Path) -> dict[str, dict]:
    text = path.read_text(encoding="utf-8")
    result = {}
    for block in sexpr_blocks(text, "footprint"):
        ref = first(r'\(property\s+"Reference"\s+"([^"]+)"', block)
        if not ref:
            continue
        all_pads = []
        for pad_block in sexpr_blocks(block, "pad"):
            pad = parse_pad_block(pad_block)
            if pad:
                all_pads.append(pad)
        footprint_at = re.search(r"\(at\s+([-+\d.eE]+)\s+([-+\d.eE]+)(?:\s+([-+\d.eE]+))?\)", block)
        footprint_rotation = float(footprint_at.group(3) or 0) if footprint_at else 0.0
        for pad in all_pads:
            pad["rotation"] = (pad["rotation"] - footprint_rotation) % 360
        pads = [pad for pad in all_pads if pad["number"]]
        result[ref] = {
            "reference": ref,
            "value": first(r'\(property\s+"Value"\s+"([^"]+)"', block),
            "footprint": first(r'^\(footprint\s+"([^"]+)"', block),
            "rotation": footprint_rotation,
            "pads": pads,
            "all_pads": all_pads,
            "models": [parse_model_block(model) for model in sexpr_blocks(block, "model")],
        }
    return result


def export_netlist(config: dict, base: Path, output: Path) -> Path:
    schematic = resolve(base, config["schematic"])
    cli = Path(config.get("kicad_cli", "kicad-cli"))
    command = [str(cli), "sch", "export", "netlist", "--format", "kicadxml",
               "-o", str(output), str(schematic)]
    subprocess.run(command, check=True, capture_output=True, text=True)
    return output


def parse_netlist(path: Path) -> dict[str, dict]:
    root = ET.parse(path).getroot()
    library_pins = {}
    libparts = root.find("libparts")
    for libpart in libparts if libparts is not None else []:
        key = (libpart.attrib.get("lib", ""), libpart.attrib.get("part", ""))
        pins = []
        pin_parent = libpart.find("pins")
        for pin in pin_parent if pin_parent is not None else []:
            pins.append({
                "number": pin.attrib.get("num", ""),
                "function": pin.attrib.get("name", ""),
                "type": pin.attrib.get("type", ""),
                "net": "",
            })
        library_pins[key] = pins

    components = {}
    for comp in root.find("components"):
        lib = comp.find("libsource")
        ref = comp.attrib["ref"]
        key = (lib.attrib.get("lib", ""), lib.attrib.get("part", ""))
        components[ref] = {
            "reference": ref,
            "value": comp.findtext("value") or "",
            "footprint": comp.findtext("footprint") or "",
            "symbol": f'{lib.attrib.get("lib", "")}:{lib.attrib.get("part", "")}',
            "pins": [dict(pin) for pin in library_pins.get(key, [])],
        }
    for net in root.find("nets"):
        for node in net.findall("node"):
            ref = node.attrib["ref"]
            if ref in components:
                number = node.attrib["pin"]
                pin = next((item for item in components[ref]["pins"]
                            if item["number"] == number), None)
                if pin is None:
                    components[ref]["pins"].append({
                        "number": number,
                        "function": node.attrib.get("pinfunction", ""),
                        "type": node.attrib.get("pintype", ""),
                        "net": net.attrib["name"],
                    })
                else:
                    pin["net"] = net.attrib["name"]
    return components


def expand_refs(value: str) -> list[str]:
    return re.findall(r"[A-Za-z]+\d+", value or "")


def parse_bom(path: Path) -> tuple[dict[str, dict], list[str]]:
    refs = {}
    duplicate = []
    with path.open(encoding="utf-8-sig", newline="") as stream:
        for row in csv.DictReader(stream):
            for ref in expand_refs(row.get("Reference", "")):
                if ref in refs:
                    duplicate.append(ref)
                refs[ref] = row
    return refs, duplicate


def part_config(parts: dict, ref: str) -> dict:
    if ref in parts:
        return parts[ref]
    for key, value in parts.items():
        if key.endswith("*") and ref.startswith(key[:-1]):
            return value
    return {}


def effective_manual_status(config: dict, part: dict) -> str:
    """Apply an explicit project-level human sign-off without mutating shared reviews."""
    if part.get("status") == "PROJECT_INTERFACE":
        return "PROJECT_INTERFACE"
    return config.get("project_manual_status", part.get("status", "PENDING"))


def references_in_design(text: str) -> set[str]:
    refs = set(re.findall(r"(?<![A-Za-z0-9])[A-Za-z]+\d+(?![A-Za-z0-9])", text))
    for match in re.finditer(
        r"(?<![A-Za-z0-9])([A-Za-z]+)(\d+)\s*[-–]\s*(?:([A-Za-z]+))?(\d+)(?![A-Za-z0-9])",
        text,
    ):
        prefix_a, start, prefix_b, end = match.groups()
        if prefix_b and prefix_b != prefix_a:
            continue
        first_number, last_number = int(start), int(end)
        if first_number <= last_number:
            refs.update(f"{prefix_a}{number}" for number in range(first_number, last_number + 1))
    return refs


def component_class_key(schematic: dict | None, pcb: dict | None, cfg: dict) -> tuple:
    class_name = cfg.get("class", "")
    return (
        class_name,
        "" if class_name else (schematic or pcb or {}).get("value", ""),
        (schematic or {}).get("symbol", ""),
        (pcb or schematic or {}).get("footprint", ""),
        json.dumps(configured_evidence(cfg), sort_keys=True),
        cfg.get("status", "PENDING"),
    )


def render_pdf_evidence(pdf: Path, entries: list[dict], out_dir: Path, stem: str) -> list[dict]:
    document = pymupdf.open(pdf)
    outputs = []
    for index, entry in enumerate(entries, 1):
        page_number = int(entry["page"])
        if not 1 <= page_number <= len(document):
            raise ValueError(f"{pdf}: page {page_number} outside 1..{len(document)}")
        target = out_dir / f"{stem}-pinout-{index}-p{page_number}.png"
        page = document[page_number - 1]
        clip = None
        if entry.get("crop"):
            x0, y0, x1, y1 = entry["crop"]
            if not (0 <= x0 < x1 <= 1 and 0 <= y0 < y1 <= 1):
                raise ValueError(f"{pdf}: crop must use normalized coordinates in [0,1]")
            clip = pymupdf.Rect(page.rect.width * x0, page.rect.height * y0,
                                page.rect.width * x1, page.rect.height * y1)
        pixmap = page.get_pixmap(matrix=pymupdf.Matrix(2.4, 2.4), clip=clip, alpha=False)
        pixmap.save(target)
        outputs.append({"path": target, "title": entry.get("title", f"PDF page {page_number}"),
                        "pdf": pdf, "page": page_number})
    return outputs


def configured_evidence(cfg: dict) -> list[dict]:
    if cfg.get("pinout"):
        pinout = cfg["pinout"]
        entries = pinout.get("evidence") or [pinout]
        return [{"pdf": pinout["pdf"], "entries": entries}]
    if cfg.get("evidence"):
        converted = []
        for evidence in cfg["evidence"]:
            converted.append({"pdf": evidence["pdf"], "entries": [
                {"page": page} for page in evidence.get("pages", [])]})
        return converted
    if cfg.get("datasheet"):
        return [{"pdf": cfg["datasheet"], "entries": [
            {"page": page} for page in cfg.get("pages", [])]}]
    return []


def configured_datasheets(cfg: dict) -> list[dict]:
    """Return source PDFs independently from any configured evidence crops."""
    sources = []
    if cfg.get("pinout"):
        pinout = cfg["pinout"]
        entries = pinout.get("evidence") or [pinout]
        sources.append({"pdf": pinout["pdf"], "page": int(entries[0].get("page", 1))})
    for evidence in cfg.get("evidence", []):
        pages = evidence.get("pages", [])
        sources.append({"pdf": evidence["pdf"], "page": int(pages[0] if pages else 1)})
    if cfg.get("datasheet"):
        sources.append({"pdf": cfg["datasheet"], "page": int(cfg.get("datasheet_page", 1))})
    for source in cfg.get("review_pdfs", []):
        if isinstance(source, str):
            source = {"pdf": source}
        sources.append({"pdf": source["pdf"], "page": int(source.get("page", 1))})

    unique = []
    seen = set()
    for source in sources:
        key = (source["pdf"], source["page"])
        if key not in seen:
            seen.add(key)
            unique.append(source)
    return unique


def normalize_function(value: str) -> str:
    value = re.sub(r"_\d+$", "", value or "")
    value = value.replace("~{", "").replace("}", "")
    return re.sub(r"[^A-Za-z0-9]+", "", value).upper()


def build_symbol_datasheet_rows(schematic: dict | None, cfg: dict) -> tuple[list[list[str]], list[str]]:
    definitions = cfg.get("datasheet_pins", {})
    if not definitions:
        return [], ["structured datasheet pin definitions missing"]
    symbol_pins = {pin["number"]: pin.get("function", "") for pin in (schematic or {}).get("pins", [])}
    rows = []
    findings = []
    for number in sorted(definitions, key=lambda value: (not value.isdigit(), int(value) if value.isdigit() else value)):
        definition = definitions[number]
        if isinstance(definition, str):
            definition = {"function": definition}
        datasheet_function = definition.get("function", "")
        symbol_function = symbol_pins.get(number, "")
        symbol_display = symbol_function or definition.get("symbol_evidence", "MISSING")
        symbol_display = re.sub(rf"_{re.escape(str(number))}$", "", symbol_display)
        rows.append([number, datasheet_function or "-", symbol_display])
    extra = sorted(set(symbol_pins) - set(definitions), key=lambda value: (not value.isdigit(), int(value) if value.isdigit() else value))
    for number in extra:
        symbol_display = re.sub(rf"_{re.escape(str(number))}$", "", symbol_pins[number] or "MISSING")
        rows.append([number, "NOT DEFINED", symbol_display])
        findings.append(f"pin {number}: symbol pin is absent from datasheet definition")
    return rows, findings


def render_pin_table(pin_rows: list[list[str]], cfg: dict) -> str:
    layout = cfg.get("pin_table_layout", {})
    sides = layout.get("sides", [])
    if not sides:
        return markdown_table(pin_rows, ["Physical pin", "Datasheet function", "KiCad symbol function"])

    by_pin = {str(row[0]): row for row in pin_rows}
    headers = []
    side_cells = []
    used = set()
    for side in sides:
        headers.append(side["label"])
        cells = []
        for pin in map(str, side.get("pins", [])):
            row = by_pin.get(pin, [pin, "NOT DEFINED", "MISSING"])
            used.add(pin)
            cells.append(f'`{pin}` · {row[1]} · {row[2]}')
        side_cells.append(cells)

    rows = []
    for index in range(max((len(cells) for cells in side_cells), default=0)):
        rows.append([cells[index] if index < len(cells) else "" for cells in side_cells])

    extras = [row for pin, row in by_pin.items() if pin not in used]
    result = ["*Cell format: `physical pin` · datasheet function · KiCad symbol function.*", "",
              markdown_table(rows, headers)]
    if extras:
        result += ["", "**Pins outside the package sides**", "",
                   markdown_table(extras, ["Physical pin", "Datasheet function", "KiCad symbol function"])]
    return "\n".join(result)


def html_pin_table(pin_rows: list[list[str]], cfg: dict) -> str:
    if not pin_rows:
        return '<p class="missing">No structured manufacturer pin definition is available.</p>'
    layout = cfg.get("pin_table_layout", {})
    sides = layout.get("sides", [])
    by_pin = {str(row[0]): row for row in pin_rows}
    if not sides:
        body = "".join(
            f'<tr data-pin="{html.escape(str(row[0]))}" tabindex="0">' +
            "".join(f"<td>{html.escape(str(cell))}</td>" for cell in row) + "</tr>"
            for row in pin_rows)
        return ("<table><thead><tr><th>Physical pin</th><th>Datasheet function</th>"
                f"<th>KiCad symbol function</th></tr></thead><tbody>{body}</tbody></table>")
    groups = []
    used = set()
    for side in sides:
        rows = []
        for pin in map(str, side.get("pins", [])):
            row = by_pin.get(pin, [pin, "NOT DEFINED", "MISSING"])
            used.add(pin)
            rows.append(f'<tr data-pin="{html.escape(pin)}" tabindex="0">' +
                        "".join(f"<td>{html.escape(str(cell))}</td>" for cell in row) + "</tr>")
        groups.append(
            f'<section class="pin-side"><h4>{html.escape(side["label"])}</h4>'
            '<table><thead><tr><th>Pin</th><th>Datasheet</th><th>KiCad</th></tr></thead>'
            f'<tbody>{"".join(rows)}</tbody></table></section>')
    extras = [row for pin, row in by_pin.items() if pin not in used]
    if extras:
        rows = "".join("<tr>" + "".join(f"<td>{html.escape(str(cell))}</td>" for cell in row) + "</tr>" for row in extras)
        groups.append('<section class="pin-side"><h4>Other pads</h4><table><thead><tr>'
                      '<th>Pin</th><th>Datasheet</th><th>KiCad</th></tr></thead>'
                      f'<tbody>{rows}</tbody></table></section>')
    return f'<div class="pin-groups">{"".join(groups)}</div>'


def html_footprint_pad_table(library_source: str | Path | None, cfg: dict,
                             schematic: dict | None, pcb: dict | None) -> str:
    """Render KiCad pad geometry independently of manufacturer pin evidence."""
    if not library_source:
        return ('<p class="missing">KiCad library footprint could not be resolved; '
                'the required footprint pad table cannot be generated.</p>')
    source = Path(library_source)
    if not source.exists():
        return (f'<p class="missing">KiCad library footprint is unavailable at '
                f'{html.escape(str(source))}; the required pad table cannot be generated.</p>')

    pads = []
    for block in sexpr_blocks(source.read_text(encoding="utf-8"), "pad"):
        pad = parse_pad_block(block)
        if pad and pad.get("number"):
            pads.append(pad)
    if not pads:
        return '<p class="missing">The resolved KiCad footprint has no numbered pad primitives.</p>'

    definitions = cfg.get("datasheet_pins", {})
    symbol_pins = {str(pin["number"]): pin for pin in (schematic or {}).get("pins", [])}
    pcb_pads = {}
    for pad in (pcb or {}).get("pads", []):
        pcb_pads.setdefault(str(pad["number"]), []).append(pad)

    def pad_sort_key(pad: dict):
        number = str(pad["number"])
        return (not number.isdigit(), int(number) if number.isdigit() else number,
                pad["y"], pad["x"])

    rows = []
    for pad in sorted(pads, key=pad_sort_key):
        number = str(pad["number"])
        definition = definitions.get(number)
        if isinstance(definition, dict):
            manufacturer_function = definition.get("function", "NOT DEFINED / PENDING")
        elif isinstance(definition, str):
            manufacturer_function = definition
        else:
            manufacturer_function = "NOT DEFINED / PENDING"
        symbol_pin = symbol_pins.get(number, {})
        placed = pcb_pads.get(number, [])
        pcb_functions = sorted({pad.get("function") for pad in placed if pad.get("function")})
        pcb_nets = sorted({pad.get("net") for pad in placed if pad.get("net")})
        if pcb_functions:
            kicad_function = " / ".join(pcb_functions)
        elif symbol_pin.get("function"):
            symbol_type = symbol_pin.get("type") or "unknown"
            kicad_function = f'{symbol_pin["function"]} ({symbol_type})'
        elif pcb_nets:
            kicad_function = " / ".join(pcb_nets)
        else:
            kicad_function = "MISSING"
        rotation = f' @ {pad["rotation"]:g}deg' if pad["rotation"] else ""
        drill = pad.get("drill") or "-"
        if pad["type"] in {"thru_hole", "np_thru_hole"}:
            drill_values = str(drill).split()
            if drill_values == ["-"]:
                size = "MISSING HOLE SIZE"
            elif len(drill_values) == 1:
                size = f'Ø{drill_values[0]} mm hole'
            else:
                size = f'{" x ".join(drill_values)} mm hole'
        else:
            size = f'{pad["w"]:g} x {pad["h"]:g} mm'
        rows.append(
            f'<tr data-pin="{html.escape(number)}" tabindex="0">'
            f'<td>{html.escape(number)}</td>'
            f'<td>({pad["x"]:g}, {pad["y"]:g}) mm{rotation}</td>'
            f'<td>{html.escape(size)}</td>'
            f'<td>{html.escape(str(manufacturer_function))}</td>'
            f'<td>{html.escape(kicad_function)}</td>'
            '</tr>')
    return ('<div class="table-scroll"><table class="footprint-pad-table"><thead><tr><th>Pad</th>'
            '<th>KiCad position</th><th>Size</th><th>Manufacturer function</th>'
            '<th>KiCad function</th></tr></thead>'
            f'<tbody>{"".join(rows)}</tbody></table></div>')


def html_physical_correspondence(cfg: dict) -> str:
    rows = cfg.get("physical_correspondence", [])
    if not rows:
        return ('<p class="missing">No explicit physical-position comparison is recorded. '
                'The symbol table below does not verify footprint geometry.</p>')
    body = "".join(
        "<tr>"
        f'<td>{html.escape(str(row.get("position", "")))}</td>'
        f'<td>{html.escape(str(row.get("datasheet", "")))}</td>'
        f'<td>{html.escape(str(row.get("kicad", "")))}</td>'
        f'<td>{html.escape(str(row.get("result", "")))}</td>'
        "</tr>" for row in rows)
    return ('<table class="physical-table"><thead><tr><th>Physical position</th>'
            '<th>Manufacturer top view</th><th>KiCad library top view</th><th>Conclusion</th>'
            f'</tr></thead><tbody>{body}</tbody></table>')


def relative_web_path(origin: Path, target: Path) -> str:
    return Path(os.path.relpath(target, origin)).as_posix()


def svg_pin_hotspots(svg_path: Path) -> dict[str, tuple[float, float, float, float]]:
    """Read KiCad's own invisible numeric pad labels from the exported SVG."""
    root = ET.parse(svg_path).getroot()
    view_box = [float(value) for value in root.attrib["viewBox"].split()]
    x0, y0, width, height = view_box
    hotspots = {}
    for label, pin_x, pin_y, rotated in svg_numbered_pin_points(root):
        x = (pin_x - x0) / width
        y = (pin_y - y0) / height
        marker_width = min(0.11, 0.85 / width) if not rotated else min(0.055, 0.45 / width)
        marker_height = min(0.055, 0.45 / height) if not rotated else min(0.11, 0.85 / height)
        hotspots.setdefault(label, (x, y, marker_width, marker_height))
    return hotspots


def html_interactive_image(output: Path, image_path: Path, alt: str,
                           hotspots: dict[str, tuple], default_size=(0.035, 0.045)) -> str:
    source = html.escape(relative_web_path(output, image_path))
    markers = []
    for pin, values in hotspots.items():
        x, y = values[:2]
        width, height = values[2:] if len(values) >= 4 else default_size
        markers.append(
            f'<span class="pin-hotspot" data-pin="{html.escape(pin)}" tabindex="0" '
            f'aria-label="Pin {html.escape(pin)}" style="left:{x * 100:.4f}%;top:{y * 100:.4f}%;'
            f'width:{width * 100:.4f}%;height:{height * 100:.4f}%"></span>')
    return (f'<div class="interactive-image"><img src="{source}" alt="{html.escape(alt)}">'
            f'<div class="hotspot-layer">{"".join(markers)}</div></div>')


def render_html_report(output: Path, config: dict, detailed: list, native_evidence: dict,
                       counts: dict, failures: list[str], sync_rows: list[dict],
                       excluded_refs: list[str]) -> Path:
    cards = []
    review_statuses = ["PENDING", "REVIEWED", "PRODUCTION_VERIFIED", "SPEC_ONLY", "FAIL", "MISSING_EVIDENCE"]
    for class_name, refs, ref, s, p, cfg, fp_asset, ds_assets, pdf_sources, pin_rows, _ in detailed:
        manual_status = effective_manual_status(config, cfg)
        available_statuses = list(review_statuses)
        if manual_status not in available_statuses:
            available_statuses.insert(0, manual_status)
        status_options = "".join(
            f'<option value="{html.escape(status)}"'
            f'{" selected" if status == manual_status else ""}>{html.escape(status)}</option>'
            for status in available_statuses)
        review_key = "|".join(sorted(refs, key=natural_ref))
        footprint_hotspots = svg_pin_hotspots(fp_asset) if fp_asset else {}
        footprint_panel = (html_interactive_image(
            output, fp_asset, f"{ref} KiCad footprint", footprint_hotspots)
            if fp_asset else '<p class="missing">Missing PCB footprint.</p>')
        footprint_panel = ('<details class="evidence-item" open>'
                           '<summary>KiCad-native library footprint</summary>'
                           f'{footprint_panel}</details>')
        crops = "".join(
            f'<figure><figcaption>{html.escape(asset["title"])} · PDF page {asset["page"]}</figcaption>'
            f'<img src="{html.escape(relative_web_path(output, asset["path"]))}" '
            f'alt="{html.escape(asset["title"])}"></figure>' for asset in ds_assets)
        if crops:
            crops = re.sub(
                r'<figure><figcaption>(.*?)</figcaption>(.*?)</figure>',
                r'<details class="evidence-item" open><summary>\1</summary><figure>\2</figure></details>',
                crops, flags=re.DOTALL)
        if not crops:
            crops = ('<p class="missing">No targeted manufacturer pinout crop is configured. '
                     'Use the full source PDF below for manual review.</p>' if pdf_sources else
                     '<p class="missing">No manufacturer datasheet is available.</p>')
        embedded = []
        seen_pdf_pages = set()
        for source in pdf_sources:
            asset = source
            key = (str(source["pdf"]), source["page"])
            if key in seen_pdf_pages:
                continue
            seen_pdf_pages.add(key)
            pdf_link = relative_web_path(output, source["pdf"])
            pdf_page_link = f'{pdf_link}#page={source["page"]}'
            embedded.append(
                f'<details class="pdf"><summary>Open source PDF · page {asset["page"]}</summary>'
                f'<object data="{html.escape(pdf_page_link)}" type="application/pdf">'
                f'<p>PDF preview unavailable. <a href="{html.escape(pdf_page_link)}">Open the PDF</a>.</p>'
                f'</object></details>')
        library_source = native_evidence.get(ref, {}).get("library_source")
        pin_content = (
            '<h3>KiCad footprint pad table</h3>'
            '<p class="table-scope">This table is always generated from the resolved KiCad library '
            'footprint. KiCad function uses the first available value in this order: PCB '
            'pinfunction, symbol function/type, PCB net.</p>'
            f'{html_footprint_pad_table(library_source, cfg, s, p)}'
            '<h3>Footprint physical-position check</h3>'
            f'{html_physical_correspondence(cfg)}')
        cards.append(f'''
<article class="component" id="{html.escape(ref)}">
  <header>
    <div><p class="refs">{html.escape(", ".join(refs))}</p><h2>{html.escape(class_name)}</h2></div>
    <div class="statuses"><label class="status-label">Review
      <select class="status review-status {manual_status.lower()}" data-review-key="{html.escape(review_key)}"
        data-baseline-status="{html.escape(manual_status)}" aria-label="Review status for {html.escape(class_name)}">
        {status_options}
      </select></label></div>
  </header>
  <dl><div><dt>Symbol</dt><dd>{html.escape((s or {}).get("symbol", "missing"))}</dd></div>
  <div><dt>Footprint</dt><dd>{html.escape((p or s or {}).get("footprint", "missing"))}</dd></div>
  <div><dt>Library source</dt><dd>{html.escape(native_evidence.get(ref, {}).get("library_source", "unresolved"))}</dd></div></dl>
  <p class="note">{html.escape(cfg.get("note", "No reviewer note supplied."))}</p>
  <div class="evidence"><section>{footprint_panel}</section>
  <section><h3>Manufacturer evidence</h3>{crops}</section></div>
  <section class="pins">{pin_content}</section>
  {''.join(embedded)}
</article>''')
    interface_findings = config.get("interface_findings", {})
    report_findings = list(failures)
    report_findings.extend(
        f"{ref}: {note}" for ref, note in interface_findings.items())
    finding_items = "".join(
        f"<li>{html.escape(item)}</li>" for item in sorted(set(report_findings))) or "<li>None.</li>"
    exclusion_items = "".join(
        f"<li><strong>{html.escape(ref)}</strong>: "
        f"{html.escape(interface_findings.get(ref, 'No additional interface finding.'))}</li>"
        for ref in sorted(excluded_refs, key=natural_ref)) or "<li>None.</li>"
    sync_body = "".join(
        "<tr>"
        f'<td>{html.escape(row["reference"])}</td>'
        f'<td><code>{html.escape(row["schematic_footprint"])}</code></td>'
        f'<td><code>{html.escape(row["pcb_footprint"])}</code></td>'
        f'<td>{html.escape("; ".join(row["issues"]))}</td>'
        "</tr>" for row in sync_rows)
    sync_content = (
        '<p>No schematic/PCB footprint synchronization differences were found.</p>'
        if not sync_rows else
        '<table><thead><tr><th>Reference</th><th>Schematic assignment</th>'
        '<th>Placed PCB instance</th><th>Difference</th></tr></thead>'
        f'<tbody>{sync_body}</tbody></table>')
    document = f'''<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>{html.escape(config.get("project_name", "KiCad"))} package audit</title>
<style>
:root{{--bg:#f4f6f8;--paper:#fff;--ink:#182026;--muted:#66727c;--line:#ccd4da;--good:#176b45;--bad:#a12d2d;--wait:#805b00}}
*{{box-sizing:border-box}} body{{margin:0;background:var(--bg);color:var(--ink);font:15px/1.5 Arial,sans-serif;letter-spacing:0}}
main{{width:min(1500px,calc(100% - 32px));margin:28px auto 64px}} h1,h2,h3,h4,p{{margin-top:0}} h1{{font-size:30px;margin-bottom:8px}} h2{{font-size:21px;margin-bottom:0}} h3{{font-size:15px;margin-bottom:10px}} h4{{font-size:14px;margin-bottom:6px}}
.intro{{color:var(--muted);max-width:1000px}} .summary{{display:flex;gap:18px;flex-wrap:wrap;padding:12px 0 24px;border-bottom:1px solid var(--line)}}
.summary strong{{font-size:20px;display:block}} .component{{background:var(--paper);border:1px solid var(--line);border-radius:6px;margin:22px 0;padding:20px}}
.component>header{{display:flex;justify-content:space-between;gap:16px;align-items:flex-start}} .refs{{color:var(--muted);font-family:monospace;margin-bottom:3px}}
.statuses{{display:flex;gap:8px;flex-wrap:wrap;justify-content:flex-end}} .status-label{{display:flex;align-items:center;gap:6px;color:var(--muted);font-size:12px}} .status{{border:1px solid var(--line);padding:3px 24px 3px 8px;border-radius:4px;font-size:12px;font-weight:bold;cursor:pointer}}
.verified,.reviewed,.production_verified,.pass{{color:var(--good);border-color:#87b7a0;background:#eef8f3}} .rejected,.fail{{color:var(--bad);border-color:#d8a0a0;background:#fff2f2}} .pending,.unregistered,.missing_evidence{{color:var(--wait);border-color:#d4be7b;background:#fff9e8}}
.spec_only{{color:#345d82;border-color:#9ab7cf;background:#f0f7fc}} .review-tools{{display:flex;gap:8px;align-items:center;flex-wrap:wrap;margin:14px 0 4px}} .review-tools button{{border:1px solid var(--line);background:var(--paper);color:var(--ink);border-radius:4px;padding:6px 10px;cursor:pointer}} .review-tools button:hover{{border-color:#78909c}} .review-message{{color:var(--muted);font-size:12px}}
dl{{display:grid;gap:4px;margin:14px 0}} dl div{{display:grid;grid-template-columns:90px minmax(0,1fr)}} dt{{color:var(--muted)}} dd{{margin:0;font-family:monospace;overflow-wrap:anywhere}}
.note{{padding:10px 12px;border-left:3px solid #78909c;background:#f7f9fa}} .evidence{{display:grid;grid-template-columns:minmax(0,1fr) minmax(0,1fr);gap:16px;margin-top:18px}}
.evidence>section{{min-width:0;border-top:1px solid var(--line);padding-top:12px}} figure{{margin:8px 0 14px}} figcaption{{color:var(--muted);font-size:12px;margin-bottom:5px}} .evidence-item{{border:1px solid var(--line);margin-bottom:10px;background:#fff}} .evidence-item>summary{{padding:7px 10px;color:var(--muted);font-size:13px}} .evidence-item[open]>summary{{border-bottom:1px solid var(--line);color:var(--ink)}} .evidence-item>.interactive-image,.evidence-item>figure{{padding:10px}}
img{{display:block;max-width:100%;max-height:720px;margin:auto;object-fit:contain}} table{{width:100%;border-collapse:collapse;font-size:13px}} th,td{{border:1px solid var(--line);padding:5px 7px;text-align:left}} th{{background:#eef1f3}}
.interactive-image{{position:relative;width:max-content;max-width:100%;margin:auto}} .interactive-image img{{width:100%;height:auto}} .hotspot-layer{{position:absolute;inset:0;pointer-events:none}} .pin-hotspot{{position:absolute;transform:translate(-50%,-50%);border:2px solid transparent;background:transparent;border-radius:4px;pointer-events:auto}} .pin-hotspot.active{{border-color:#e53935;background:rgba(255,235,59,.62);box-shadow:0 0 0 2px rgba(255,255,255,.85)}} tr[data-pin]{{cursor:crosshair}} tr[data-pin].active td{{background:#fff3a8}}
.pins{{margin-top:18px}} .table-scroll{{overflow-x:auto}} .footprint-pad-table{{min-width:760px}} .subheading{{margin-top:20px}} .table-scope{{color:var(--muted);margin-bottom:9px}} .physical-table td:last-child{{font-weight:bold}} .pin-groups{{display:grid;grid-template-columns:repeat(2,minmax(0,1fr));gap:12px}} .pdf{{margin-top:14px;border:1px solid var(--line)}} summary{{cursor:pointer;padding:9px 12px;font-weight:bold}} object{{display:block;width:100%;height:780px;border:0;border-top:1px solid var(--line)}}
.findings{{background:#fff;border:1px solid var(--line);padding:18px;margin-top:24px}} code{{font-family:Consolas,monospace;font-size:12px;overflow-wrap:anywhere}} .missing{{color:var(--bad)}}
@media(max-width:850px){{main{{width:min(100% - 16px,1500px);margin-top:12px}}.component{{padding:14px}}.component>header,.evidence{{display:block}}.statuses{{justify-content:flex-start;margin-top:10px}}.pin-groups{{grid-template-columns:1fr}}object{{height:600px}}}}
</style></head><body><main>
<h1>{html.escape(config.get("project_name", "KiCad"))} Component Package Audit</h1>
<p class="intro">Human review report. KiCad-native library geometry and manufacturer evidence are shown side by side before PCB layout.</p>
<div class="review-tools"><button type="button" id="export-review">Export review-status JSON</button><button type="button" id="reset-review">Restore configured statuses</button><span class="review-message" id="review-message">Clickable statuses are stored only in this browser; they do not modify the audit config or manifest.</span></div>
<section class="summary"><div><strong>{html.escape(config.get('project_manual_status', 'NOT SIGNED'))}</strong>project manual sign-off</div><div><strong>{counts['audited']}</strong>references audited</div><div><strong>{counts['classes']}</strong>component classes</div><div><strong>{counts['paired']}</strong>paired evidence classes</div><div><strong>{len(set(report_findings))}</strong>independent findings</div></section>
<section class="findings sync"><h2>Schematic / PCB footprint consistency</h2>
<p class="intro">Only differences are listed. Matching names are also checked for stale embedded pad geometry and 3D model bindings.</p>
{sync_content}</section>
{''.join(cards)}
<section class="findings"><h2>Follow-up findings</h2><ul>{finding_items}</ul><h2>Excluded project interfaces</h2><ul>{exclusion_items}</ul></section>
</main><script>
const reportStorageKey = 'kicad-package-audit:' + {json.dumps(config.get("project_name", "KiCad"))} + ':';
const reviewSelects = Array.from(document.querySelectorAll('.review-status'));
const statusClasses = ['pending', 'reviewed', 'production_verified', 'spec_only', 'fail', 'missing_evidence', 'pass', 'project_interface'];
const storageGet = key => {{ try {{ return localStorage.getItem(key); }} catch (_) {{ return null; }} }};
const storageSet = (key, value) => {{ try {{ localStorage.setItem(key, value); return true; }} catch (_) {{ return false; }} }};
const storageRemove = key => {{ try {{ localStorage.removeItem(key); }} catch (_) {{}} }};
const applyReviewStatus = (select, value) => {{
  if (!Array.from(select.options).some(option => option.value === value)) return;
  select.value = value;
  statusClasses.forEach(name => select.classList.remove(name));
  select.classList.add(value.toLowerCase());
}};
reviewSelects.forEach(select => {{
  const saved = storageGet(reportStorageKey + select.dataset.reviewKey);
  applyReviewStatus(select, saved || select.dataset.baselineStatus);
  select.addEventListener('change', () => {{
    applyReviewStatus(select, select.value);
    const persisted = storageSet(reportStorageKey + select.dataset.reviewKey, select.value);
    document.getElementById('review-message').textContent = persisted
      ? 'Review status saved in this browser. Export JSON to retain or share the decisions.'
      : 'Status changed for this session; browser storage is unavailable. Export JSON before closing.';
  }});
}});
document.getElementById('reset-review').addEventListener('click', () => {{
  reviewSelects.forEach(select => {{
    storageRemove(reportStorageKey + select.dataset.reviewKey);
    applyReviewStatus(select, select.dataset.baselineStatus);
  }});
  document.getElementById('review-message').textContent = 'Configured statuses restored.';
}});
document.getElementById('export-review').addEventListener('click', () => {{
  const statuses = Object.fromEntries(reviewSelects.map(select => [select.dataset.reviewKey, select.value]));
  const payload = JSON.stringify({{
    project: {json.dumps(config.get("project_name", "KiCad"))},
    exported_at: new Date().toISOString(),
    source: 'components.html browser review',
    statuses
  }}, null, 2);
  const link = document.createElement('a');
  link.href = URL.createObjectURL(new Blob([payload], {{type: 'application/json'}}));
  link.download = 'component-review-status.json';
  link.click();
  setTimeout(() => URL.revokeObjectURL(link.href), 0);
}});
document.querySelectorAll('.component').forEach(card => {{
  const activate = pin => {{
    card.querySelectorAll('[data-pin]').forEach(node =>
      node.classList.toggle('active', node.dataset.pin === pin));
  }};
  const clear = () => card.querySelectorAll('[data-pin].active').forEach(node => node.classList.remove('active'));
  card.querySelectorAll('tr[data-pin], .pin-hotspot[data-pin]').forEach(node => {{
    node.addEventListener('mouseenter', () => activate(node.dataset.pin));
    node.addEventListener('focus', () => activate(node.dataset.pin));
    node.addEventListener('mouseleave', clear);
    node.addEventListener('blur', clear);
  }});
}});
</script></body></html>'''
    target = output / "components.html"
    target.write_text(document, encoding="utf-8")
    return target


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--config", required=True)
    args = parser.parse_args()
    config_path = Path(args.config).resolve()
    base = config_path.parent
    config = json.loads(config_path.read_text(encoding="utf-8"))
    parts, review_sources = load_part_configs(base, config)
    schematic = resolve(base, config["schematic"])
    pcb_path = resolve(base, config["pcb"]) if config.get("pcb") else None
    bom_path = resolve(base, config["bom"])
    design_paths = [resolve(base, p) for p in config.get("design_documents", [])]
    output = resolve(base, config.get("output_dir", "audit"))
    if output.name.lower() != "audit":
        raise ValueError(f"output_dir must resolve to an audit directory: {output}")
    assets = output / "assets"
    native_staging = output / "native-staging"
    output.mkdir(parents=True, exist_ok=True)
    assets.mkdir(parents=True, exist_ok=True)
    if native_staging.exists():
        shutil.rmtree(native_staging)
    native_staging.mkdir(parents=True, exist_ok=True)
    for stale_asset in assets.iterdir():
        if stale_asset.is_file() and stale_asset.suffix.lower() in {".png", ".svg"}:
            stale_asset.unlink()

    netlist = export_netlist(config, base, output / "schematic-netlist.xml")
    sch = parse_netlist(netlist)
    pcb = ({ref: item for ref, item in parse_pcb(pcb_path).items()
            if item["pads"] and not ref.startswith("G")}
           if pcb_path and pcb_path.exists() else {})
    bom, duplicate_bom = parse_bom(bom_path)
    design_text = "\n".join(p.read_text(encoding="utf-8") for p in design_paths)
    design_refs = references_in_design(design_text)
    all_refs = sorted(set(sch) | set(pcb) | set(bom), key=natural_ref)
    configured_exclusions = set(config.get("exclude_refs", []))

    coverage_rows = []
    sync_rows = []
    excluded_refs = []
    failures = []
    evidence_cache = {}
    classes = defaultdict(list)
    for ref in all_refs:
        s = sch.get(ref)
        p = pcb.get(ref)
        b = bom.get(ref)
        cfg = part_config(parts, ref)
        if ref in configured_exclusions or cfg.get("status") == "PROJECT_INTERFACE":
            excluded_refs.append(ref)
            continue
        status = effective_manual_status(config, cfg)
        pin_count = len({pin["number"] for pin in (s or {}).get("pins", [])})
        required_pair = pin_count > 2
        mismatches = []
        sync_issues = []
        if not s: mismatches.append("missing schematic")
        if not b: mismatches.append("missing BOM")
        if s and not p:
            sync_issues.append("missing PCB instance")
        elif p and not s:
            sync_issues.append("missing schematic symbol")
        elif s and p and s["footprint"] != p["footprint"]:
            sync_issues.append("schematic/PCB footprint identifiers differ")
        elif s and p and s["footprint"]:
            sync_issues.extend(library_instance_issues(
                base, s["footprint"], p, Path(config.get("kicad_cli", "kicad-cli"))))
        if sync_issues:
            mismatches.extend(sync_issues)
            sync_rows.append({
                "reference": ref,
                "schematic_footprint": (s or {}).get("footprint", "missing"),
                "pcb_footprint": (p or {}).get("footprint", "missing"),
                "issues": sync_issues,
            })
        if s and b and b.get("Value", "") != s["value"]: mismatches.append("BOM/schematic value differs")
        if s and b and s["footprint"] not in b.get("Package", ""): mismatches.append("BOM package does not name schematic footprint")
        normalize_net = lambda value: value.replace("{slash}", "/")
        sch_pin_nets = {(x["number"], normalize_net(x["net"])) for x in (s or {}).get("pins", [])}
        # An unconnected PCB-only pad is absent from the KiCad XML netlist.
        # Do not report a false schematic/PCB mismatch for it (notably QFN and
        # SOIC exposed pads that are intentionally left unconnected).
        pcb_pin_nets = {(x["number"], normalize_net(x["net"]))
                        for x in (p or {}).get("pads", [])
                        if x["net"] or x["function"]}
        if s and p and sch_pin_nets != pcb_pin_nets: mismatches.append("schematic/PCB pin-net sets differ")
        # A maintained design authority commonly specifies passive families by
        # class/value rather than repeating every reference designator.  A
        # reference match is therefore optional; configs may supply an exact
        # `design_evidence` string when a particular contract must be checked.
        design_evidence = cfg.get("design_evidence")
        design_hit = (str(design_evidence) in design_text
                      if design_evidence else bool(design_paths))
        if not design_hit:
            mismatches.append(
                "design evidence text absent" if design_evidence
                else "no design document configured")
        if required_pair and status in {"PENDING", "MISSING_EVIDENCE", "FAIL"}: failures.append(f"{ref}: {status}")
        if (required_pair and not configured_evidence(cfg)
                and status not in {"PROJECT_INTERFACE", "MISSING_EVIDENCE", "PRODUCTION_VERIFIED", "PASS"}):
            mismatches.append("no datasheet mapping")
        if mismatches: failures.extend(f"{ref}: {m}" for m in mismatches)
        coverage_rows.append([ref, (s or p or {}).get("value", ""), str(pin_count), "yes" if required_pair else "no", status, "yes" if design_hit else "no", "; ".join(mismatches) or "-"])
        classes[component_class_key(s, p, cfg)].append(ref)

    detailed = []
    native_evidence = {}
    for class_index, (_, refs) in enumerate(sorted(classes.items(), key=lambda item: natural_ref(item[1][0])), 1):
        representative = refs[0]
        s = sch.get(representative)
        p = pcb.get(representative)
        cfg = part_config(parts, representative)
        pin_count = len({pin["number"] for pin in (s or {}).get("pins", [])})
        if pin_count <= 2:
            continue
        class_name = cfg.get("class") or f'{(s or p or {}).get("value", "unknown")} / {(p or s or {}).get("footprint", "unknown")}'
        stem = re.sub(r"[^A-Za-z0-9_-]", "_", f"class-{class_index}-{class_name}")[:96]
        footprint_asset = assets / f"{stem}-footprint.svg"
        footprint_id = (s or p or {}).get("footprint", "")
        if footprint_id:
            native_evidence[representative] = render_kicad_footprint_svg(
                base, footprint_id, footprint_asset,
                Path(config.get("kicad_cli", "kicad-cli")), native_staging)
        datasheet_assets = []
        for evidence_index, evidence in enumerate(configured_evidence(cfg), 1):
            pdf = resolve(base, evidence["pdf"])
            if pdf.exists():
                entries = evidence.get("entries", [])
                key = (str(pdf), json.dumps(entries, sort_keys=True))
                if key not in evidence_cache:
                    evidence_cache[key] = render_pdf_evidence(pdf, entries, assets, f"{stem}-e{evidence_index}")
                datasheet_assets.extend(evidence_cache[key])
            else:
                failures.append(f"{class_name}: datasheet file missing: {pdf}")
        datasheet_sources = []
        for source in configured_datasheets(cfg):
            pdf = resolve(base, source["pdf"])
            if pdf.exists():
                datasheet_sources.append({"pdf": pdf, "page": source["page"]})
            else:
                failures.append(f"{class_name}: datasheet file missing: {pdf}")
        pin_rows, evidence_findings = build_symbol_datasheet_rows(s, cfg)
        if pin_count > 2 and not pin_rows and effective_manual_status(config, cfg) != "PASS":
            failures.extend(f"{representative}: {finding}" for finding in evidence_findings)
        detailed.append((class_name, refs, representative, s, p, cfg,
                         footprint_asset if footprint_asset.exists() else None,
                         datasheet_assets, datasheet_sources, pin_rows, evidence_findings))

    unique_failures = sorted(set(failures))
    for obsolete_report in (output / "README.md", output / "index.html"):
        if obsolete_report.exists():
            obsolete_report.unlink()
    report = render_html_report(
        output, config, detailed, native_evidence,
        {"audited": len(all_refs) - len(excluded_refs), "classes": len(classes),
         "paired": len(detailed)},
        unique_failures, sync_rows, excluded_refs)
    project_manual_status = config.get("project_manual_status", "NOT SIGNED")
    blocking = [] if project_manual_status == "PASS" else unique_failures
    manifest = {"report": str(report), "report_format": "html",
                "project_manual_status": project_manual_status,
                "source_references": len(all_refs),
                "audited_references": len(all_refs) - len(excluded_refs),
                "excluded_references": excluded_refs, "classes": len(classes),
                "paired_classes": len(detailed),
                "coverage_checks": len(coverage_rows),
                "schematic_pcb_footprint_differences": sync_rows,
                "review_sources": review_sources,
                "footprint_evidence": native_evidence,
                "follow_up_findings": unique_failures,
                "blocking": blocking}
    (output / "manifest.json").write_text(json.dumps(manifest, indent=2), encoding="utf-8")
    print(json.dumps(manifest, indent=2))
    return 1 if blocking else 0


if __name__ == "__main__":
    sys.exit(main())
