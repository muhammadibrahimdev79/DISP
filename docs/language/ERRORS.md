# DISP typed failure and cleanup model

DISP separates recoverable failure from fatal execution traps. Recoverable failure is ordinary,
typed data: `Result<T, E>` for an error value and `Option<T>` for absence. There is no hidden
exception channel, untyped `catch`, or implicit error conversion in the DISP 1.0 core.

## Propagation

Postfix `?` evaluates its operand exactly once. For `Result<T, E>`, `Ok(value)` continues with
`value` and `Err(error)` immediately returns the same `Err(error)` carrier. For `Option<T>`,
`Some(value)` continues and `None` immediately returns `None`. The enclosing callable must use
the same carrier, and a propagated `Result` error type must match exactly.

This exactness is deliberate. Conversions that lose information or authority are never guessed;
programs may convert errors explicitly before propagation.

## Cleanup

On propagation, every still-initialized owned local that is not part of the returned carrier is
destroyed exactly once in reverse lexical declaration order. Moved fields are not destroyed
again, while the initialized remainder of a partially moved aggregate is destroyed. The error
carrier is moved into the return place before local cleanup. These rules apply equally in
synchronous functions, asynchronous state machines, the interpreter, and native execution.

## Fatal traps

Internal invariant violations and operations whose checked preconditions were bypassed are fatal
traps, not recoverable errors. A trap terminates the affected program and cannot be caught by
`Result` handling. Public operations that can fail under normal environmental conditions—such as
filesystem, network, process, parsing, conversion, and data operations—return typed `Result`
values instead.
