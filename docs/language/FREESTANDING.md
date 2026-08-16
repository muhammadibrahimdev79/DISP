# DISP freestanding profile

Status: completed Pass 021 core profile, August 16, 2026.

The freestanding target produces a machine-bootable image without an operating system, heap,
libc, C compiler, linker, or language runtime in the output. It is a deliberately strict first
system profile, not an alias for an ordinary hosted executable.

## Build and boot

```disp
fn main() {
    print("Hello from freestanding DISP")
    var total: u16 = 0
    var next: u16 = 1
    while next <= 10 {
        total += next
        next += 1
    }
    print(total)
}
```

```text
disp build --freestanding examples/freestanding_hello.disp
```

The command writes `examples/build/freestanding_hello-x86-bios.img`. A program fitting in 510 bytes
is emitted as exactly one 512-byte legacy x86 BIOS boot sector. A larger program is emitted as a
signed 512-byte loader followed by a sector-padded second stage. BIOS teletype output is also
mirrored to I/O port `0xe9`, enabling deterministic QEMU and Bochs test capture. Rebuilding identical
source produces byte-identical output.

The current profile requires exactly one plain `fn main()` entry and permits additional bounded
scalar helper functions. It supports constant text output and an allocation-free computation subset:

- initialized `let`, `var`, and folded `const` locals explicitly typed `u8`, `u16`, `u32`, `i32`, or
  `bool`;
- at most 128 lexically scoped locals and 4,096 aligned local bytes, stored in fixed machine memory
  without allocation;
- exact-width literals and locals, boolean literals, `!`, signed/unsigned comparisons, and
  short-circuit `&&` / `||`;
- checked `+`, `-`, `*`, `/`, and `%`, including compound assignment;
- `if`/`else`, `while`, indefinite `loop`, lexical `break`/`continue`, empty `return`, and nested
  blocks;
- `print` of constant ASCII text, runtime integers, or booleans, with allocation-free signed and
  unsigned decimal conversion.

Unsigned carry/borrow, multiply overflow, signed overflow, division by zero, and the `i32::MIN / -1`
case print `freestanding arithmetic failure` and halt; they never wrap or fault silently. Integer
widths and signedness are never implicitly mixed by the freestanding backend. Text is restricted to
printable ASCII, newline, and carriage return. Each `print` adds a CRLF line ending.

`u8` is a genuine compact byte representation: each fixed local occupies one byte and loads
zero-extend before computation. The 16-bit target stack transports byte arguments and saved byte
slots in two-byte words, and the generated stack guard charges that physical width rather than the
storage width. Byte arithmetic rejects results outside `0..=255`; it does not silently wrap.

## Guarded allocation-free function ABI

Helper functions may take and return `u8`, `u16`, `u32`, `i32`, or `bool`; `Unit` helpers are also
supported. They must be synchronous, non-generic, capability-free, and implemented in DISP.
Forward calls are supported.

Every function receives a deterministic machine label, and every parameter and lexical local
receives an aligned fixed-memory slot. The compiler inventories complete function frames before
emitting code, including locals declared in conditional and loop blocks.
At a call site, arguments are evaluated left-to-right and pushed at their declared widths. Only
after every argument succeeds are values popped in reverse order into callee slots, preventing a
nested call from overwriting an outer call's pending arguments. A direct near call transfers control;
scalar results return in `AX` or `EAX`, and `Unit` calls return no value.

Before evaluating arguments, the caller snapshots every fixed slot owned by the callee onto the
machine stack. After return it preserves the scalar result, restores those slots in reverse order,
then reinstates the result. Consequently direct recursion, mutual recursion, forward calls, and
nested argument calls preserve independent logical frames without a heap or hosted frame manager.
Calling `main` still fails compilation because it is an entry point, not a helper.

Every call first compares the live stack pointer with a deterministic lower bound covering the
complete callee snapshot, argument values, near return address, and an additional expression reserve.
The stack may never cross `0x7000`, the address immediately above the complete reserved local arena;
exhaustion prints `freestanding stack limit exceeded` and halts before fixed local storage can be
corrupted. The initial stack is `0x7c00`, while fixed locals occupy the bounded arena beginning at
`0x6000`.

