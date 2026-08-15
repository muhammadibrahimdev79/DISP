# Initial native baselines

Measured on 2026-08-09 on Windows x86-64 with an Intel Core Ultra 7 258V.
These are bootstrap baselines only; they are not cross-language performance claims.

| Measurement | Configuration | Result |
| --- | --- | ---: |
| Compiler startup plus `check examples/hello.disp` | debug bootstrap compiler, mean of 5 | 20.022 ms |
| `examples/hello.disp` build | debug native output, mean of 5 | 306.059 ms |
| `hello.exe` size | release native output | 136,483 bytes |
| Arithmetic loop, 1,000,000 checked additions | release, process startup included, mean of 7 | 90.719 ms |
| Recursive Fibonacci(20), plus ten setup iterations | release, process startup included, mean of 7 | 11.466 ms |

`hello.exe` startup could not be measured reliably because Windows Application
Control consistently rejected that particular generated executable with OS error
4551. More complex native executables, the compiler, and sanitizer-instrumented
fuzz executables ran successfully. A timing of the policy rejection is not a
program-startup measurement and is intentionally not reported as one.

The benchmark sources are
[`native_arithmetic_benchmark.disp`](../../compiler/examples/native_arithmetic_benchmark.disp)
and [`native_recursive_benchmark.disp`](../../compiler/examples/native_recursive_benchmark.disp).
