#!/usr/bin/env python3
"""Generate a deterministic CycloneDX 1.6 SBOM from locked Cargo metadata.

On Linux, repeated --artifact arguments add the actual dynamically linked
native libraries reported by ldd, including file hashes and distribution
package versions when dpkg owns the resolved file.
"""

from __future__ import annotations

import argparse
import datetime
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import struct
import subprocess
import sys
import tomllib
from urllib.parse import quote


GENERATOR_VERSION = "1"
SHA256 = re.compile(r"^[0-9a-f]{64}$")


def run(arguments: list[str]) -> str:
    result = subprocess.run(arguments, check=True, text=True, capture_output=True)
    return result.stdout


def locked_checksums(lockfile: Path) -> dict[tuple[str, str], str]:
    data = tomllib.loads(lockfile.read_text(encoding="utf-8"))
    checksums: dict[tuple[str, str], str] = {}
    for package in data.get("package", []):
        checksum = package.get("checksum")
        if checksum:
            checksums[(package["name"], package["version"])] = checksum
    return checksums


def cargo_purl(name: str, version: str) -> str:
    return f"pkg:cargo/{quote(name, safe='')}@{quote(version, safe='')}"


def cargo_components(metadata: dict, checksums: dict[tuple[str, str], str]) -> tuple[list[dict], list[dict], str]:
    packages = {package["id"]: package for package in metadata["packages"]}
    id_to_ref: dict[str, str] = {}
    components: list[dict] = []
    for package_id, package in packages.items():
        name = package["name"]
        version = package["version"]
        reference = cargo_purl(name, version)
        id_to_ref[package_id] = reference
        component: dict = {
            "type": "library",
            "bom-ref": reference,
            "name": name,
            "version": version,
            "purl": reference,
            "properties": [
                {"name": "disp:cargo:source", "value": package.get("source") or "workspace"}
            ],
        }
        checksum = checksums.get((name, version))
        if checksum:
            if not SHA256.fullmatch(checksum):
                raise ValueError(f"invalid Cargo checksum for {name} {version}")
            component["hashes"] = [{"alg": "SHA-256", "content": checksum}]
        license_expression = package.get("license")
        if license_expression:
            component["licenses"] = [{"expression": license_expression}]
        components.append(component)

    dependencies: list[dict] = []
    for node in metadata["resolve"]["nodes"]:
        dependencies.append(
            {
                "ref": id_to_ref[node["id"]],
                "dependsOn": sorted(id_to_ref[item["pkg"]] for item in node["deps"]),
            }
        )
    root_id = metadata["resolve"].get("root")
    if root_id is None:
        workspace_members = metadata.get("workspace_members", [])
        if len(workspace_members) != 1:
            raise ValueError("SBOM generation requires one resolvable root package")
        root_id = workspace_members[0]
    return components, dependencies, id_to_ref[root_id]