`break` and `continue` are resolved to the innermost lexical loop during code generation. A `while`
continue edge returns to condition evaluation; an indefinite-loop continue edge returns to its body
head. Break edges target the unique instruction after that loop. DISP exposes no arbitrary jump or
user-controlled machine label in the safe freestanding profile.

## Initial 32-bit protected-mode target

```text
disp build --freestanding32 examples/protected32_hello.disp
```

This command emits `examples/build/protected32_hello-x86-protected32.img`. The current profile accepts
one plain `fn main()` plus scalar helper functions, with explicitly typed `u8`, `u16`, `u32`, `i32`,
and `bool` parameters and returns. Locals additionally accept bounded fixed arrays such as
`[u8; 64]`, initialized with exact scalar literals such as `u8(7)`. It
supports initialized bindings, assignment and compound checked arithmetic, signed/unsigned
comparisons, boolean negation and short circuiting, `if`/`else`, `while`, indefinite `loop`, lexical
`break`/`continue`, checked array reads and writes, empty return, and `print` of ASCII text, runtime
integers, and booleans.

At most 128 scalar storage slots share a 4,096-byte deterministic arena beginning at `0x100000`, demonstrating
execution beyond the real-mode ceiling. `u8` occupies one byte, `u16` occupies two aligned bytes, and
`u32`/`i32`/`bool` occupy four aligned bytes. Narrow loads zero-extend and stores touch only their
declared width. Each fixed-array element consumes one slot and arrays are contiguous at their element
alignment. Runtime indices are unsigned-range checked before address scaling or memory access; a
negative signed index therefore also fails closed. Violation prints `protected32 index out of bounds`
and halts. Array elements participate independently in recursive frame snapshots. Narrow
overflow/underflow, signed overflow, multiplication high-half mismatch, zero
divisors, and `i32::MIN / -1` enter the defined arithmetic-failure path. Signed decimal output covers
the entire `i32` domain.

Protected helpers support forward and nested calls, `Unit` or scalar returns, direct recursion, and
mutual recursion. Before code generation the compiler inventories every parameter and lexical local
in each fixed high-memory frame. Each call checks `ESP`, snapshots all callee slots as 32-bit stack
words, evaluates arguments left-to-right, commits them in reverse, and restores the complete frame
after preserving the return accumulator. The stack begins at `0x90000`, may not cross `0x80000`, and
retains an additional expression reserve. Exhaustion prints `protected32 stack limit exceeded` and
halts before corruption; calls to the `main` entry are rejected.

Protected32 additionally exposes exact byte port operations as `Port.read_u8(port: u16) -> u8` and
`Port.write_u8(port: u16, value: u8)`. Both require a lexically enclosing explicit
`unsafe uses DeviceIo { ... }` contract. Bare unsafe blocks, unrelated unsafe capabilities, wrong
port/value widths, and unsupported port operations fail during checking. `DeviceIo` enters the
function effect manifest and propagates through direct calls, so callers cannot hide hardware
authority. Lowering uses the variable-port x86 `in al, dx` and `out dx, al` instructions without a
runtime, while arguments retain normal left-to-right evaluation. This is a low-level target API,
not a claim that an arbitrary port is safe for a particular machine; the unsafe contract makes that
device-specific obligation explicit and reviewable.
Hosted native builds and the interpreter reject `Port.*` before code generation or program
execution and direct users to `--freestanding32`; privileged I/O is never emitted into an ordinary
host process.

The generated image is direct and signed. The encoder retains a one-sector path when the complete
protected payload fits 510 bytes. The mandatory exception infrastructure now makes the current
baseline use the bounded EDD loader and a sector-padded protected stage relocated to `0x7e00`;
GDTR, GDT base, IDTR operand, handler addresses, and far-transfer addresses are all regenerated for
that origin. A stage above 64 sectors is rejected before writing an artifact.

