#!/usr/bin/env python3

import struct
import unittest

import generate_sbom


class MachOParserTests(unittest.TestCase):
    def test_reads_versioned_dylib_and_rpath_commands(self) -> None:
        dylib_name = b"@rpath/libExample.dylib\0"
        dylib_size = (24 + len(dylib_name) + 7) & ~7
        dylib = struct.pack(
            "<IIIIII", 0x0C, dylib_size, 24, 0, (1 << 16) | (2 << 8) | 3, 0
        ) + dylib_name.ljust(dylib_size - 24, b"\0")

        rpath_name = b"@loader_path/Frameworks\0"
        rpath_size = (12 + len(rpath_name) + 7) & ~7
        rpath = struct.pack("<III", 0x8000001C, rpath_size, 12) + rpath_name.ljust(
            rpath_size - 12, b"\0"
        )
        commands = dylib + rpath
        header = struct.pack(
            "<IIIIIIII", 0xFEEDFACF, 0x01000007, 3, 2, 2, len(commands), 0, 0
        )
        imports, rpaths = generate_sbom.macho_slice_imports(header + commands)
        self.assertEqual(imports, [("@rpath/libExample.dylib", "1.2.3")])
        self.assertEqual(rpaths, ["@loader_path/Frameworks"])

    def test_rejects_truncated_load_commands(self) -> None:
        header = struct.pack("<IIIIIIII", 0xFEEDFACF, 0, 0, 2, 1, 24, 0, 0)
        with self.assertRaises(ValueError):
            generate_sbom.macho_slice_imports(header + b"\0" * 8)


if __name__ == "__main__":
    unittest.main()
