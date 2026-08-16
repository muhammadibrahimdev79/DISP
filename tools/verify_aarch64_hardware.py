import struct
from pathlib import Path

from unicorn import (
    UC_ARCH_ARM64,
    UC_HOOK_CODE,
    UC_HOOK_MEM_READ,
    UC_HOOK_MEM_WRITE,
    UC_MODE_ARM,
    Uc,
)
from unicorn.arm64_const import UC_ARM64_REG_PC, UC_ARM64_REG_X0, UC_ARM64_REG_X20, UC_ARM64_REG_X24


LOAD = 0x40080000
DTB = 0x48000000
UART = 0x0A000000
WFI = 0xD503207F


def pad4(value: bytes) -> bytes:
    return value + bytes((-len(value)) % 4)


def make_dtb(memory_type: bytes = b"memory\0") -> bytes:
    names = [b"#address-cells\0", b"#size-cells\0", b"device_type\0", b"reg\0", b"compatible\0"]
    strings = b"".join(names)
    offsets = {}
    cursor = 0
    for name in names:
        offsets[name[:-1].decode()] = cursor
        cursor += len(name)

    structure = bytearray()
    token = lambda value: structure.extend(struct.pack(">I", value))
    begin = lambda name: (token(1), structure.extend(pad4(name.encode() + b"\0")))
    prop = lambda name, value: (
        token(3),
        structure.extend(struct.pack(">II", len(value), offsets[name])),
        structure.extend(pad4(value)),
    )

    begin("")
    prop("#address-cells", struct.pack(">I", 2))
    prop("#size-cells", struct.pack(">I", 2))
    begin("memory@40000000")
    prop("reg", struct.pack(">QQ", 0x40000000, 0x08000000))
    prop("device_type", memory_type)
    token(2)
    begin("pl011@a000000")
    prop("reg", struct.pack(">QQ", UART, 0x1000))
    prop("compatible", b"arm,primecell\0arm,pl011\0")
    token(2)
    token(2)
    token(9)

    reserve = bytes(16)
    off_structure = 40 + len(reserve)
    off_strings = off_structure + len(structure)
    total = off_strings + len(strings)
    header = struct.pack(
        ">10I",
        0xD00DFEED,
        total,
        off_structure,
        off_strings,
        40,
        17,
        16,
        0,
        len(strings),
        len(structure),
    )
    return header + reserve + bytes(structure) + strings


def vector_table(image: bytes) -> int:
    for offset in range(2048, len(image) - 2047, 2048):
        if all(
            int.from_bytes(image[offset + slot * 128 : offset + slot * 128 + 4], "little")
            & 0xFC000000
            == 0x14000000
            for slot in range(16)
        ):
            return offset
    raise RuntimeError("aligned vector table not found")


def execute(image: bytes, dtb: bytes, simulate_data_abort: bool = False):
    table = vector_table(image)
    machine = Uc(UC_ARCH_ARM64, UC_MODE_ARM)
    machine.mem_map(LOAD, 0x40000)
    machine.mem_write(LOAD, image)
    machine.mem_map(DTB, 0x200000)
    machine.mem_write(DTB, dtb)
    machine.mem_map(UART, 0x1000)
    machine.reg_write(UC_ARM64_REG_X0, DTB)
    output = bytearray()
    device_accesses = []

    def capture(_machine, _access, address, _size, value, _user_data):
        if address == UART:
            output.append(value & 0xFF)

    def observe(_machine, access, address, size, _value, _user_data):
        if UART <= address < UART + 0x1000:
            device_accesses.append((access, address, size))

    def control(machine, address, _size, _user_data):
        offset = address - LOAD
        instruction = int.from_bytes(machine.mem_read(address, 4), "little")
        if offset == 64:
            machine.reg_write(UC_ARM64_REG_PC, address + 4)
        elif offset == 92:
            machine.reg_write(UC_ARM64_REG_X24, 4)
            machine.reg_write(UC_ARM64_REG_PC, address + 4)
        elif offset in (120, 124):
            machine.reg_write(UC_ARM64_REG_PC, address + 4)
        elif offset == 128:
            machine.reg_write(UC_ARM64_REG_PC, LOAD + 280)
        elif offset == 280:
            machine.reg_write(
                UC_ARM64_REG_PC, LOAD + table if simulate_data_abort else address + 4
            )
        elif instruction == 0xD5385200:
            machine.reg_write(UC_ARM64_REG_X0, 0x25 << 26)
            machine.reg_write(UC_ARM64_REG_PC, address + 4)
        elif instruction == WFI:
            machine.emu_stop()

    machine.hook_add(UC_HOOK_MEM_WRITE, capture)
    machine.hook_add(UC_HOOK_MEM_READ | UC_HOOK_MEM_WRITE, observe)
    machine.hook_add(UC_HOOK_CODE, control)
    machine.emu_start(LOAD, LOAD + len(image), count=4_000_000)
    root = LOAD + len(image) - 5 * 4096
    low_l2 = root + 4096
    low_l3 = low_l2 + 4096
    read_descriptor = lambda table, index: int.from_bytes(
        machine.mem_read(table + index * 8, 8), "little"
    )
    descriptors = (
        read_descriptor(root, (UART >> 30) & 0x1FF),
        read_descriptor(low_l2, (UART >> 21) & 0x1FF),
        read_descriptor(low_l3, (UART >> 12) & 0x1FF),
    )
    return (
        bytes(output),
        machine.reg_read(UC_ARM64_REG_X20),
        descriptors,
        device_accesses,
    )