The protected bootstrap normalizes segment state and requests A20 through the system-control port.
Before using high memory it performs a reversible alias probe between `0x500` and `0x100500`, restores
both original bytes, and prints `A` then halts if A20 remains disabled. It then loads a GDT with null
and flat 4 GiB 32-bit code/data descriptors, sets `CR0.PE`, and far-jumps into the protected code
selector. Protected code reloads `DS`, `ES`, `SS`, `FS`, and `GS`, establishes `ESP` at `0x90000`,
and never calls a BIOS interrupt. Output is written directly to VGA text memory at `0xb8000` and
mirrored with exact CRLF endings to debug port `0xe9`. Dynamic decimal conversion is allocation-free;
arithmetic carry, borrow, high-half overflow, and zero divisors print a defined failure and halt.
Identical source produces byte-identical images.

Before entering `main`, protected32 constructs 32 present DPL0 32-bit interrupt gates in a dedicated
256-byte IDT at `0x101000`, immediately beyond the maximum local arena, and loads its six-byte IDTR.
All architectural CPU-exception vectors initially converge on a non-returning fail-closed handler.
The handler disables maskable interrupts, reloads the flat data selectors, resets `ESP` and the VGA
cursor to known values, prints `protected32 CPU exception`, and halts. External interrupts remain
disabled until later interrupt-controller and driver passes define their ownership and acknowledgement
contracts; the current table is an exception-safety foundation, not a complete interrupt subsystem.

After loading the IDT and before entering `main`, protected32 clears one page directory and one page
table at `0x102000` and `0x103000`. It installs only PDE 0, leaves PTE 0 absent, and identity-maps
pages `0x1000` through `0x3ff000` as supervisor memory, then removes write permission from the
complete possible loader/stage envelope at `0x7000` through `0xffff`. It loads `CR3`, sets both
`CR0.PG` and `CR0.WP`, and
serializes instruction fetch before user code. Thus the flat 4 GiB segment descriptors do not imply
ambient access to 4 GiB: null-page accesses and every linear address at or above 4 MiB fault through
the installed exception path. The current non-PAE format has no execute-disable bit; finer
code/read-only-data separation, writable non-executable pages, guard pages, and per-component address
spaces remain later paging work.

Unsupported widths/statements, aggregate parameters or returns, non-ASCII text, and stages exceeding the safe
relocated bound fail at compile time. Per-vector handlers, privilege rings, and kernel services remain
explicit system-pass work.

## Initial x86-64 long-mode target

```text
disp build --freestanding64 examples/freestanding64_hello.disp
```

This command emits `examples/build/freestanding64_hello-x86_64-long.img`. The source profile accepts
exactly one plain `fn main()`. In addition to printable-ASCII text, it supports explicitly typed
`u8`, `u16`, `u32`, `i32`, and `bool` locals; initialized bindings and assignments; checked scalar
arithmetic; exact comparisons and short-circuit Boolean expressions; `if`, `while`, and `loop`;
`break`, `continue`, empty return, and typed decimal/Boolean output. Exact scalar functions support
forward references, nested calls, scalar parameters and returns, direct `Unit` calls, recursion, and
mutual recursion. Fixed arrays of supported scalars use exact compact storage, literal
initialization, checked dynamic reads, direct and compound element assignment, and recursive-frame
preservation. Unsupported language constructs fail during backend validation; they are not silently
delegated to a hosted runtime.

The boot image uses the bounded transactional EDD stage loader. It requests A20 and performs the same
reversible low/high alias probe as protected32 before touching high memory. In 32-bit protected mode
it proves that EFLAGS.ID is mutable, queries the extended CPUID ceiling, and requires the architectural
long-mode and execute-disable feature bits. Unsupported machines write `L` to VGA/debug output and
halt.

The transition zeroes four complete 4 KiB hierarchy pages at `0x100000` through `0x103000`, installs
PML4/PDPT/PD links, leaves virtual page zero absent, and identity-maps `0x1000` through `0x1fffff`
with 4 KiB PTEs. Every mapped leaf begins non-executable. The possible loader/stage envelope
`0x7000` through `0xffff` is the only execution whitelist and is also read-only; stack, VGA, paging
structures, IDT, and local storage remain writable where required but NX. The bootstrap sets
`CR4.PAE`, loads `CR3`, sets `IA32_EFER.LME|NXE`, then sets `CR0.PG|CR0.WP` before a far transfer to a
GDT descriptor with `L=1,D=0`. Addresses at or above 2 MiB remain non-present.

