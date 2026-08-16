#!/usr/bin/env python3
"""Fail closed on malformed or incomplete DISP CycloneDX SBOM output."""

from __future__ import annotations

import argparse
import json
from pathlib import Path
import re
import uuid


SHA256 = re.compile(r"^[0-9a-f]{64}$")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("sbom", type=Path)
    parser.add_argument("--require-native", action="store_true")
    arguments = parser.parse_args()
    bom = json.loads(arguments.sbom.read_text(encoding="utf-8"))
    if bom.get("bomFormat") != "CycloneDX" or bom.get("specVersion") != "1.6":
        raise ValueError("SBOM is not CycloneDX 1.6")
    serial_number = bom.get("serialNumber")
    if not isinstance(serial_number, str) or not serial_number.startswith("urn:uuid:"):
        raise ValueError("SBOM lacks a GitHub-attestable UUID serial number")
    try:
        uuid.UUID(serial_number.removeprefix("urn:uuid:"))
    except ValueError as error:
        raise ValueError("SBOM has a malformed UUID serial number") from error
    components = bom.get("components")
    dependencies = bom.get("dependencies")
    if not isinstance(components, list) or not components:
        raise ValueError("SBOM has no components")
    if not isinstance(dependencies, list):
        raise ValueError("SBOM has no dependency graph")
    references = [item.get("bom-ref") for item in components]
    if any(not isinstance(item, str) or not item for item in references):
        raise ValueError("SBOM component lacks bom-ref")
    if len(references) != len(set(references)):
        raise ValueError("SBOM component references are not unique")
    known = set(references)
    for dependency in dependencies:
        if dependency.get("ref") not in known:
            raise ValueError("dependency source is absent from components")
        if any(item not in known for item in dependency.get("dependsOn", [])):
            raise ValueError("dependency target is absent from components")
    cargo_count = sum(item.startswith("pkg:cargo/") for item in references)
    native_count = sum(
        any(prop.get("name") == "disp:native:source" for prop in item.get("properties", []))
        for item in components
    )
    if cargo_count == 0:
        raise ValueError("SBOM has no Cargo components")
    if arguments.require_native and native_count == 0:
        raise ValueError("artifact SBOM has no resolved native components")
    for component in components:
        for item in component.get("hashes", []):
            if item.get("alg") != "SHA-256" or not SHA256.fullmatch(item.get("content", "")):
                raise ValueError("SBOM contains a malformed component hash")
    encoded = arguments.sbom.read_text(encoding="utf-8")
    if "\\\\Users\\" in encoded or '"null"' in encoded:
        raise ValueError("SBOM leaks a host path or stringified null")
    print(
        f"verified {arguments.sbom}: {cargo_count} Cargo components, "
        f"{native_count} resolved native components"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