def sha256_file(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest()


def dpkg_identity(path: Path) -> tuple[str | None, str | None]:
    try:
        owner_output = run(["dpkg-query", "-S", str(path)]).strip()
        owner = owner_output.split(":", 1)[0]
        version = run(["dpkg-query", "-W", "-f=${Version}", owner]).strip()
        return owner, version
    except (FileNotFoundError, subprocess.CalledProcessError):
        return None, None


def native_components(artifacts: list[Path]) -> list[dict]:
    if not artifacts:
        return []
    if sys.platform == "win32":
        return windows_native_components(artifacts)
    if sys.platform == "darwin":
        return macos_native_components(artifacts)
    if sys.platform != "linux":
        raise RuntimeError("--artifact native inventory requires Linux, Windows, or macOS")
    libraries: dict[tuple[str, str], dict] = {}
    for artifact in artifacts:
        if not artifact.is_file():
            raise FileNotFoundError(f"release artifact does not exist: {artifact}")
        output = run(["ldd", str(artifact)])
        for raw_line in output.splitlines():
            line = raw_line.strip()
            if not line or line.startswith("linux-vdso"):
                continue
            if "=> not found" in line:
                raise RuntimeError(f"unresolved native dependency for {artifact}: {line}")
            if "=>" in line:
                name, remainder = (part.strip() for part in line.split("=>", 1))
                resolved_text = remainder.split(" ", 1)[0]
            else:
                resolved_text = line.split(" ", 1)[0]
                name = Path(resolved_text).name
            resolved = Path(resolved_text).resolve(strict=True)
            file_hash = sha256_file(resolved)
            package, version = dpkg_identity(resolved)
            identity = package or name
            version = version or "unknown"
            reference = f"pkg:generic/{quote(identity, safe='')}@{quote(version, safe='')}?sha256={file_hash}"
            libraries[(identity, file_hash)] = {
                "type": "library",
                "bom-ref": reference,
                "name": identity,
                "version": version,
                "hashes": [{"alg": "SHA-256", "content": file_hash}],
                "properties": [
                    {"name": "disp:native:soname", "value": name},
                    {"name": "disp:native:source", "value": "ldd-resolved-release-artifact"},
                ],
            }
    return list(libraries.values())


def pe_rva_to_offset(rva: int, sections: list[tuple[int, int, int]]) -> int:
    for virtual_address, span, raw_offset in sections:
        if virtual_address <= rva < virtual_address + span:
            return raw_offset + (rva - virtual_address)
    raise ValueError(f"PE RVA 0x{rva:x} is outside every section")


def pe_c_string(data: bytes, offset: int, limit: int | None = None) -> str:
    maximum = min(len(data), offset + 4096, limit if limit is not None else len(data))
    end = data.find(b"\0", offset, maximum)
    if end < 0:
        raise ValueError("PE import name is unterminated")
    return data[offset:end].decode("ascii")


def pe_imports(path: Path) -> list[str]:
    data = path.read_bytes()
    if len(data) < 64 or data[:2] != b"MZ":
        raise ValueError(f"artifact is not a PE image: {path}")
    pe_offset = struct.unpack_from("<I", data, 0x3C)[0]
    if pe_offset + 24 > len(data) or data[pe_offset : pe_offset + 4] != b"PE\0\0":
        raise ValueError(f"artifact has an invalid PE header: {path}")
    section_count = struct.unpack_from("<H", data, pe_offset + 6)[0]
    optional_size = struct.unpack_from("<H", data, pe_offset + 20)[0]
    optional = pe_offset + 24
    magic = struct.unpack_from("<H", data, optional)[0]
    if magic == 0x20B:
        directories = optional + 112
        image_base = struct.unpack_from("<Q", data, optional + 24)[0]
    elif magic == 0x10B:
        directories = optional + 96
        image_base = struct.unpack_from("<I", data, optional + 28)[0]
    else:
        raise ValueError(f"artifact has unknown PE optional-header magic 0x{magic:x}")
    section_table = optional + optional_size
    sections: list[tuple[int, int, int]] = []
    for index in range(section_count):
        offset = section_table + index * 40
        if offset + 40 > len(data):
            raise ValueError("PE section table is truncated")
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
            "<IIII", data, offset + 8
        )
        sections.append((virtual_address, max(virtual_size, raw_size), raw_offset))

    def directory(index: int) -> tuple[int, int]:
        offset = directories + index * 8
        if offset + 8 > optional + optional_size:
            return 0, 0
        return struct.unpack_from("<II", data, offset)

    names: set[str] = set()
    import_rva, import_size = directory(1)
    if import_rva and import_size:
        descriptor = pe_rva_to_offset(import_rva, sections)
        limit = min(len(data), descriptor + import_size)
        while descriptor + 20 <= limit:
            fields = struct.unpack_from("<IIIII", data, descriptor)
            if not any(fields):
                break
            name_offset = pe_rva_to_offset(fields[3], sections)
            names.add(pe_c_string(data, name_offset).lower())
            descriptor += 20

    delay_rva, delay_size = directory(13)
    if delay_rva and delay_size:
        descriptor = pe_rva_to_offset(delay_rva, sections)
        limit = min(len(data), descriptor + delay_size)
        while descriptor + 32 <= limit:
            fields = struct.unpack_from("<IIIIIIII", data, descriptor)
            if not any(fields):
                break
            attributes, name_value = fields[0], fields[1]
            name_rva = name_value if attributes & 1 else name_value - image_base
            name_offset = pe_rva_to_offset(name_rva, sections)
            names.add(pe_c_string(data, name_offset).lower())
            descriptor += 32
    return sorted(names)