Long-mode code establishes a 64-bit stack, keeps `IF=0`, constructs 48 sixteen-byte DPL0 interrupt
gates in a dedicated IDT at `0x104000`, loads a ten-byte IDTR, and only then enters DISP `main`. The initial
non-returning exception handler resets stack/output state, prints `x86-64 CPU exception`, and halts.
Vectors 6, 13, and 14 are then replaced with distinct DPL0 interrupt gates for invalid opcode,
general protection, and page fault. Each dedicated handler disables interrupts, resets the stack and
output cursor to known compiler-owned addresses, prints a stable vector-specific diagnostic, and
halts without consuming or returning through the interrupted frame. Other first-32-vector faults
retain the common fail-closed handler; external interrupts and user-installed handlers remain outside
this profile.

The remaining 16 gates cover hardware vectors 32 through 47. After loading the IDTR, compiler-owned
bootstrap code initializes both legacy 8259 PICs in 8086 mode, remaps the master to vectors 32–39 and
the slave to 40–47, records the IRQ2 cascade, and writes `0xff` to both interrupt masks. Every PIC
write is followed by the conventional port-`0x80` delay. Thus no device can asynchronously enter
DISP code in this bounded profile. If an unexpected already-pending hardware interrupt reaches the
table, its common handler disables interrupts, restores known stack/output state, acknowledges slave
then master, prints `x86-64 unexpected hardware interrupt`, and halts. Selective unmasking, `STI`,
selective device IRQs beyond the capability timer, APIC operation, and source-level handlers remain
future controlled capabilities.
Scalar locals occupy at most one compiler-owned writable page at `0x105000`; byte, word, and dword
loads/stores use explicit long-mode absolute-address encodings. Expression temporaries use balanced
64-bit stack operations, while checked arithmetic operates on exact low-width values and branches to
a non-returning diagnostic on overflow or division failure. Output uses 64-bit registers and direct
VGA/debug-port access without BIOS, libc, allocator, OS, or language runtime. Linux CI boots the
foundation, checked-scalar, deliberate-overflow, recursive-function, stack-exhaustion, fixed-array,
bounds-failure, and authorized-device-I/O artifacts under `qemu-system-x86_64` and compares exact
CRLF output, including all non-returning diagnostics.

Every function receives compiler-assigned fixed scalar slots. Before a call, generated code compares
`RSP` against the `0x80000` stack floor plus a 4096-byte expression reserve and the complete pending
frame requirement. It then snapshots every callee parameter/local as a 64-bit stack word, evaluates
arguments left-to-right, commits them in reverse, calls through a fixed relative target, preserves
the return accumulator, and restores every prior slot in reverse. This convention keeps nested,
recursive, and mutually recursive calls isolated without a heap or hidden runtime. Exhaustion prints
`x86-64 stack limit exceeded` and halts; a dedicated QEMU fixture exercises that path.

Each array index is evaluated once, compared unsigned against the declared length, and scaled by the
exact one-, two-, or four-byte element width before access. Indexed long-mode operands use the
checked offset plus the compiler-assigned arena base. A failed check reports
`x86-64 index out of bounds` and halts before any read or write. Array elements are individual frame
slots, so recursive calls snapshot and restore the full array without aliasing an earlier invocation.

The `DeviceIo` capability is distinct in long mode exactly as it is in protected32. Only an explicit
`unsafe uses DeviceIo` region inside a function whose effect contract includes `DeviceIo` may call
`Port.read_u8(u16) -> u8` or `Port.write_u8(u16, u8)`. The backend emits the architectural `in al,dx`
and `out dx,al` instructions directly; missing, implicit, or mismatched authority fails in the normal
safety pipeline before image generation. QEMU verifies an authorized port round trip through the
debug device.

