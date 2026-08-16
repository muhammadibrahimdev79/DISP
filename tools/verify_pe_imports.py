#!/usr/bin/env python3
"""Fail when a Windows PE artifact statically imports a forbidden library."""

from __future__ import annotations

import argparse
import pathlib
import struct


def u16(data: bytes, offset: int) -> int:
    return struct.unpack_from("<H", data, offset)[0]


def u32(data: bytes, offset: int) -> int:
    return struct.unpack_from("<I", data, offset)[0]


def imports(path: pathlib.Path) -> list[str]:
    data = path.read_bytes()
    if len(data) < 0x40 or data[:2] != b"MZ":
        raise ValueError("artifact is not a bounded DOS/PE image")
    pe = u32(data, 0x3C)
    if pe + 24 > len(data) or data[pe : pe + 4] != b"PE\0\0":
        raise ValueError("artifact has no valid PE signature")
    sections_count = u16(data, pe + 6)
    optional_size = u16(data, pe + 20)
    optional = pe + 24
    if optional + optional_size > len(data):
        raise ValueError("PE optional header exceeds the artifact")
    magic = u16(data, optional)
    directory = optional + (112 if magic == 0x20B else 96 if magic == 0x10B else -1)
    if directory < optional or directory + 16 > optional + optional_size:
        raise ValueError("PE data directory is malformed")
    import_rva = u32(data, directory + 8)
    import_size = u32(data, directory + 12)
    section_table = optional + optional_size
    if section_table + sections_count * 40 > len(data):
        raise ValueError("PE section table exceeds the artifact")

    sections: list[tuple[int, int, int, int]] = []
    for index in range(sections_count):
        section = section_table + index * 40
        virtual_size = u32(data, section + 8)
        virtual_address = u32(data, section + 12)
        raw_size = u32(data, section + 16)
        raw_offset = u32(data, section + 20)
        sections.append((virtual_address, max(virtual_size, raw_size), raw_offset, raw_size))

    def offset_for(rva: int, length: int = 1) -> int:
        for virtual_address, extent, raw_offset, raw_size in sections:
            relative = rva - virtual_address
            if 0 <= relative and relative + length <= extent and relative + length <= raw_size:
                offset = raw_offset + relative
                if offset + length <= len(data):
                    return offset
        raise ValueError(f"PE RVA 0x{rva:x} is not backed by file data")

    if import_rva == 0:
        return []
    if import_size == 0 or import_size > len(data):
        raise ValueError("PE import directory has an invalid bound")
    descriptor = offset_for(import_rva, 20)
    end = descriptor + import_size
    result: list[str] = []
    while descriptor + 20 <= len(data) and descriptor < end:
        fields = struct.unpack_from("<IIIII", data, descriptor)
        if fields == (0, 0, 0, 0, 0):
            return result
        name_offset = offset_for(fields[3])
        terminator = data.find(b"\0", name_offset, min(len(data), name_offset + 4096))
        if terminator < 0:
            raise ValueError("PE import name is not bounded by NUL")
        result.append(data[name_offset:terminator].decode("ascii", "strict"))
        descriptor += 20
    raise ValueError("PE import directory lacks a bounded terminator")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("artifact", type=pathlib.Path)
    parser.add_argument("--forbid", action="append", default=[])
    arguments = parser.parse_args()
    names = imports(arguments.artifact)
    forbidden = {name.casefold() for name in arguments.forbid}
    matches = [name for name in names if name.casefold() in forbidden]
    if matches:
        raise SystemExit(f"forbidden static PE imports: {', '.join(matches)}")
    print("PASS PE imports: " + ", ".join(names))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