def windows_file_version(path: Path) -> str:
    import ctypes
    from ctypes import wintypes

    version = ctypes.WinDLL("version", use_last_error=True)
    version.GetFileVersionInfoSizeW.argtypes = [wintypes.LPCWSTR, ctypes.POINTER(wintypes.DWORD)]
    version.GetFileVersionInfoSizeW.restype = wintypes.DWORD
    version.GetFileVersionInfoW.argtypes = [
        wintypes.LPCWSTR,
        wintypes.DWORD,
        wintypes.DWORD,
        ctypes.c_void_p,
    ]
    version.GetFileVersionInfoW.restype = wintypes.BOOL
    version.VerQueryValueW.argtypes = [
        ctypes.c_void_p,
        wintypes.LPCWSTR,
        ctypes.POINTER(ctypes.c_void_p),
        ctypes.POINTER(wintypes.UINT),
    ]
    version.VerQueryValueW.restype = wintypes.BOOL
    size = version.GetFileVersionInfoSizeW(str(path), None)
    if size == 0:
        return "unknown"
    buffer = ctypes.create_string_buffer(size)
    if not version.GetFileVersionInfoW(str(path), 0, size, buffer):
        return "unknown"
    pointer = ctypes.c_void_p()
    length = wintypes.UINT()
    if not version.VerQueryValueW(buffer, "\\", ctypes.byref(pointer), ctypes.byref(length)):
        return "unknown"

    class FixedFileInfo(ctypes.Structure):
        _fields_ = [
            ("signature", wintypes.DWORD),
            ("structure_version", wintypes.DWORD),
            ("file_version_ms", wintypes.DWORD),
            ("file_version_ls", wintypes.DWORD),
            ("product_version_ms", wintypes.DWORD),
            ("product_version_ls", wintypes.DWORD),
            ("file_flags_mask", wintypes.DWORD),
            ("file_flags", wintypes.DWORD),
            ("file_os", wintypes.DWORD),
            ("file_type", wintypes.DWORD),
            ("file_subtype", wintypes.DWORD),
            ("file_date_ms", wintypes.DWORD),
            ("file_date_ls", wintypes.DWORD),
        ]

    info = ctypes.cast(pointer, ctypes.POINTER(FixedFileInfo)).contents
    if info.signature != 0xFEEF04BD:
        return "unknown"
    return ".".join(
        str(value)
        for value in (
            info.file_version_ms >> 16,
            info.file_version_ms & 0xFFFF,
            info.file_version_ls >> 16,
            info.file_version_ls & 0xFFFF,
        )
    )


def resolve_windows_dll(name: str, artifact: Path) -> Path | None:
    candidates = [artifact.parent / name]
    system_root = os.environ.get("SystemRoot")
    if system_root:
        candidates.append(Path(system_root) / "System32" / name)
    located = shutil.which(name)
    if located:
        candidates.append(Path(located))
    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()
    return None


def windows_native_components(artifacts: list[Path]) -> list[dict]:
    libraries: dict[tuple[str, str], dict] = {}
    os_version = os.environ.get("DISP_WINDOWS_BUILD", platform.version())
    for artifact in artifacts:
        if not artifact.is_file():
            raise FileNotFoundError(f"release artifact does not exist: {artifact}")
        for name in pe_imports(artifact):
            resolved = resolve_windows_dll(name, artifact)
            if resolved is None:
                if name.startswith(("api-ms-win-", "ext-ms-win-")):
                    reference = f"pkg:generic/{quote(name, safe='')}@{quote(os_version, safe='')}"
                    libraries[(name, "api-set")] = {
                        "type": "operating-system",
                        "bom-ref": reference,
                        "name": name,
                        "version": os_version,
                        "properties": [
                            {"name": "disp:native:source", "value": "windows-api-set-contract"}
                        ],
                    }
                    continue
                raise FileNotFoundError(f"Windows loader dependency cannot be resolved: {name}")
            file_hash = sha256_file(resolved)
            version = windows_file_version(resolved)
            reference = f"pkg:generic/{quote(name, safe='')}@{quote(version, safe='')}?sha256={file_hash}"
            libraries[(name, file_hash)] = {
                "type": "operating-system" if "system32" in str(resolved).lower() else "library",
                "bom-ref": reference,
                "name": name,
                "version": version,
                "hashes": [{"alg": "SHA-256", "content": file_hash}],
                "properties": [
                    {"name": "disp:native:source", "value": "pe-import-resolved-release-artifact"}
                ],
            }
    return list(libraries.values())