The optional `Timer` capability is the only exception to the fully quarantined interrupt profile.
A function containing `Time.ticks() -> u32` must explicitly declare `uses Timer`; inference alone is
rejected by this backend. If and only if such a contract exists, the compiler replaces vector 32
with a dedicated interrupt gate, clears a naturally aligned counter at `0x106000`, programs PIT
channel 0 with divisor 11932 for approximately 100 Hz, unmasks only master IRQ0, keeps every slave
line masked, and executes `STI` after the table and controller are ready. The interrupt gate preserves
RAX, increments the counter once, acknowledges only the master PIC, and returns with `iretq`; the
interrupt-gate entry itself prevents nesting. `Time.ticks()` is one aligned `u32` load and wraps
modulo 2^32 in fixed 10 millisecond units. The ordinary no-`Timer` image still masks every IRQ and
contains no `STI`. Linux QEMU boots a fixture that busy-waits until a real IRQ advances the counter
before emitting its success line.

## Direct AArch64 virt-8.2 profile

```text
disp build --freestanding-aarch64 examples/freestanding_aarch64_hello.disp
```

This command emits `examples/build/freestanding_aarch64_hello-aarch64-virt-8.2.img`. It is DISP's
first direct non-x86 artifact. The current source profile requires exactly one plain `fn main()` and
permits additional plain functions with initialized, explicitly typed `u8`, `u16`, `u32`, `i32`,
and `bool` locals. Compact integers occupy
one and two aligned bytes rather than widened slots. It lowers lexical scopes, assignments,
short-circuit boolean logic, unsigned comparisons, `if`/`else`, `while`, indefinite `loop`, `break`,
`continue`, and returns. Plain exact-scalar/`Unit` functions support nested calls and recursion.
Local fixed arrays support owned `u8`, `u16`, `u32`, `i32`, or `bool` elements, an exact-length
array-literal initializer, direct dynamic indexing, simple element assignment, and checked compound
element assignment. Elements occupy contiguous, naturally aligned storage at their exact widths;
all elements count toward the same 4,096-byte local-storage limit.
`print` directly formats every exact scalar, including the full
`i32` range, through one image-local 16-byte decimal buffer. Literal UTF-8 rejects embedded NUL;
static terminated strings are limited to 64 KiB, and CRLF is appended after every `print`. The image
is capped at 256 KiB and aligned scalar storage at 4,096 bytes. Every unsupported construct fails
during backend validation and never falls through to the hosted runtime.

The artifact begins with the architectural 64-byte little-endian Arm64 Image header: its first
instruction branches over the header, `text_offset` is `0x80000`, `image_size` equals the exact padded
artifact length, flags select little-endian 4 KiB placement, and magic is `ARM\x64` (`0x644d5241`).
The payload consists only of directly encoded A64 instructions and target-owned static data. `ADR`
establishes position-independent local-storage and string addresses without relocation records; a
bounded boot prelude obtains the UART address from the FDT in `x0`. Unsigned addition and subtraction branch on carry/borrow, multiplication widens
to 64 bits, and compact results are checked against their exact maxima. Signed operations branch on
A64 overflow; signed multiplication must equal the sign extension of its low half. Division and
remainder reject zero, and `i32::MIN / -1` is rejected before `SDIV`.
All failures print `[DISP arithmetic fault]` and enter the non-returning `wfi` loop. Comparisons produce
canonical booleans; `&&` and `||` branch before evaluating the right operand. Literal output reads one
byte with post-increment, polls PL011 `UARTFR.TXFF`, and writes `UARTDR`.

The entry point derives and installs its own 16-byte-aligned stack inside the image; it never trusts
firmware stack state. Each push compares `sp` with the PC-derived floor before mutation. Calls use
direct `BL`/`RET`, save `x30`, snapshot every callee parameter/local in exact-width-neutral 16-byte
slots, install arguments, and restore the frame after return. Binary-expression temporaries use the
same guard, which keeps recursive expressions isolated. Exhaustion prints `[DISP stack exhausted]`
without using the exhausted stack and halts. The backend uses no allocator, OS, firmware call, libc,
assembler, linker, or language runtime.