image_path = (
    Path(__file__).resolve().parents[1]
    / "compiler/examples/build/freestanding_aarch64_mmu-aarch64-virt-8.2.img"
)
image = image_path.read_bytes()
valid = make_dtb()
cases = [
    ("valid alternate UART", valid, False, b"AArch64 MMU W^X active\r\n"),
    ("post-MMU protection", valid, True, b"[DISP memory protection fault]\r\n"),
    ("invalid magic", b"\0\0\0\0" + valid[4:], False, b""),
    ("malformed memory type", make_dtb(b"memory\0x"), False, b""),
]
for name, dtb, simulate_data_abort, expected in cases:
    actual, discovered, descriptors, _accesses = execute(
        image, dtb, simulate_data_abort
    )
    if actual != expected:
        raise SystemExit(f"expected {expected!r}, got {actual!r}")
    if expected and discovered != UART:
        raise SystemExit(f"expected UART {UART:#x}, got {discovered:#x}")
    root = LOAD + len(image) - 5 * 4096
    expected_descriptors = (
        root + 4096 | 3,
        root + 8192 | 3,
        UART | (1 << 53) | (1 << 54) | 0x607,
    )
    if expected and descriptors != expected_descriptors:
        raise SystemExit(
            f"expected runtime descriptors {expected_descriptors!r}, got {descriptors!r}"
        )
    if not expected and descriptors != (0, 0, 0):
        raise SystemExit(f"invalid DTB modified device descriptors: {descriptors!r}")
    print(f"PASS {name}: {actual!r}")

for name, filename, expected in [
    (
        "capability MMIO",
        "freestanding_aarch64_mmio-aarch64-virt-8.2.img",
        b"MMIO capability access active\r\n",
    ),
    (
        "MMIO page bound",
        "freestanding_aarch64_mmio_bounds-aarch64-virt-8.2.img",
        b"[DISP device access fault]\r\n",
    ),
]:
    runtime_image = image_path.with_name(filename).read_bytes()
    actual, discovered, descriptors, accesses = execute(runtime_image, valid)
    if actual != expected:
        raise SystemExit(f"expected {expected!r}, got {actual!r}")
    if discovered != UART:
        raise SystemExit(f"expected UART {UART:#x}, got {discovered:#x}")
    root = LOAD + len(runtime_image) - 5 * 4096
    expected_descriptors = (
        root + 4096 | 3,
        root + 8192 | 3,
        UART | (1 << 53) | (1 << 54) | 0x607,
    )
    if descriptors != expected_descriptors:
        raise SystemExit(
            f"expected runtime descriptors {expected_descriptors!r}, got {descriptors!r}"
        )
    if name == "capability MMIO" and not (
        len(accesses) >= 2
        and accesses[0][1:] == (UART + 24, 4)
        and accesses[1][1:] == (UART, 4)
    ):
        raise SystemExit(f"MMIO read/write did not execute first: {accesses[:4]!r}")
    if name == "MMIO page bound" and any(
        address not in (UART, UART + 24) for _access, address, _size in accesses
    ):
        raise SystemExit(f"rejected MMIO offset touched the device page: {accesses!r}")
    print(f"PASS {name}: {actual!r}")