MACHO_DYLIB_COMMANDS = {0x0C, 0x20, 0x80000018, 0x8000001F, 0x80000023}
LC_RPATH = 0x8000001C


def macho_slice_imports(data: bytes) -> tuple[list[tuple[str, str]], list[str]]:
    if len(data) < 28:
        raise ValueError("Mach-O header is truncated")
    magic = struct.unpack_from("<I", data)[0]
    if magic == 0xFEEDFACF:
        header_size = 32
    elif magic == 0xFEEDFACE:
        header_size = 28
    else:
        raise ValueError(f"unsupported Mach-O magic 0x{magic:08x}")
    command_count, command_bytes = struct.unpack_from("<II", data, 16)
    if header_size + command_bytes > len(data):
        raise ValueError("Mach-O load-command region is truncated")
    imports: list[tuple[str, str]] = []
    rpaths: list[str] = []
    offset = header_size
    command_end = header_size + command_bytes
    for _ in range(command_count):
        if offset + 8 > command_end:
            raise ValueError("Mach-O load command is truncated")
        command, size = struct.unpack_from("<II", data, offset)
        if size < 8 or offset + size > command_end:
            raise ValueError("Mach-O load command has an invalid size")
        if command in MACHO_DYLIB_COMMANDS:
            if size < 24:
                raise ValueError("Mach-O dylib command is truncated")
            name_offset, _, current_version, _ = struct.unpack_from("<IIII", data, offset + 8)
            if not 24 <= name_offset < size:
                raise ValueError("Mach-O dylib name offset is invalid")
            name = pe_c_string(data, offset + name_offset, offset + size)
            version = (
                f"{current_version >> 16}."
                f"{(current_version >> 8) & 0xFF}."
                f"{current_version & 0xFF}"
            )
            imports.append((name, version))
        elif command == LC_RPATH:
            if size < 12:
                raise ValueError("Mach-O rpath command is truncated")
            path_offset = struct.unpack_from("<I", data, offset + 8)[0]
            if not 12 <= path_offset < size:
                raise ValueError("Mach-O rpath offset is invalid")
            rpaths.append(pe_c_string(data, offset + path_offset, offset + size))
        offset += size
    if offset != command_end:
        raise ValueError("Mach-O load-command sizes do not match the header")
    return imports, rpaths


def macho_imports(path: Path) -> tuple[list[tuple[str, str]], list[str]]:
    data = path.read_bytes()
    if len(data) < 8:
        raise ValueError(f"artifact is not a Mach-O image: {path}")
    magic_be = struct.unpack_from(">I", data)[0]
    if magic_be not in (0xCAFEBABE, 0xCAFEBABF):
        return macho_slice_imports(data)
    count = struct.unpack_from(">I", data, 4)[0]
    record_size = 32 if magic_be == 0xCAFEBABF else 20
    table_end = 8 + count * record_size
    if table_end > len(data):
        raise ValueError("universal Mach-O architecture table is truncated")
    imports: set[tuple[str, str]] = set()
    rpaths: set[str] = set()
    for index in range(count):
        record = 8 + index * record_size
        if record_size == 32:
            slice_offset, slice_size = struct.unpack_from(">QQ", data, record + 8)
        else:
            slice_offset, slice_size = struct.unpack_from(">II", data, record + 8)
        if slice_offset + slice_size > len(data):
            raise ValueError("universal Mach-O slice is truncated")
        slice_imports, slice_rpaths = macho_slice_imports(
            data[slice_offset : slice_offset + slice_size]
        )
        imports.update(slice_imports)
        rpaths.update(slice_rpaths)
    return sorted(imports), sorted(rpaths)


def expand_macho_path(value: str, artifact: Path) -> Path:
    return Path(
        value.replace("@executable_path", str(artifact.parent)).replace(
            "@loader_path", str(artifact.parent)
        )
    )


def resolve_macho_library(name: str, artifact: Path, rpaths: list[str]) -> Path | None:
    candidates: list[Path] = []
    if name.startswith("@rpath/"):
        suffix = name[len("@rpath/") :]
        candidates.extend(expand_macho_path(path, artifact) / suffix for path in rpaths)
    elif name.startswith(("@executable_path", "@loader_path")):
        candidates.append(expand_macho_path(name, artifact))
    elif name.startswith("/"):
        candidates.append(Path(name))
    else:
        candidates.append(artifact.parent / name)
    for candidate in candidates:
        if candidate.is_file():
            return candidate.resolve()
    return None