An array index is evaluated as an exact integer and compared unsigned against the declared length.
That single comparison rejects both an index equal to the length and a signed negative index before
address calculation, memory access, or assignment right-hand-side effects. Valid element addresses
are derived from the compiler-owned base plus the index scaled by one, two, or four bytes; loads and
stores preserve the element width. Indexed compound assignment uses the same checked arithmetic as
scalar assignment. Every array element is included in the callee-frame snapshot, so recursive calls
cannot overwrite a caller's array. An invalid index prints `[DISP index out of bounds]` through the
stack-independent PL011 path and halts without returning. Only direct local arrays are indexable;
array parameters, array results, slices, and aggregate nesting remain rejected.

Before ordinary code runs, entry masks debug, system-error, IRQ, and FIQ exceptions, reads
`CurrentEL`, and installs one image-owned vector table in `VBAR_EL1` or `VBAR_EL2`. Any other
exception level fails closed. The complete table is 2 KiB aligned and contains all sixteen 128-byte
architectural entries. Each of the four exception-origin groups routes synchronous, IRQ, FIQ, and
system-error entries to distinct stack-independent PL011 diagnostics and the common non-returning
halt loop. There is no `ERET` recovery path. A fixed NOP at complete-image offset 128 gives CI one
non-semantic fault-injection checkpoint: a copied artifact replaces only that instruction with
`BRK`, proving real synchronous vector delivery while leaving the production artifact unchanged.
Interrupt sources remain disabled and masked; syndrome reporting and context recovery are not yet
part of this foundation.

Before installing VBAR or touching MMIO, the entry parses the flattened device tree passed by the
boot contract in `x0`. It accepts only a nonzero aligned FDT of 40 bytes through 2 MiB with bounded
version-17-compatible structure and string blocks, root 64-bit address/size cells, nesting no deeper
than 64, exactly one direct-child `arm,pl011` device, and a direct-child `memory` range containing the
complete image. Node names, property names, aligned values, compatible string-list members, tokens,
and all pointer/range arithmetic are checked before access. The UART must be nonzero, page-aligned,
below 4 GiB, at least one page wide, unique, and outside the image's root-table slot. Invalid,
ambiguous, or incomplete input enters a silent `wfi` loop because no MMIO address has yet been
authenticated.

The target then enables a bounded stage-1 MMU regime at the detected exception level. Five 4 KiB
tables form a sparse three-level identity map. The static root contains only the image branch and its
reserved device tables are empty. While translation is still off, the validated prelude patches a
root/L2/L3 path selected from the discovered UART address; the resulting tables map one device page
and exactly the complete artifact. Header, code, and vector
pages are privileged read-only executable. Static data, exact locals, decimal storage, and the stack
are privileged read-write execute-never. Translation-table pages become privileged read-only
execute-never, and PL011 is Device-nGnRnE, read-write, and execute-never. Every other virtual page is
invalid. Both EL1 and EL2 paths install MAIR/TCR/TTBR0, invalidate the current translation regime,
preserve SCTLR state, and enable translation, instruction/data caches, and WXN through ordered
barriers. The mapping is identity-based so instruction execution continues across activation.

Current-level data aborts are separated from other synchronous exceptions and produce
`[DISP memory protection fault]`. A second fixed NOP at image offset 280 executes only after the MMU
is live while `x28` identifies an executable page. Linux CI patches a copied artifact at that exact
checkpoint with `STR WZR,[X28]`; reaching the protection diagnostic proves that code and vectors are
not writable. Virtual relocation, demand paging, EL0, heap allocation, and runtime permission
changes remain outside this bounded foundation.

Explicit `unsafe uses DeviceIo` regions may access the authenticated PL011 page through
`Mmio.read_u8`, `read_u16`, or `read_u32` and the three matching writes. The argument is a `u16`
offset relative to the discovered page, never an absolute source-provided address. Each access checks
the complete selected width against the 4 KiB boundary and checks natural halfword/word alignment
before a write value is evaluated or any register is touched. Failure prints
`[DISP device access fault]` and halts. Successful operations use one exact-width A64 load or store
between `DMB OSH` barriers. This is volatile ordered device access, not ordinary memory that an
optimizer may merge or elide. Calls propagate `DeviceIo`; bare unsafe, `RawMemory`, and `Foreign`
cannot authorize them.

This first board contract remains intentionally pinned to QEMU `virt-8.2` and `cortex-a53`, but it no
longer embeds or assumes UART0's historical address. Linux CI loads the file through QEMU's direct
Arm64 kernel path and compares exact serial output for the board-generated FDT and for a separately
compiled checked-in DTB whose property order and compatible string list exercise the bounded parser.
The runtime matrix also covers the original hello image, scalar/control,
exact-width/scalar-output, recursive-function, stack-exhaustion, fixed-array, bounds-failure,
exception-readiness, injected synchronous-exception, MMU-readiness, injected write-protection, and
signed-overflow fixtures. Independent instruction emulation supplies an alternate PL011 address and
verifies successful output, patched descriptor state, data-abort routing, capability MMIO, bounded
access rejection, and silent rejection of an invalid FDT. The
parser deliberately does not interpret arbitrary bus ranges, phandles, interrupts, or physical-board
topology; richer exception state and aggregates also remain before broader compatibility can be claimed.

With Python Unicorn installed, `tools/verify_aarch64_hardware.py` is the independent executable oracle. It
uses PL011 at `0x0a000000`, inspects all three runtime-installed descriptors, injects the protected
data-abort route, and rejects both bad magic and a malformed `memory` prefix without touching device
tables. It also executes a real capability-gated status-register read, data-register write, and
end-of-page rejection against that alternate address. QEMU's copied-image `STR WZR,[X28]` gate is the
real MMU protection probe.

## Deterministic x86 BIOS image layout

Small programs retain the minimal one-sector layout and required `55 aa` signature. When machine
code, routines, and embedded data exceed 510 bytes, the first sector becomes a fixed loader. It:

1. normalizes real-mode segments and the stack;
2. preserves the BIOS boot-drive identifier;
3. uses the INT 13h extended-read packet to load contiguous sectors from LBA 1 at `0000:7e00`;
4. transfers control with an explicit far jump, or prints `!` and halts if loading fails.

The second stage is independently relocated for origin `0x7e00`. Its sector count is encoded in an
aligned 16-byte disk-address packet, and unused bytes are zero padded. At most 64 stage sectors are
accepted so code/data cannot cross the real-mode address ceiling. The checked-in multisector example
exercises the loader plus checked `u8`, `u16`, `u32`, `i32`, and boolean output in QEMU. The computational
profile requires an Intel 80386-compatible processor because exact 32-bit operations use real-mode
operand-size prefixes and conditional-set instructions.

The compiler runs the normal lexer, parser, expansion, resolver, type, ownership, effect, HIR, MIR,
and control-flow validation pipeline first. Freestanding validation then rejects modules, imports,
user types, traits, implementations, other numeric widths, heap-backed values, unsupported
expressions/statements, calls to `main`, async/generic functions, unsupported capabilities, non-scalar
parameters or returns, and external ABIs. Rejection is a compile-time backend diagnostic and never
falls back to the hosted runtime.

## Security and reproducibility boundary

- The generators emit x86 or A64 machine instructions, branches, target-owned data, and required
  output routines directly from the validated AST.
- It never invokes a compiler, assembler, linker, package manager, or build script.
- Image size and encoding are checked before filesystem mutation.
- Output is written to a same-directory staging file, flushed, synchronized, and then installed;
  a failed replacement restores the prior artifact.
- Generated programs initialize segment and stack state, restrict writes to compiler-assigned fixed
  locals plus documented output devices, disable interrupts before their terminal halt, and remain
  in a halt loop.

Pass 021 is complete at DISP-CORE-0095. The three x86 execution profiles remain broader than the
AArch64 profile; that does not weaken the common freestanding contract, and architecture/device
expansion continues under hardware and kernel Passes 026–028.
AArch64 virtual relocation, exception recovery/context, richer DTB bus/interrupt discovery, physical boards,
further architectures, embedded linker layouts, and kernel facilities remain later increments and
must not be inferred from these artifacts.