def macos_native_components(artifacts: list[Path]) -> list[dict]:
    libraries: dict[tuple[str, str], dict] = {}
    os_version = platform.mac_ver()[0] or platform.version()
    for artifact in artifacts:
        if not artifact.is_file():
            raise FileNotFoundError(f"release artifact does not exist: {artifact}")
        imports, rpaths = macho_imports(artifact)
        for name, load_version in imports:
            resolved = resolve_macho_library(name, artifact, rpaths)
            identity = Path(name).name
            if resolved is None:
                if name.startswith(("/System/Library/", "/usr/lib/")):
                    reference = (
                        f"pkg:generic/{quote(identity, safe='')}@{quote(os_version, safe='')}"
                        "?source=dyld-shared-cache"
                    )
                    libraries[(name, "dyld-cache")] = {
                        "type": "operating-system",
                        "bom-ref": reference,
                        "name": identity,
                        "version": os_version,
                        "properties": [
                            {"name": "disp:native:install-name", "value": name},
                            {"name": "disp:native:load-version", "value": load_version},
                            {"name": "disp:native:source", "value": "macos-dyld-shared-cache"},
                        ],
                    }
                    continue
                raise FileNotFoundError(f"Mach-O dependency cannot be resolved: {name}")
            file_hash = sha256_file(resolved)
            reference = (
                f"pkg:generic/{quote(identity, safe='')}@{quote(load_version, safe='')}"
                f"?sha256={file_hash}"
            )
            libraries[(name, file_hash)] = {
                "type": "operating-system" if name.startswith("/System/Library/") else "library",
                "bom-ref": reference,
                "name": identity,
                "version": load_version,
                "hashes": [{"alg": "SHA-256", "content": file_hash}],
                "properties": [
                    {"name": "disp:native:install-name", "value": name},
                    {"name": "disp:native:source", "value": "macho-load-command-release-artifact"},
                ],
            }
    return list(libraries.values())


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--manifest", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--artifact", action="append", default=[], type=Path)
    arguments = parser.parse_args()

    manifest = arguments.manifest.resolve(strict=True)
    lockfile = manifest.parent / "Cargo.lock"
    if not lockfile.is_file():
        raise FileNotFoundError(f"locked graph is missing: {lockfile}")
    metadata = json.loads(
        run(
            [
                "cargo",
                "metadata",
                "--locked",
                "--format-version",
                "1",
                "--manifest-path",
                str(manifest),
            ]
        )
    )
    components, dependencies, root_reference = cargo_components(
        metadata, locked_checksums(lockfile)
    )
    natives = native_components([path.resolve() for path in arguments.artifact])
    components.extend(natives)
    for dependency in dependencies:
        if dependency["ref"] == root_reference:
            dependency["dependsOn"] = sorted(
                set(dependency["dependsOn"]) | {item["bom-ref"] for item in natives}
            )
    dependencies.extend({"ref": item["bom-ref"], "dependsOn": []} for item in natives)

    root_component = next(item for item in components if item["bom-ref"] == root_reference)
    root_component["type"] = "application"
    metadata_section: dict = {
        "tools": {
            "components": [
                {
                    "type": "application",
                    "name": "DISP locked Cargo/native SBOM generator",
                    "version": GENERATOR_VERSION,
                }
            ]
        },
        "component": root_component,
        "properties": [
            {"name": "disp:lockfile:sha256", "value": sha256_file(lockfile)},
            {"name": "disp:native:artifact-count", "value": str(len(arguments.artifact))},
        ],
    }
    epoch = os.environ.get("SOURCE_DATE_EPOCH")
    if epoch is not None:
        instant = datetime.datetime.fromtimestamp(int(epoch), tz=datetime.timezone.utc)
        metadata_section["timestamp"] = instant.isoformat().replace("+00:00", "Z")

    bom = {
        "bomFormat": "CycloneDX",
        "specVersion": "1.6",
        "version": 1,
        "metadata": metadata_section,
        "components": sorted(components, key=lambda item: item["bom-ref"]),
        "dependencies": sorted(dependencies, key=lambda item: item["ref"]),
    }
    arguments.output.parent.mkdir(parents=True, exist_ok=True)
    arguments.output.write_text(
        json.dumps(bom, indent=2, sort_keys=True) + "\n", encoding="utf-8", newline="\n"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
