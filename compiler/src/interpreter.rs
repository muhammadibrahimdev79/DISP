use crate::ast::{
    AssignmentOperator, BinaryOperator, Block, EnumDeclaration, Expr, Expression, Function,
    Pattern, Program, Statement, UnaryOperator, VariantDeclaration,
};
use crate::diagnostics::{Diagnostic, DiagnosticKind, Span};
use std::{
    any::Any,
    collections::HashMap,
    fs,
    io::{Read, Write},
    net::{Shutdown, TcpListener as StdTcpListener, TcpStream as StdTcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex as StdMutex, Weak,
        atomic::{AtomicBool, AtomicI64, Ordering},
    },
    thread,
    time::{Duration as StdDuration, Instant as StdInstant, SystemTime, UNIX_EPOCH},
};

#[derive(Clone)]
struct RuntimeThread(Arc<ThreadState>);

type ThreadResult = Result<Box<dyn Any + Send>, Diagnostic>;
type ThreadHandle = thread::JoinHandle<ThreadResult>;

struct ThreadState {
    handle: StdMutex<Option<ThreadHandle>>,
}

impl RuntimeThread {
    fn new(handle: ThreadHandle) -> Self {
        Self(Arc::new(ThreadState {
            handle: StdMutex::new(Some(handle)),
        }))
    }

    fn join(&self, span: Span) -> Result<Value, Diagnostic> {
        let handle = self
            .0
            .handle
            .lock()
            .map_err(|_| {
                Diagnostic::new(DiagnosticKind::Runtime, "thread state is poisoned", span)
            })?
            .take()
            .ok_or_else(|| {
                Diagnostic::new(
                    DiagnosticKind::Runtime,
                    "thread has already been joined",
                    span,
                )
            })?;
        let value = handle.join().map_err(|_| {
            Diagnostic::new(DiagnosticKind::Runtime, "spawned thread panicked", span)
        })??;
        value.downcast::<Value>().map(|value| *value).map_err(|_| {
            Diagnostic::new(
                DiagnosticKind::Runtime,
                "thread returned an invalid value",
                span,
            )
        })
    }
}

impl Drop for ThreadState {
    fn drop(&mut self) {
        if let Ok(slot) = self.handle.get_mut()
            && let Some(handle) = slot.take()
        {
            let _ = handle.join();
        }
    }
}

impl std::fmt::Debug for RuntimeThread {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Thread")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeThread {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeFuture(Arc<StdMutex<Option<FutureWork>>>);

enum FutureWork {
    Function(Box<Function>, Vec<Value>),
    Yield,
    Sleep(StdDuration),
    ReadText(PathBuf),
    ReadBytes(PathBuf),
    WriteText(PathBuf, RuntimeString),
    WriteBytes(PathBuf, Vec<u8>),
    Connect(RuntimeSocketAddress),
    Accept(RuntimeTcpListener, Option<StdDuration>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RuntimeSocketAddress {
    host: String,
    port: u16,
}

#[derive(Clone)]
struct RuntimeTcpStream(Arc<StdMutex<Option<StdTcpStream>>>);

#[derive(Clone)]
struct RuntimeTcpListener(Arc<StdMutex<Option<StdTcpListener>>>);

impl std::fmt::Debug for RuntimeTcpStream {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("TcpStream")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeTcpStream {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl std::fmt::Debug for RuntimeTcpListener {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("TcpListener")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeTcpListener {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl RuntimeFuture {
    fn new(function: Function, arguments: Vec<Value>) -> Self {
        Self(Arc::new(StdMutex::new(Some(FutureWork::Function(
            Box::new(function),
            arguments,
        )))))
    }

    fn yielding() -> Self {
        Self(Arc::new(StdMutex::new(Some(FutureWork::Yield))))
    }

    fn operation(work: FutureWork) -> Self {
        Self(Arc::new(StdMutex::new(Some(work))))
    }
}

impl std::fmt::Debug for RuntimeFuture {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Future")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeFuture {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeTask(Arc<StdMutex<RuntimeTaskWork>>);

enum RuntimeTaskWork {
    Future(RuntimeFuture),
    Running,
    Ready(Value),
    Consumed,
}

impl RuntimeTask {
    fn new(future: RuntimeFuture) -> Self {
        Self(Arc::new(StdMutex::new(RuntimeTaskWork::Future(future))))
    }
}

impl std::fmt::Debug for RuntimeTask {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Task")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeTask {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeMutex(Arc<RuntimeMutexState>);

struct RuntimeMutexState {
    locked: AtomicBool,
    value: StdMutex<Box<dyn Any + Send>>,
}

impl RuntimeMutex {
    fn new(value: Value) -> Self {
        Self(Arc::new(RuntimeMutexState {
            locked: AtomicBool::new(false),
            value: StdMutex::new(Box::new(value)),
        }))
    }

    fn lock(&self) -> RuntimeMutexGuard {
        while self
            .0
            .locked
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            thread::yield_now();
        }
        RuntimeMutexGuard(Arc::new(RuntimeMutexGuardState {
            mutex: Arc::clone(&self.0),
        }))
    }
}

impl std::fmt::Debug for RuntimeMutex {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("Mutex")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeMutex {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeMutexGuard(Arc<RuntimeMutexGuardState>);

struct RuntimeMutexGuardState {
    mutex: Arc<RuntimeMutexState>,
}

impl RuntimeMutexGuard {
    fn read(&self) -> Option<Value> {
        self.0
            .mutex
            .value
            .lock()
            .ok()?
            .downcast_ref::<Value>()
            .cloned()
    }

    fn write(&self, value: Value) -> Option<()> {
        *self.0.mutex.value.lock().ok()? = Box::new(value);
        Some(())
    }
}

impl Drop for RuntimeMutexGuardState {
    fn drop(&mut self) {
        self.mutex.locked.store(false, Ordering::Release);
    }
}

impl std::fmt::Debug for RuntimeMutexGuard {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("MutexGuard")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeMutexGuard {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Clone)]
struct RuntimeAtomicInt(Arc<AtomicI64>);

impl RuntimeAtomicInt {
    fn add(&self, delta: i64, span: Span) -> Result<(i64, i64), Diagnostic> {
        let mut current = self.0.load(Ordering::SeqCst);
        loop {
            let next = current.checked_add(delta).ok_or_else(|| {
                Diagnostic::new(DiagnosticKind::Runtime, "AtomicInt overflow", span)
            })?;
            match self
                .0
                .compare_exchange_weak(current, next, Ordering::SeqCst, Ordering::SeqCst)
            {
                Ok(previous) => return Ok((previous, next)),
                Err(observed) => current = observed,
            }
        }
    }
}

impl std::fmt::Debug for RuntimeAtomicInt {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_tuple("AtomicInt")
            .field(&Arc::as_ptr(&self.0))
            .finish()
    }
}

impl PartialEq for RuntimeAtomicInt {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeString {
    text: String,
    capacity: usize,
}

impl RuntimeString {
    fn literal(text: String) -> Self {
        let capacity = text.len();
        Self { text, capacity }
    }
    fn with_capacity(capacity: usize) -> Self {
        Self {
            text: String::new(),
            capacity,
        }
    }
    fn len(&self) -> usize {
        self.text.len()
    }
    fn capacity(&self) -> usize {
        self.capacity.max(self.text.len())
    }
    fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
    fn push(&mut self, value: char) {
        self.text.push(value);
        self.capacity = self.capacity.max(self.text.capacity());
    }
    fn push_str(&mut self, value: &RuntimeString) {
        self.text.push_str(&value.text);
        self.capacity = self.capacity.max(self.text.capacity());
    }
    fn clear(&mut self) {
        self.text.clear();
    }
}

#[derive(Debug, Clone, PartialEq)]
struct RuntimeCString(Arc<Vec<u8>>);

impl RuntimeCString {
    fn new(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.contains(&0) {
            return Err("CString source contains an interior NUL byte");
        }
        let mut terminated = Vec::with_capacity(bytes.len() + 1);
        terminated.extend_from_slice(bytes);
        terminated.push(0);
        Ok(Self(Arc::new(terminated)))
    }

    fn len(&self) -> usize {
        self.0.len().saturating_sub(1)
    }

    fn text(&self) -> String {
        String::from_utf8(self.0[..self.len()].to_vec())
            .expect("CString is created only from valid DISP UTF-8")
    }
}

#[derive(Debug)]
struct RuntimeMemoryState {
    bytes: Vec<u8>,
    alignment: usize,
}

#[derive(Debug, Clone)]
struct RuntimeMemory(Arc<StdMutex<RuntimeMemoryState>>);

impl PartialEq for RuntimeMemory {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
}

impl RuntimeMemory {
    fn new(size: usize, alignment: usize) -> Result<Self, &'static str> {
        if alignment == 0 || !alignment.is_power_of_two() {
            return Err("Memory alignment must be a non-zero power of two");
        }
        if alignment > 1 << 20 {
            return Err("Memory alignment exceeds the supported maximum");
        }
        if size
            .checked_add(3 * std::mem::size_of::<usize>())
            .and_then(|value| value.checked_add(alignment - 1))
            .is_none()
        {
            return Err("Memory size overflow");
        }
        let mut bytes = Vec::new();
        bytes
            .try_reserve_exact(size)
            .map_err(|_| "Memory allocation failed")?;
        bytes.resize(size, 0);
        Ok(Self(Arc::new(StdMutex::new(RuntimeMemoryState {
            bytes,
            alignment,
        }))))
    }

    fn len(&self) -> Option<usize> {
        Some(self.0.lock().ok()?.bytes.len())
    }

    fn alignment(&self) -> Option<usize> {
        Some(self.0.lock().ok()?.alignment)
    }

    fn read(&self, index: usize) -> Option<u8> {
        self.0.lock().ok()?.bytes.get(index).copied()
    }

    fn write(&self, index: usize, value: u8) -> Option<()> {
        *self.0.lock().ok()?.bytes.get_mut(index)? = value;
        Some(())
    }

    fn fill(&self, value: u8) -> Option<()> {
        self.0.lock().ok()?.bytes.fill(value);
        Some(())
    }

    fn copy_from(
        &self,
        destination: usize,
        source: &Self,
        source_offset: usize,
        count: usize,
    ) -> Option<()> {
        let destination_end = destination.checked_add(count)?;
        let source_end = source_offset.checked_add(count)?;
        if Arc::ptr_eq(&self.0, &source.0) {
            let mut state = self.0.lock().ok()?;
            if destination_end > state.bytes.len() || source_end > state.bytes.len() {
                return None;
            }
            state
                .bytes
                .copy_within(source_offset..source_end, destination);
            return Some(());
        }
        let copied = {
            let state = source.0.lock().ok()?;
            state.bytes.get(source_offset..source_end)?.to_vec()
        };
        let mut state = self.0.lock().ok()?;
        state
            .bytes
            .get_mut(destination..destination_end)?
            .copy_from_slice(&copied);
        Some(())
    }
}

fn grow_list_capacity(capacity: &mut usize, needed: usize) -> Result<(), &'static str> {
    if needed <= *capacity {
        return Ok(());
    }
    let mut next = (*capacity).max(4);
    while next < needed {
        next = next.checked_mul(2).ok_or("List capacity overflow")?;
    }
    *capacity = next;
    Ok(())
}

fn option_value(value: Option<Value>) -> Value {
    match value {
        Some(value) => Value::Enum {
            type_name: "Option".into(),
            variant: "Some".into(),
            payload: vec![value],
        },
        None => Value::Enum {
            type_name: "Option".into(),
            variant: "None".into(),
            payload: vec![],
        },
    }
}

fn runtime_result(value: Result<Value, std::io::Error>) -> Value {
    match value {
        Ok(value) => Value::Enum {
            type_name: "Result".into(),
            variant: "Ok".into(),
            payload: vec![value],
        },
        Err(error) => Value::Enum {
            type_name: "Result".into(),
            variant: "Err".into(),
            payload: vec![Value::String(RuntimeString::literal(error.to_string()))],
        },
    }
}

fn runtime_path(value: Value) -> Option<PathBuf> {
    match value {
        Value::Path(path) => Some(path),
        Value::String(text) => Some(PathBuf::from(text.text)),
        Value::Reference(_, _) => None,
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Value {
    Int(i64),
    UInt(u64),
    Signed(i128, u16),
    Unsigned(u128, u16),
    Float(f64),
    Float32(f32),
    Reference(Place, bool),
    String(RuntimeString),
    CString(RuntimeCString),
    CStr(RuntimeCString),
    Memory(RuntimeMemory),
    Path(PathBuf),
    SocketAddress(RuntimeSocketAddress),
    TcpStream(RuntimeTcpStream),
    TcpListener(RuntimeTcpListener),
    Instant(StdInstant),
    Duration(StdDuration),
    Thread(RuntimeThread),
    Future(RuntimeFuture),
    Task(RuntimeTask),
    Mutex(RuntimeMutex),
    MutexGuard(RuntimeMutexGuard),
    AtomicInt(RuntimeAtomicInt),
    Array(Vec<Value>),
    Slice(Vec<Value>),
    List {
        values: Vec<Value>,
        capacity: usize,
    },
    Map {
        entries: Vec<(Value, Value)>,
        capacity: usize,
    },
    Set {
        values: Vec<Value>,
        capacity: usize,
    },
    Char(char),
    Bool(bool),
    Function(String),
    Closure(Box<RuntimeClosure>),
    CaptureReference(Place, bool),
    Constructor {
        type_name: String,
        variant: String,
    },
    Struct {
        type_name: String,
        fields: HashMap<String, Value>,
    },
    Enum {
        type_name: String,
        variant: String,
        payload: Vec<Value>,
    },
    Unit,
    Uninitialized,
}

#[derive(Debug, Clone)]
struct RuntimeClosure {
    parameters: Vec<crate::ast::Parameter>,
    return_type: Option<crate::ast::TypeName>,
    body: crate::ast::ClosureBody,
    captures: Arc<StdMutex<HashMap<String, Value>>>,
}

impl PartialEq for RuntimeClosure {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.captures, &other.captures)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Place {
    scope: usize,
    name: String,
    fields: Vec<PlaceSegment>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum PlaceSegment {
    Field(String),
    Index(usize),
    MapValue(usize),
    Subslice(usize, usize),
    MutexValue,
}

enum RuntimeFault {
    Error(Diagnostic),
    Propagate(Value),
}

type RuntimeResult<T> = Result<T, RuntimeFault>;
const INTERPRETER_STACK_BYTES: usize = 8 * 1024 * 1024;

enum Flow {
    Normal,
    Return(Value),
    Break,
    Continue,
}

pub struct Interpreter {
    scopes: Vec<HashMap<String, Value>>,
    scope_orders: Vec<Vec<String>>,
    output: Arc<StdMutex<Vec<String>>>,
    call_depth: usize,
    tasks: Vec<Weak<StdMutex<RuntimeTaskWork>>>,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            scopes: Vec::new(),
            scope_orders: Vec::new(),
            output: Arc::new(StdMutex::new(Vec::new())),
            call_depth: 0,
            tasks: Vec::new(),
        }
    }

    pub fn run(&mut self, program: &Program) -> Result<Vec<String>, Diagnostic> {
        thread::scope(|scope| {
            let worker = thread::Builder::new()
                .name("disp-interpreter".into())
                .stack_size(INTERPRETER_STACK_BYTES)
                .spawn_scoped(scope, || self.run_inner(program))
                .map_err(|error| {
                    Diagnostic::new(
                        DiagnosticKind::Runtime,
                        format!("could not start interpreter: {error}"),
                        Span::point(1, 1),
                    )
                })?;
            worker.join().map_err(|_| {
                Diagnostic::new(
                    DiagnosticKind::Runtime,
                    "interpreter worker panicked",
                    Span::point(1, 1),
                )
            })?
        })
    }

    fn run_inner(&mut self, program: &Program) -> Result<Vec<String>, Diagnostic> {
        self.scopes.clear();
        self.scope_orders.clear();
        self.output
            .lock()
            .expect("interpreter output lock poisoned")
            .clear();
        self.call_depth = 0;
        self.tasks.clear();
        let main = program
            .functions
            .iter()
            .find(|function| function.name == "main")
            .ok_or_else(|| self.diagnostic("missing `main` function", Span::point(1, 1)))?
            .clone();
        let result = self
            .call_function(program, &main, Vec::new(), main.name_span)
            .map_err(RuntimeFault::into_diagnostic)?;
        if let Value::Future(future) = result {
            self.await_future(program, future, main.name_span)
                .map_err(RuntimeFault::into_diagnostic)?;
        }
        Ok(std::mem::take(
            &mut *self
                .output
                .lock()
                .expect("interpreter output lock poisoned"),
        ))
    }

    fn call_function(
        &mut self,
        program: &Program,
        function: &Function,
        arguments: Vec<Value>,
        call_span: Span,
    ) -> RuntimeResult<Value> {
        if function.asynchronous {
            return Ok(Value::Future(RuntimeFuture::new(
                function.clone(),
                arguments,
            )));
        }
        self.call_function_body(program, function, arguments, call_span)
    }

    fn call_function_body(
        &mut self,
        program: &Program,
        function: &Function,
        arguments: Vec<Value>,
        call_span: Span,
    ) -> RuntimeResult<Value> {
        if function.external.is_some() {
            return self.call_external(function, arguments, call_span);
        }
        // Keep the semantic oracle below the smallest supported test-thread stack.
        // Native DISP programs use the platform stack and are not capped here.
        const MAX_CALL_DEPTH: usize = 32;
        if function.parameters.len() != arguments.len() {
            return Err(self.error(
                format!(
                    "function `{}` expects {} arguments, found {}",
                    function.name,
                    function.parameters.len(),
                    arguments.len()
                ),
                call_span,
            ));
        }
        if self.call_depth >= MAX_CALL_DEPTH {
            return Err(self.error(
                format!("call depth exceeds the runtime limit of {MAX_CALL_DEPTH}"),
                call_span,
            ));
        }
        self.call_depth += 1;
        self.push_scope(HashMap::new());
        for (parameter, value) in function.parameters.iter().zip(arguments) {
            let value = coerce_value(value, &parameter.ty)
                .map_err(|message| self.error(message, call_span))?;
            self.scopes
                .last_mut()
                .unwrap()
                .insert(parameter.name.clone(), value);
            self.scope_orders
                .last_mut()
                .unwrap()
                .push(parameter.name.clone());
        }
        let flow = self.execute_block_contents(program, &function.body);
        self.pop_scope();
        self.call_depth -= 1;
        match flow {
            Err(RuntimeFault::Propagate(value)) => Ok(value),
            Err(error) => Err(error),
            Ok(Flow::Return(value)) => {
                if let Some(return_type) = &function.return_type {
                    coerce_value(value, return_type)
                        .map_err(|message| self.error(message, call_span))
                } else {
                    Ok(value)
                }
            }
            Ok(Flow::Normal) => Ok(Value::Unit),
            Ok(Flow::Break | Flow::Continue) => {
                Err(self.error("loop control escaped a function body", function.body.span))
            }
        }
    }

    fn await_future(
        &mut self,
        program: &Program,
        future: RuntimeFuture,
        span: Span,
    ) -> RuntimeResult<Value> {
        let work = future
            .0
            .lock()
            .map_err(|_| self.error("future state is poisoned", span))?
            .take()
            .ok_or_else(|| self.error("future has already been awaited", span))?;
        match work {
            FutureWork::Function(function, arguments) => {
                self.call_function_body(program, &function, arguments, span)
            }
            FutureWork::Yield => {
                self.progress_tasks(program, span)?;
                Ok(Value::Unit)
            }
            FutureWork::Sleep(duration) => {
                thread::sleep(duration);
                Ok(Value::Unit)
            }
            FutureWork::ReadText(path) => Ok(runtime_result(
                fs::read_to_string(path).map(|text| Value::String(RuntimeString::literal(text))),
            )),
            FutureWork::ReadBytes(path) => Ok(runtime_result(fs::read(path).map(|bytes| {
                let values = bytes
                    .into_iter()
                    .map(|value| Value::Unsigned(value as u128, 8))
                    .collect::<Vec<_>>();
                Value::List {
                    capacity: values.len(),
                    values,
                }
            }))),
            FutureWork::WriteText(path, text) => Ok(runtime_result(
                fs::write(path, text.text).map(|()| Value::Unit),
            )),
            FutureWork::WriteBytes(path, bytes) => {
                Ok(runtime_result(fs::write(path, bytes).map(|()| Value::Unit)))
            }
            FutureWork::Connect(address) => Ok(runtime_result(
                StdTcpStream::connect((address.host.as_str(), address.port)).map(|stream| {
                    Value::TcpStream(RuntimeTcpStream(Arc::new(StdMutex::new(Some(stream)))))
                }),
            )),
            FutureWork::Accept(listener, timeout) => {
                let deadline = timeout.and_then(|duration| StdInstant::now().checked_add(duration));
                loop {
                    let accepted = {
                        let guard = listener
                            .0
                            .lock()
                            .map_err(|_| self.error("TCP listener state is poisoned", span))?;
                        match guard.as_ref() {
                            Some(listener) => listener.accept(),
                            None => Err(std::io::Error::new(
                                std::io::ErrorKind::NotConnected,
                                "TCP listener is closed",
                            )),
                        }
                    };
                    match accepted {
                        Ok((stream, _)) => {
                            break Ok(runtime_result(Ok(Value::TcpStream(RuntimeTcpStream(
                                Arc::new(StdMutex::new(Some(stream))),
                            )))));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            if deadline.is_some_and(|deadline| StdInstant::now() >= deadline) {
                                break Ok(runtime_result(Err(std::io::Error::new(
                                    std::io::ErrorKind::TimedOut,
                                    "TCP accept timed out",
                                ))));
                            }
                            thread::sleep(StdDuration::from_millis(1));
                        }
                        Err(error) => break Ok(runtime_result(Err(error))),
                    }
                }
            }
        }
    }

    fn progress_tasks(&mut self, program: &Program, span: Span) -> RuntimeResult<()> {
        self.tasks.retain(|task| task.strong_count() > 0);
        let tasks = self.tasks.clone();
        for weak in tasks {
            let Some(state) = weak.upgrade() else {
                continue;
            };
            let work = {
                let mut work = state
                    .lock()
                    .map_err(|_| self.error("task state is poisoned", span))?;
                match &*work {
                    RuntimeTaskWork::Future(_) => {
                        std::mem::replace(&mut *work, RuntimeTaskWork::Running)
                    }
                    RuntimeTaskWork::Running
                    | RuntimeTaskWork::Ready(_)
                    | RuntimeTaskWork::Consumed => continue,
                }
            };
            let RuntimeTaskWork::Future(future) = work else {
                unreachable!()
            };
            match self.await_future(program, future, span) {
                Ok(value) => {
                    *state
                        .lock()
                        .map_err(|_| self.error("task state is poisoned", span))? =
                        RuntimeTaskWork::Ready(value);
                }
                Err(error) => {
                    if let Ok(mut work) = state.lock() {
                        *work = RuntimeTaskWork::Consumed;
                    }
                    return Err(error);
                }
            }
        }
        Ok(())
    }

    fn call_closure(
        &mut self,
        program: &Program,
        closure: RuntimeClosure,
        arguments: Vec<Value>,
        call_span: Span,
    ) -> RuntimeResult<Value> {
        if closure.parameters.len() != arguments.len() {
            return Err(self.error(
                format!(
                    "closure expects {} arguments, found {}",
                    closure.parameters.len(),
                    arguments.len()
                ),
                call_span,
            ));
        }
        const MAX_CALL_DEPTH: usize = 32;
        if self.call_depth >= MAX_CALL_DEPTH {
            return Err(self.error(
                format!("call depth exceeds the runtime limit of {MAX_CALL_DEPTH}"),
                call_span,
            ));
        }
        self.call_depth += 1;
        let captures = closure
            .captures
            .lock()
            .map_err(|_| self.error("closure capture state is poisoned", call_span))?
            .clone();
        self.push_scope(captures);
        self.push_scope(HashMap::new());
        for (parameter, value) in closure.parameters.iter().zip(arguments) {
            let value = coerce_value(value, &parameter.ty)
                .map_err(|message| self.error(message, call_span))?;
            self.scopes
                .last_mut()
                .unwrap()
                .insert(parameter.name.clone(), value);
            self.scope_orders
                .last_mut()
                .unwrap()
                .push(parameter.name.clone());
        }
        let flow = match &closure.body {
            crate::ast::ClosureBody::Expression(value) => {
                self.evaluate(program, value).map(Flow::Return)
            }
            crate::ast::ClosureBody::Block(block) => self.execute_block_contents(program, block),
        };
        self.pop_scope();
        let captures = self.scopes.pop().unwrap_or_default();
        self.scope_orders.pop();
        *closure
            .captures
            .lock()
            .map_err(|_| self.error("closure capture state is poisoned", call_span))? = captures;
        self.call_depth -= 1;
        let value = match flow {
            Err(RuntimeFault::Propagate(value)) => value,
            Err(error) => return Err(error),
            Ok(Flow::Return(value)) => value,
            Ok(Flow::Normal) => Value::Unit,
            Ok(Flow::Break | Flow::Continue) => {
                return Err(self.error("loop control escaped a closure body", call_span));
            }
        };
        if let Some(return_type) = &closure.return_type {
            coerce_value(value, return_type).map_err(|message| self.error(message, call_span))
        } else {
            Ok(value)
        }
    }

    fn call_external(
        &mut self,
        function: &Function,
        arguments: Vec<Value>,
        call_span: Span,
    ) -> RuntimeResult<Value> {
        if function.parameters.len() != arguments.len() {
            return Err(self.error(
                format!(
                    "external function `{}` expects {} arguments, found {}",
                    function.name,
                    function.parameters.len(),
                    arguments.len()
                ),
                call_span,
            ));
        }
        let link_name = function
            .external
            .as_ref()
            .map_or(function.name.as_str(), |external| {
                external.link_name.as_str()
            });
        match link_name {
            "abs" => {
                let value = match arguments.into_iter().next().unwrap() {
                    Value::Signed(value, 32) => i32::try_from(value).ok(),
                    Value::Int(value) => i32::try_from(value).ok(),
                    _ => None,
                }
                .ok_or_else(|| self.error("C abs expects CInt", call_span))?;
                let value = value
                    .checked_abs()
                    .ok_or_else(|| self.error("C abs overflow", call_span))?;
                Ok(Value::Signed(value.into(), 32))
            }
            "strlen" => {
                let value = match arguments.into_iter().next().unwrap() {
                    Value::CStr(value) | Value::CString(value) => value,
                    _ => return Err(self.error("C strlen expects CStr", call_span)),
                };
                Ok(Value::UInt(value.len() as u64))
            }
            "sqrt" => {
                let value = arguments.into_iter().next().unwrap();
                let value = numeric_as_f64(&value)
                    .ok_or_else(|| self.error("C sqrt expects CDouble", call_span))?;
                Ok(Value::Float(value.sqrt()))
            }
            name => Err(self.error(
                format!("external C function `{name}` requires native execution"),
                call_span,
            )),
        }
    }

    fn execute_block(&mut self, program: &Program, block: &Block) -> RuntimeResult<Flow> {
        self.push_scope(HashMap::new());
        let result = self.execute_block_contents(program, block);
        self.pop_scope();
        result
    }

    fn execute_block_contents(&mut self, program: &Program, block: &Block) -> RuntimeResult<Flow> {
        for statement in &block.statements {
            let flow = self.execute_statement(program, &statement.node, statement.span)?;
            if !matches!(flow, Flow::Normal) {
                return Ok(flow);
            }
        }
        Ok(Flow::Normal)
    }

    fn execute_statement(
        &mut self,
        program: &Program,
        statement: &Statement,
        span: Span,
    ) -> RuntimeResult<Flow> {
        match statement {
            Statement::Binding {
                name,
                annotation,
                value,
                ..
            } => {
                let mut value = value
                    .as_ref()
                    .map(|value| self.consume(program, value))
                    .transpose()?
                    .unwrap_or(Value::Uninitialized);
                if let Some(annotation) = annotation
                    && !matches!(value, Value::Uninitialized)
                {
                    value = coerce_value(value, annotation)
                        .map_err(|message| self.error(message, span))?;
                }
                self.scopes.last_mut().unwrap().insert(name.clone(), value);
                self.scope_orders.last_mut().unwrap().push(name.clone());
                Ok(Flow::Normal)
            }
            Statement::Assignment {
                name,
                operator,
                value,
                ..
            } => {
                let mut right = self.consume(program, value)?;
                if let Some(existing) = self.lookup(name).and_then(|value| match value {
                    Value::CaptureReference(place, _) => self.read_place(&place),
                    value => Some(value),
                }) {
                    right = coerce_like(right, &existing)
                        .map_err(|message| self.error(message, span))?;
                }
                let new_value = if *operator == AssignmentOperator::Assign {
                    right
                } else {
                    let left = self
                        .lookup(name)
                        .and_then(|value| match value {
                            Value::CaptureReference(place, _) => self.read_place(&place),
                            value => Some(value),
                        })
                        .ok_or_else(|| self.error(format!("undefined variable `{name}`"), span))?;
                    let binary = match operator {
                        AssignmentOperator::Add => BinaryOperator::Add,
                        AssignmentOperator::Subtract => BinaryOperator::Subtract,
                        AssignmentOperator::Multiply => BinaryOperator::Multiply,
                        AssignmentOperator::Divide => BinaryOperator::Divide,
                        AssignmentOperator::Assign => unreachable!(),
                    };
                    self.evaluate_binary(binary, left, right, span)?
                };
                if self.assign(name, new_value.clone()).is_none() {
                    if *operator != AssignmentOperator::Assign {
                        return Err(self.error(format!("undefined variable `{name}`"), span));
                    }
                    self.scopes
                        .last_mut()
                        .unwrap()
                        .insert(name.clone(), new_value);
                    self.scope_orders.last_mut().unwrap().push(name.clone());
                }
                Ok(Flow::Normal)
            }
            Statement::PlaceAssignment {
                target,
                operator,
                value,
            } => {
                let place = self
                    .dynamic_place(program, target)?
                    .ok_or_else(|| self.error("assignment target is not a place", target.span))?;
                let right = self.consume(program, value)?;
                let new_value = if *operator == AssignmentOperator::Assign {
                    right
                } else {
                    let left = self
                        .read_place(&place)
                        .ok_or_else(|| self.error("invalid assignment target", target.span))?;
                    self.evaluate_binary(
                        match operator {
                            AssignmentOperator::Add => BinaryOperator::Add,
                            AssignmentOperator::Subtract => BinaryOperator::Subtract,
                            AssignmentOperator::Multiply => BinaryOperator::Multiply,
                            AssignmentOperator::Divide => BinaryOperator::Divide,
                            AssignmentOperator::Assign => unreachable!(),
                        },
                        left,
                        right,
                        span,
                    )?
                };
                self.write_place(&place, new_value)
                    .ok_or_else(|| self.error("invalid assignment target", target.span))?;
                Ok(Flow::Normal)
            }
            Statement::Expression(expression) => {
                self.evaluate(program, expression)?;
                Ok(Flow::Normal)
            }
            Statement::Return(value) => {
                let value = value
                    .as_ref()
                    .map(|expression| self.consume(program, expression))
                    .transpose()?
                    .unwrap_or(Value::Unit);
                Ok(Flow::Return(value))
            }
            Statement::If {
                condition,
                then_branch,
                else_branch,
            } => match self.evaluate(program, condition)? {
                Value::Bool(true) => self.execute_block(program, then_branch),
                Value::Bool(false) => {
                    if let Some(branch) = else_branch {
                        self.execute_block(program, branch)
                    } else {
                        Ok(Flow::Normal)
                    }
                }
                _ => Err(self.error("if condition did not evaluate to Bool", condition.span)),
            },
            Statement::While { condition, body } => {
                loop {
                    match self.evaluate(program, condition)? {
                        Value::Bool(true) => {}
                        Value::Bool(false) => break,
                        _ => {
                            return Err(self.error(
                                "while condition did not evaluate to Bool",
                                condition.span,
                            ));
                        }
                    }
                    match self.execute_block(program, body)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                }
                Ok(Flow::Normal)
            }
            Statement::For {
                name,
                start,
                end,
                inclusive,
                body,
                ..
            } => {
                let Value::Int(mut current) = self.evaluate(program, start)? else {
                    return Err(self.error("range start did not evaluate to int", start.span));
                };
                let Value::Int(end) = self.evaluate(program, end)? else {
                    return Err(self.error("range end did not evaluate to int", end.span));
                };
                while if *inclusive {
                    current <= end
                } else {
                    current < end
                } {
                    self.push_scope(HashMap::from([(name.clone(), Value::Int(current))]));
                    let flow = self.execute_block_contents(program, body);
                    self.pop_scope();
                    match flow? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                    if *inclusive && current == end {
                        break;
                    }
                    current = current.checked_add(1).ok_or_else(|| {
                        self.error("integer overflow while advancing for-loop range", span)
                    })?;
                }
                Ok(Flow::Normal)
            }
            Statement::ForEach {
                name,
                iterable,
                body,
                ..
            } => {
                let iterable = match self.evaluate(program, iterable)? {
                    Value::Reference(place, _) => self
                        .read_place(&place)
                        .ok_or_else(|| self.error("dangling iterable reference", iterable.span))?,
                    value => value,
                };
                let values = match iterable {
                    Value::Array(values) | Value::Slice(values) => values,
                    Value::List { values, .. } | Value::Set { values, .. } => values,
                    _ => {
                        return Err(self.error("iteration requires an array, slice, or List", span));
                    }
                };
                for value in values {
                    self.push_scope(HashMap::from([(name.clone(), value)]));
                    let flow = self.execute_block_contents(program, body);
                    self.pop_scope();
                    match flow? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                }
                Ok(Flow::Normal)
            }
            Statement::Loop(body) => {
                loop {
                    match self.execute_block(program, body)? {
                        Flow::Normal | Flow::Continue => {}
                        Flow::Break => break,
                        returned @ Flow::Return(_) => return Ok(returned),
                    }
                }
                Ok(Flow::Normal)
            }
            Statement::Unsafe(body) => self.execute_block(program, body),
            Statement::Break => Ok(Flow::Break),
            Statement::Continue => Ok(Flow::Continue),
        }
    }

    fn evaluate(&mut self, program: &Program, expression: &Expr) -> RuntimeResult<Value> {
        match &expression.node {
            Expression::Integer(value) => {
                if *value <= i64::MAX as u128 {
                    Ok(Value::Int(*value as i64))
                } else {
                    Ok(Value::Unsigned(*value, 128))
                }
            }
            Expression::Float(value) => Ok(Value::Float(*value)),
            Expression::String(value) => Ok(Value::String(RuntimeString::literal(value.clone()))),
            Expression::Array(values) => values
                .iter()
                .map(|value| self.consume(program, value))
                .collect::<Result<Vec<_>, _>>()
                .map(Value::Array),
            Expression::Closure {
                move_captures,
                parameters,
                return_type,
                body,
            } => {
                let mut captures = HashMap::new();
                for (name, usage) in crate::ast::closure_capture_uses(parameters, body) {
                    let Some(place) = self.expression_place(&crate::ast::Spanned {
                        node: Expression::Identifier(name.clone()),
                        span: usage.span,
                    }) else {
                        continue;
                    };
                    let value = if *move_captures {
                        let value = self.read_place(&place).ok_or_else(|| {
                            self.error(format!("invalid capture `{name}`"), usage.span)
                        })?;
                        if !value_is_copy(program, &value) {
                            self.write_place(&place, Value::Uninitialized)
                                .ok_or_else(|| {
                                    self.error(format!("invalid capture `{name}`"), usage.span)
                                })?;
                        }
                        value
                    } else {
                        Value::CaptureReference(place, usage.mutated)
                    };
                    captures.insert(name, value);
                }
                Ok(Value::Closure(Box::new(RuntimeClosure {
                    parameters: parameters.clone(),
                    return_type: return_type.clone(),
                    body: body.clone(),
                    captures: Arc::new(StdMutex::new(captures)),
                })))
            }
            Expression::Index { object, index } => {
                let values = self.evaluate(program, object)?;
                let index_value = self.evaluate(program, index)?;
                let index = match index_value {
                    Value::Int(value) if value >= 0 => value as usize,
                    Value::UInt(value) => value as usize,
                    Value::Signed(value, _) if value >= 0 => value as usize,
                    Value::Unsigned(value, _) => usize::try_from(value)
                        .map_err(|_| self.error("array index is too large", index.span))?,
                    _ => {
                        return Err(
                            self.error("array index must be a non-negative integer", index.span)
                        );
                    }
                };
                let values = match values {
                    Value::Array(values) | Value::Slice(values) => values,
                    Value::List { values, .. } => values,
                    _ => return Err(self.error("value is not indexable", object.span)),
                };
                values.get(index).cloned().ok_or_else(|| {
                    self.error(
                        format!(
                            "array index {index} is out of bounds for length {}",
                            values.len()
                        ),
                        expression.span,
                    )
                })
            }
            Expression::Subslice { object, start, end } => {
                let values = self.evaluate(program, object)?;
                let start = self.index_value(program, start)?;
                let end = self.index_value(program, end)?;
                let values = match values {
                    Value::Array(values) | Value::Slice(values) => values,
                    Value::List { values, .. } => values,
                    Value::String(value) => {
                        if start > end
                            || end > value.text.len()
                            || !value.text.is_char_boundary(start)
                            || !value.text.is_char_boundary(end)
                        {
                            return Err(self.error(
                                "string slice is out of bounds or not on UTF-8 boundaries",
                                expression.span,
                            ));
                        }
                        return Ok(Value::String(RuntimeString::literal(
                            value.text[start..end].to_owned(),
                        )));
                    }
                    _ => return Err(self.error("value is not sliceable", object.span)),
                };
                if start > end || end > values.len() {
                    return Err(self.error(
                        format!(
                            "subslice range {start}..{end} is out of bounds for length {}",
                            values.len()
                        ),
                        expression.span,
                    ));
                }
                Ok(Value::Slice(values[start..end].to_vec()))
            }
            Expression::Character(value) => Ok(Value::Char(*value)),
            Expression::Bool(value) => Ok(Value::Bool(*value)),
            Expression::StructConstruct { name, fields, .. } => {
                let mut values = HashMap::new();
                let declaration = program
                    .structs
                    .iter()
                    .find(|declaration| declaration.name == *name);
                for field in fields {
                    let mut value = self.consume(program, &field.value)?;
                    if let Some(ty) = declaration.and_then(|declaration| {
                        declaration
                            .fields
                            .iter()
                            .find(|candidate| candidate.name == field.name)
                            .map(|field| &field.ty)
                    }) {
                        value = coerce_value(value, ty)
                            .map_err(|message| self.error(message, field.value.span))?;
                    }
                    values.insert(field.name.clone(), value);
                }
                Ok(Value::Struct {
                    type_name: name.clone(),
                    fields: values,
                })
            }
            Expression::Identifier(name) => {
                if let Some(value) = self.lookup(name) {
                    if let Value::CaptureReference(place, _) = value {
                        return self.read_place(&place).ok_or_else(|| {
                            self.error(format!("dangling capture `{name}`"), expression.span)
                        });
                    }
                    return Ok(value);
                }
                if program
                    .functions
                    .iter()
                    .any(|function| function.name == *name)
                {
                    return Ok(Value::Function(name.clone()));
                }
                match name.as_str() {
                    "None" => {
                        return Ok(Value::Enum {
                            type_name: "Option".into(),
                            variant: "None".into(),
                            payload: vec![],
                        });
                    }
                    "Some" | "Ok" | "Err" => {
                        return Ok(Value::Constructor {
                            type_name: if name == "Some" { "Option" } else { "Result" }.into(),
                            variant: name.clone(),
                        });
                    }
                    _ => {}
                }
                let candidates = find_variants(program, name);
                if candidates.len() == 1 {
                    let (owner, variant) = candidates[0];
                    if variant.payload.is_empty() {
                        return Ok(Value::Enum {
                            type_name: owner.name.clone(),
                            variant: variant.name.clone(),
                            payload: vec![],
                        });
                    }
                    return Ok(Value::Constructor {
                        type_name: owner.name.clone(),
                        variant: variant.name.clone(),
                    });
                }
                Err(self.error(format!("undefined name `{name}`"), expression.span))
            }
            Expression::FieldAccess { object, field, .. } => {
                if let Expression::Identifier(type_name) = &object.node
                    && let Some((owner, variant)) =
                        find_qualified_variant(program, type_name, field)
                {
                    if variant.payload.is_empty() {
                        return Ok(Value::Enum {
                            type_name: owner.name.clone(),
                            variant: variant.name.clone(),
                            payload: vec![],
                        });
                    }
                    return Ok(Value::Constructor {
                        type_name: owner.name.clone(),
                        variant: variant.name.clone(),
                    });
                }
                let value = match self.evaluate(program, object)? {
                    Value::Reference(place, _) => self
                        .read_place(&place)
                        .ok_or_else(|| self.error("dangling reference", object.span))?,
                    value => value,
                };
                match value {
                    Value::Struct { fields, .. } => fields.get(field).cloned().ok_or_else(|| {
                        self.error(format!("struct has no field `{field}`"), expression.span)
                    }),
                    _ => Err(self.error("field access requires a struct", object.span)),
                }
            }
            Expression::Unary { operator, operand } => {
                let value = self.evaluate(program, operand)?;
                match (operator, value) {
                    (UnaryOperator::Negate, Value::Int(value)) => {
                        value.checked_neg().map(Value::Int).ok_or_else(|| {
                            self.error("integer overflow in unary `-`", expression.span)
                        })
                    }
                    (UnaryOperator::Negate, Value::Signed(value, width)) => {
                        let negated = value.checked_neg().filter(|value| {
                            width == 128
                                || (-(1_i128 << (width - 1))..=(1_i128 << (width - 1)) - 1)
                                    .contains(value)
                        });
                        negated
                            .map(|value| Value::Signed(value, width))
                            .ok_or_else(|| {
                                self.error(
                                    format!("i{width} overflow in negation"),
                                    expression.span,
                                )
                            })
                    }
                    (UnaryOperator::Negate, Value::Unsigned(value, 128))
                        if value <= i128::MAX as u128 =>
                    {
                        Ok(Value::Signed(-(value as i128), 128))
                    }
                    (UnaryOperator::Negate, Value::Unsigned(value, 128))
                        if value == (1_u128 << 127) =>
                    {
                        Ok(Value::Signed(i128::MIN, 128))
                    }
                    (UnaryOperator::Negate, Value::Float(value)) => Ok(Value::Float(-value)),
                    (UnaryOperator::Not, Value::Bool(value)) => Ok(Value::Bool(!value)),
                    _ => Err(self.error("invalid operand for unary operator", expression.span)),
                }
            }
            Expression::Binary {
                left,
                operator: BinaryOperator::And,
                right,
            } => match self.evaluate(program, left)? {
                Value::Bool(false) => Ok(Value::Bool(false)),
                Value::Bool(true) => match self.evaluate(program, right)? {
                    Value::Bool(value) => Ok(Value::Bool(value)),
                    _ => Err(self.error("right operand of `&&` is not Bool", right.span)),
                },
                _ => Err(self.error("left operand of `&&` is not Bool", left.span)),
            },
            Expression::Binary {
                left,
                operator: BinaryOperator::Or,
                right,
            } => match self.evaluate(program, left)? {
                Value::Bool(true) => Ok(Value::Bool(true)),
                Value::Bool(false) => match self.evaluate(program, right)? {
                    Value::Bool(value) => Ok(Value::Bool(value)),
                    _ => Err(self.error("right operand of `||` is not Bool", right.span)),
                },
                _ => Err(self.error("left operand of `||` is not Bool", left.span)),
            },
            Expression::Binary {
                left,
                operator,
                right,
            } => {
                let left = self.evaluate(program, left)?;
                let right_span = right.span;
                let right = self.evaluate(program, right)?;
                let (left, right) = coerce_numeric_pair(left, right)
                    .map_err(|message| self.error(message, right_span))?;
                self.evaluate_binary(*operator, left, right, expression.span)
            }
            Expression::Call { callee, arguments } => {
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "AtomicInt")
                    && field == "new"
                {
                    let Value::Int(value) = self.evaluate(program, &arguments[0])? else {
                        unreachable!("type checking validates AtomicInt.new")
                    };
                    return Ok(Value::AtomicInt(RuntimeAtomicInt(Arc::new(
                        AtomicI64::new(value),
                    ))));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Mutex")
                    && field == "new"
                {
                    return Ok(Value::Mutex(RuntimeMutex::new(
                        self.consume(program, &arguments[0])?,
                    )));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "TcpListener")
                    && field == "bind"
                {
                    let address = self.consume(program, &arguments[0])?;
                    let Value::SocketAddress(address) = address else {
                        return Err(
                            self.error("TcpListener.bind expects SocketAddress", arguments[0].span)
                        );
                    };
                    let bound = StdTcpListener::bind((address.host.as_str(), address.port))
                        .and_then(|listener| {
                            listener.set_nonblocking(true)?;
                            Ok(Value::TcpListener(RuntimeTcpListener(Arc::new(
                                StdMutex::new(Some(listener)),
                            ))))
                        });
                    return Ok(runtime_result(bound));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "String") {
                    return Ok(Value::String(RuntimeString::with_capacity(0)));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "SocketAddress") {
                    let host = self.evaluate(program, &arguments[0])?;
                    let Value::String(host) = host else {
                        return Err(self.error(
                            "SocketAddress host must be String or str",
                            arguments[0].span,
                        ));
                    };
                    if host.text.is_empty() {
                        return Err(self.error("socket host cannot be empty", arguments[0].span));
                    }
                    if host.text.contains('\0') {
                        return Err(
                            self.error("socket host cannot contain a NUL byte", arguments[0].span)
                        );
                    }
                    let port = self.index_value(program, &arguments[1])?;
                    let port = u16::try_from(port).map_err(|_| {
                        self.error("socket port is outside 0 through 65535", arguments[1].span)
                    })?;
                    return Ok(Value::SocketAddress(RuntimeSocketAddress {
                        host: host.text,
                        port,
                    }));
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "Path") {
                    let value = self.evaluate(program, &arguments[0])?;
                    let path = runtime_path(value).ok_or_else(|| {
                        self.error("Path source must be String or str", arguments[0].span)
                    })?;
                    if path.as_os_str().to_string_lossy().contains('\0') {
                        return Err(self.error("Path cannot contain a NUL byte", arguments[0].span));
                    }
                    return Ok(Value::Path(path));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "String")
                {
                    return match field.as_str() {
                        "new" => Ok(Value::String(RuntimeString::with_capacity(0))),
                        "with_capacity" => {
                            let value = self.evaluate(program, &arguments[0])?;
                            let capacity = match value {
                                Value::Int(x) if x >= 0 => x as usize,
                                Value::UInt(x) => x as usize,
                                _ => {
                                    return Err(self.error(
                                        "capacity must be a non-negative integer",
                                        arguments[0].span,
                                    ));
                                }
                            };
                            Ok(Value::String(RuntimeString::with_capacity(capacity)))
                        }
                        _ => Err(self.error("unknown String constructor", expression.span)),
                    };
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "CString")
                    && field == "new"
                {
                    let source = self.evaluate(program, &arguments[0])?;
                    let bytes = match source {
                        Value::String(value) => value.text.into_bytes(),
                        Value::CStr(value) | Value::CString(value) => value.text().into_bytes(),
                        _ => unreachable!("type checking validates CString source"),
                    };
                    return Ok(match RuntimeCString::new(&bytes) {
                        Ok(value) => Value::Enum {
                            type_name: "Result".into(),
                            variant: "Ok".into(),
                            payload: vec![Value::CString(value)],
                        },
                        Err(message) => Value::Enum {
                            type_name: "Result".into(),
                            variant: "Err".into(),
                            payload: vec![Value::String(RuntimeString::literal(message.into()))],
                        },
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Memory")
                    && field == "allocate"
                {
                    let size = self.index_value(program, &arguments[0])?;
                    let alignment = self.index_value(program, &arguments[1])?;
                    return Ok(match RuntimeMemory::new(size, alignment) {
                        Ok(memory) => Value::Enum {
                            type_name: "Result".into(),
                            variant: "Ok".into(),
                            payload: vec![Value::Memory(memory)],
                        },
                        Err(message) => Value::Enum {
                            type_name: "Result".into(),
                            variant: "Err".into(),
                            payload: vec![Value::String(RuntimeString::literal(message.into()))],
                        },
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "List")
                {
                    if field == "of" {
                        let mut values = Vec::with_capacity(arguments.len());
                        for argument in arguments {
                            values.push(self.consume(program, argument)?);
                        }
                        return Ok(Value::List {
                            capacity: values.len(),
                            values,
                        });
                    }
                    let capacity = if field == "with_capacity" {
                        self.index_value(program, &arguments[0])?
                    } else {
                        0
                    };
                    return Ok(Value::List {
                        values: Vec::new(),
                        capacity,
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Map")
                {
                    if field == "of" {
                        let mut entries = Vec::with_capacity(arguments.len() / 2);
                        for pair in arguments.chunks_exact(2) {
                            let key = self.consume(program, &pair[0])?;
                            let value = self.consume(program, &pair[1])?;
                            if let Some(entry) =
                                entries.iter_mut().find(|(candidate, _)| candidate == &key)
                            {
                                entry.1 = value;
                            } else {
                                entries.push((key, value));
                            }
                        }
                        return Ok(Value::Map {
                            capacity: entries.len(),
                            entries,
                        });
                    }
                    let capacity = if field == "with_capacity" {
                        self.index_value(program, &arguments[0])?
                    } else {
                        0
                    };
                    return Ok(Value::Map {
                        entries: Vec::new(),
                        capacity,
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Set")
                {
                    if field == "of" {
                        let mut values = Vec::with_capacity(arguments.len());
                        for argument in arguments {
                            let value = self.consume(program, argument)?;
                            if !values.contains(&value) {
                                values.push(value);
                            }
                        }
                        return Ok(Value::Set {
                            capacity: values.len(),
                            values,
                        });
                    }
                    let capacity = if field == "with_capacity" {
                        self.index_value(program, &arguments[0])?
                    } else {
                        0
                    };
                    return Ok(Value::Set {
                        values: Vec::new(),
                        capacity,
                    });
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(&object.node, Expression::Identifier(name) if name == "Async")
                {
                    return match field.as_str() {
                        "yield" => Ok(Value::Future(RuntimeFuture::yielding())),
                        "spawn" => {
                            let value = self.consume(program, &arguments[0])?;
                            let Value::Future(future) = value else {
                                return Err(
                                    self.error("Async.spawn requires a Future", arguments[0].span)
                                );
                            };
                            let task = RuntimeTask::new(future);
                            self.tasks.push(Arc::downgrade(&task.0));
                            Ok(Value::Task(task))
                        }
                        "sleep" => {
                            let Value::Duration(duration) =
                                self.evaluate(program, &arguments[0])?
                            else {
                                return Err(
                                    self.error("Async.sleep expects Duration", arguments[0].span)
                                );
                            };
                            Ok(Value::Future(RuntimeFuture::operation(FutureWork::Sleep(
                                duration,
                            ))))
                        }
                        "connect" => {
                            let value = self.consume(program, &arguments[0])?;
                            let Value::SocketAddress(address) = value else {
                                return Err(self.error(
                                    "Async.connect expects SocketAddress",
                                    arguments[0].span,
                                ));
                            };
                            Ok(Value::Future(RuntimeFuture::operation(
                                FutureWork::Connect(address),
                            )))
                        }
                        "read_text" | "read_bytes" => {
                            let value = self.consume(program, &arguments[0])?;
                            let Value::Path(path) = value else {
                                return Err(self.error(
                                    "async file operation expects Path",
                                    arguments[0].span,
                                ));
                            };
                            let work = if field == "read_text" {
                                FutureWork::ReadText(path)
                            } else {
                                FutureWork::ReadBytes(path)
                            };
                            Ok(Value::Future(RuntimeFuture::operation(work)))
                        }
                        "write_text" | "write_bytes" => {
                            let path = self.consume(program, &arguments[0])?;
                            let Value::Path(path) = path else {
                                return Err(self.error(
                                    "async file operation expects Path",
                                    arguments[0].span,
                                ));
                            };
                            let value = self.consume(program, &arguments[1])?;
                            let work = if field == "write_text" {
                                let Value::String(text) = value else {
                                    return Err(self.error(
                                        "Async.write_text expects owned String",
                                        arguments[1].span,
                                    ));
                                };
                                FutureWork::WriteText(path, text)
                            } else {
                                let Value::List { values, .. } = value else {
                                    return Err(self.error(
                                        "Async.write_bytes expects owned List<u8>",
                                        arguments[1].span,
                                    ));
                                };
                                let mut bytes = Vec::with_capacity(values.len());
                                for value in values {
                                    let Value::Unsigned(value, 8) = value else {
                                        return Err(self.error(
                                            "Async.write_bytes expects owned List<u8>",
                                            arguments[1].span,
                                        ));
                                    };
                                    bytes.push(value as u8);
                                }
                                FutureWork::WriteBytes(path, bytes)
                            };
                            Ok(Value::Future(RuntimeFuture::operation(work)))
                        }
                        _ => Err(self.error("unknown Async operation", callee.span)),
                    };
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && let Expression::Identifier(owner) = &object.node
                    && matches!(
                        owner.as_str(),
                        "Path" | "File" | "Directory" | "Time" | "Duration"
                    )
                {
                    if owner == "Path" {
                        let value = self.evaluate(program, &arguments[0])?;
                        let path = runtime_path(value).ok_or_else(|| {
                            self.error("Path source must be String or str", arguments[0].span)
                        })?;
                        if path.as_os_str().to_string_lossy().contains('\0') {
                            return Err(
                                self.error("Path cannot contain a NUL byte", arguments[0].span)
                            );
                        }
                        return Ok(Value::Path(path));
                    }
                    if owner == "Time" {
                        return match field.as_str() {
                            "now" => Ok(Value::Instant(StdInstant::now())),
                            "unix_seconds" => Ok(Value::UInt(
                                SystemTime::now()
                                    .duration_since(UNIX_EPOCH)
                                    .unwrap_or_default()
                                    .as_secs(),
                            )),
                            "sleep" => {
                                let Value::Duration(duration) =
                                    self.evaluate(program, &arguments[0])?
                                else {
                                    return Err(
                                        self.error("sleep expects Duration", arguments[0].span)
                                    );
                                };
                                thread::sleep(duration);
                                Ok(Value::Unit)
                            }
                            _ => Err(self.error("unknown Time operation", expression.span)),
                        };
                    }
                    if owner == "Duration" {
                        let amount = self.index_value(program, &arguments[0])? as u64;
                        let factor = match field.as_str() {
                            "from_nanos" => 1,
                            "from_millis" => 1_000_000,
                            _ => 1_000_000_000,
                        };
                        let nanos = amount
                            .checked_mul(factor)
                            .ok_or_else(|| self.error("Duration overflow", expression.span))?;
                        return Ok(Value::Duration(StdDuration::from_nanos(nanos)));
                    }
                    let path =
                        runtime_path(self.evaluate(program, &arguments[0])?).ok_or_else(|| {
                            self.error("filesystem path must be Path", arguments[0].span)
                        })?;
                    if owner == "File" {
                        return match field.as_str() {
                            "read_text" => Ok(runtime_result(
                                fs::read_to_string(path)
                                    .map(|text| Value::String(RuntimeString::literal(text))),
                            )),
                            "read_bytes" => Ok(runtime_result(fs::read(path).map(|bytes| {
                                let values = bytes
                                    .into_iter()
                                    .map(|value| Value::Unsigned(value as u128, 8))
                                    .collect::<Vec<_>>();
                                Value::List {
                                    capacity: values.len(),
                                    values,
                                }
                            }))),
                            "size" => Ok(runtime_result(
                                fs::metadata(path).map(|metadata| Value::UInt(metadata.len())),
                            )),
                            "modified_seconds" => Ok(runtime_result(
                                fs::metadata(path)
                                    .and_then(|metadata| metadata.modified())
                                    .and_then(|modified| {
                                        modified
                                            .duration_since(UNIX_EPOCH)
                                            .map_err(std::io::Error::other)
                                    })
                                    .map(|duration| Value::UInt(duration.as_secs())),
                            )),
                            "write_text" | "append_text" => {
                                let Value::String(text) = self.evaluate(program, &arguments[1])?
                                else {
                                    return Err(self.error(
                                        "file text must be String or str",
                                        arguments[1].span,
                                    ));
                                };
                                let result = if field == "append_text" {
                                    use std::io::Write;
                                    fs::OpenOptions::new()
                                        .create(true)
                                        .append(true)
                                        .open(path)
                                        .and_then(|mut file| file.write_all(text.text.as_bytes()))
                                } else {
                                    fs::write(path, text.text)
                                };
                                Ok(runtime_result(result.map(|_| Value::Unit)))
                            }
                            "write_bytes" | "append_bytes" => {
                                let values = self.evaluate(program, &arguments[1])?;
                                let values = match values {
                                    Value::List { values, .. } | Value::Slice(values) => values,
                                    _ => {
                                        return Err(self.error(
                                            "file bytes must be List<u8> or a u8 slice",
                                            arguments[1].span,
                                        ));
                                    }
                                };
                                let bytes =
                                    values
                                        .into_iter()
                                        .map(|value| match value {
                                            Value::Unsigned(value, 8) => Ok(value as u8),
                                            _ => Err(self
                                                .error("file byte is not u8", arguments[1].span)),
                                        })
                                        .collect::<Result<Vec<_>, _>>()?;
                                let result = if field == "append_bytes" {
                                    use std::io::Write;
                                    fs::OpenOptions::new()
                                        .create(true)
                                        .append(true)
                                        .open(path)
                                        .and_then(|mut file| file.write_all(&bytes))
                                } else {
                                    fs::write(path, bytes)
                                };
                                Ok(runtime_result(result.map(|_| Value::Unit)))
                            }
                            "exists" => Ok(Value::Bool(path.is_file())),
                            "remove" => {
                                Ok(runtime_result(fs::remove_file(path).map(|_| Value::Unit)))
                            }
                            "copy" => {
                                let to = runtime_path(self.evaluate(program, &arguments[1])?)
                                    .ok_or_else(|| {
                                        self.error(
                                            "filesystem path must be Path",
                                            arguments[1].span,
                                        )
                                    })?;
                                Ok(runtime_result(fs::copy(path, to).map(|_| Value::Unit)))
                            }
                            "move" => {
                                let to = runtime_path(self.evaluate(program, &arguments[1])?)
                                    .ok_or_else(|| {
                                        self.error(
                                            "filesystem path must be Path",
                                            arguments[1].span,
                                        )
                                    })?;
                                Ok(runtime_result(fs::rename(path, to).map(|_| Value::Unit)))
                            }
                            _ => Err(self.error("unknown File operation", expression.span)),
                        };
                    }
                    return match field.as_str() {
                        "exists" => Ok(Value::Bool(path.is_dir())),
                        "create" => Ok(runtime_result(fs::create_dir(path).map(|_| Value::Unit))),
                        "create_all" => Ok(runtime_result(
                            fs::create_dir_all(path).map(|_| Value::Unit),
                        )),
                        "remove" => Ok(runtime_result(fs::remove_dir(path).map(|_| Value::Unit))),
                        "read" => Ok(runtime_result(
                            fs::read_dir(path)
                                .and_then(|entries| {
                                    entries
                                        .map(|entry| entry.map(|entry| Value::Path(entry.path())))
                                        .collect::<Result<Vec<_>, _>>()
                                })
                                .map(|values| Value::List {
                                    capacity: values.len(),
                                    values,
                                }),
                        )),
                        _ => Err(self.error("unknown Directory operation", expression.span)),
                    };
                }
                if let Expression::Identifier(name) = &callee.node
                    && is_numeric_name(name)
                {
                    let value = self.evaluate(program, &arguments[0])?;
                    return coerce_value(
                        value,
                        &crate::ast::TypeName {
                            name: name.clone(),
                            arguments: vec![],
                            qualifier: crate::ast::TypeQualifier::Owned,
                            span: callee.span,
                        },
                    )
                    .map_err(|message| self.error(message, expression.span));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && let Expression::Identifier(name) = &object.node
                    && is_numeric_name(name)
                    && field == "try_from"
                {
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::Map {
                            mut entries,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let field = match field.as_str() {
                            "count" => "len",
                            "empty" => "is_empty",
                            "contains_key" => "has",
                            "insert" => "set",
                            other => other,
                        };
                        match field {
                            "len" => return Ok(Value::UInt(entries.len() as u64)),
                            "capacity" => return Ok(Value::UInt(capacity as u64)),
                            "is_empty" => return Ok(Value::Bool(entries.is_empty())),
                            "has" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                return Ok(Value::Bool(
                                    entries.iter().any(|(candidate, _)| candidate == &key),
                                ));
                            }
                            "get" | "get_mut" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                return Ok(
                                    match entries
                                        .iter()
                                        .position(|(candidate, _)| candidate == &key)
                                    {
                                        Some(index) => {
                                            let mut value = place.clone();
                                            value.fields.push(PlaceSegment::MapValue(index));
                                            Value::Enum {
                                                type_name: "Option".into(),
                                                variant: "Some".into(),
                                                payload: vec![Value::Reference(
                                                    value,
                                                    field == "get_mut",
                                                )],
                                            }
                                        }
                                        None => Value::Enum {
                                            type_name: "Option".into(),
                                            variant: "None".into(),
                                            payload: vec![],
                                        },
                                    },
                                );
                            }
                            "set" => {
                                let key = self.consume(program, &arguments[0])?;
                                let value = self.consume(program, &arguments[1])?;
                                let old = if let Some(entry) =
                                    entries.iter_mut().find(|(candidate, _)| candidate == &key)
                                {
                                    Some(std::mem::replace(&mut entry.1, value))
                                } else {
                                    grow_list_capacity(&mut capacity, entries.len() + 1)
                                        .map_err(|message| self.error(message, expression.span))?;
                                    entries.push((key, value));
                                    None
                                };
                                self.write_place(&place, Value::Map { entries, capacity })
                                    .ok_or_else(|| {
                                        self.error("Map mutation target is invalid", object.span)
                                    })?;
                                return Ok(match old {
                                    Some(value) => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "Some".into(),
                                        payload: vec![value],
                                    },
                                    None => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "None".into(),
                                        payload: vec![],
                                    },
                                });
                            }
                            "remove" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                let removed = entries
                                    .iter()
                                    .position(|(candidate, _)| candidate == &key)
                                    .map(|index| entries.remove(index).1);
                                self.write_place(&place, Value::Map { entries, capacity })
                                    .ok_or_else(|| {
                                        self.error("Map mutation target is invalid", object.span)
                                    })?;
                                return Ok(match removed {
                                    Some(value) => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "Some".into(),
                                        payload: vec![value],
                                    },
                                    None => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "None".into(),
                                        payload: vec![],
                                    },
                                });
                            }
                            "clear" => entries.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::Map { entries, capacity })
                            .ok_or_else(|| {
                                self.error("Map mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::Set {
                            mut values,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let field = match field.as_str() {
                            "count" => "len",
                            "empty" => "is_empty",
                            "contains" => "has",
                            "insert" => "add",
                            other => other,
                        };
                        match field {
                            "len" => return Ok(Value::UInt(values.len() as u64)),
                            "capacity" => return Ok(Value::UInt(capacity as u64)),
                            "is_empty" => return Ok(Value::Bool(values.is_empty())),
                            "has" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                return Ok(Value::Bool(values.contains(&value)));
                            }
                            "add" => {
                                let value = self.consume(program, &arguments[0])?;
                                let added = if values.contains(&value) {
                                    false
                                } else {
                                    grow_list_capacity(&mut capacity, values.len() + 1)
                                        .map_err(|message| self.error(message, expression.span))?;
                                    values.push(value);
                                    true
                                };
                                self.write_place(&place, Value::Set { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("Set mutation target is invalid", object.span)
                                    })?;
                                return Ok(Value::Bool(added));
                            }
                            "remove" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                let removed = values
                                    .iter()
                                    .position(|candidate| candidate == &value)
                                    .map(|index| values.remove(index))
                                    .is_some();
                                self.write_place(&place, Value::Set { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("Set mutation target is invalid", object.span)
                                    })?;
                                return Ok(Value::Bool(removed));
                            }
                            "clear" => values.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::Set { values, capacity })
                            .ok_or_else(|| {
                                self.error("Set mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    let value = self.evaluate(program, &arguments[0])?;
                    return Ok(
                        match coerce_value(
                            value,
                            &crate::ast::TypeName {
                                name: name.clone(),
                                arguments: vec![],
                                qualifier: crate::ast::TypeQualifier::Owned,
                                span: object.span,
                            },
                        ) {
                            Ok(value) => Value::Enum {
                                type_name: "Result".into(),
                                variant: "Ok".into(),
                                payload: vec![value],
                            },
                            Err(message) => Value::Enum {
                                type_name: "Result".into(),
                                variant: "Err".into(),
                                payload: vec![Value::String(RuntimeString::literal(message))],
                            },
                        },
                    );
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && matches!(
                        field.as_str(),
                        "wrapping_add"
                            | "wrapping_sub"
                            | "wrapping_mul"
                            | "saturating_add"
                            | "saturating_sub"
                            | "saturating_mul"
                    )
                {
                    let left = self.evaluate(program, object)?;
                    let right = coerce_like(self.evaluate(program, &arguments[0])?, &left)
                        .map_err(|message| self.error(message, arguments[0].span))?;
                    return integer_method(field, left, right)
                        .map_err(|message| self.error(message, expression.span));
                }
                if let Expression::FieldAccess { object, field, .. } = &callee.node
                    && !matches!(&object.node, Expression::Identifier(name) if program.enums.iter().any(|declaration| declaration.name == *name))
                {
                    if field == "lock" && arguments.is_empty() {
                        let Value::Mutex(mutex) = self.evaluate(program, object)? else {
                            return Err(self.error("Mutex method requires a Mutex", object.span));
                        };
                        return Ok(Value::MutexGuard(mutex.lock()));
                    }
                    if field == "share" && arguments.is_empty() {
                        return match self.evaluate(program, object)? {
                            Value::Mutex(mutex) => Ok(Value::Mutex(mutex)),
                            Value::AtomicInt(atomic) => Ok(Value::AtomicInt(atomic)),
                            _ => {
                                Err(self.error("share requires a Mutex or AtomicInt", object.span))
                            }
                        };
                    }
                    if matches!(
                        field.as_str(),
                        "share" | "load" | "store" | "add" | "fetch_add"
                    ) && let Value::AtomicInt(atomic) = self.evaluate(program, object)?
                    {
                        return match field.as_str() {
                            "share" => Ok(Value::AtomicInt(atomic)),
                            "load" => Ok(Value::Int(atomic.0.load(Ordering::SeqCst))),
                            "store" => {
                                let Value::Int(value) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates AtomicInt.store")
                                };
                                atomic.0.store(value, Ordering::SeqCst);
                                Ok(Value::Unit)
                            }
                            "add" | "fetch_add" => {
                                let Value::Int(value) = self.evaluate(program, &arguments[0])?
                                else {
                                    unreachable!("type checking validates AtomicInt.add")
                                };
                                let (previous, next) = atomic
                                    .add(value, expression.span)
                                    .map_err(RuntimeFault::Error)?;
                                Ok(Value::Int(if field == "add" { next } else { previous }))
                            }
                            _ => unreachable!(),
                        };
                    }
                    if matches!(field.as_str(), "offset" | "read" | "write")
                        && let Value::Reference(mut place, mutable) =
                            self.evaluate(program, object)?
                    {
                        return match field.as_str() {
                            "offset" => {
                                let offset = match self.evaluate(program, &arguments[0])? {
                                    Value::Int(value) => value,
                                    Value::Signed(value, _) => {
                                        i64::try_from(value).map_err(|_| {
                                            self.error(
                                                "raw pointer offset is out of range",
                                                arguments[0].span,
                                            )
                                        })?
                                    }
                                    _ => unreachable!("type checking validates pointer offset"),
                                };
                                let Some(PlaceSegment::Index(index)) = place.fields.last_mut()
                                else {
                                    return Err(self.error(
                                        "interpreter raw pointer has no addressable element",
                                        object.span,
                                    ));
                                };
                                *index =
                                    index.checked_add_signed(offset as isize).ok_or_else(|| {
                                        self.error("raw pointer offset overflow", arguments[0].span)
                                    })?;
                                Ok(Value::Reference(place, mutable))
                            }
                            "read" => self.read_place(&place).ok_or_else(|| {
                                self.error("raw pointer read is out of bounds", expression.span)
                            }),
                            "write" if mutable => {
                                let value = self.evaluate(program, &arguments[0])?;
                                self.write_place(&place, value).ok_or_else(|| {
                                    self.error(
                                        "raw pointer write is out of bounds",
                                        expression.span,
                                    )
                                })?;
                                Ok(Value::Unit)
                            }
                            "write" => Err(self.error(
                                "cannot write through a const raw pointer",
                                expression.span,
                            )),
                            _ => unreachable!(),
                        };
                    }
                    if field == "join" && arguments.is_empty() {
                        let Value::Thread(handle) = self.consume(program, object)? else {
                            return Err(
                                self.error("Thread.join requires a Thread value", object.span)
                            );
                        };
                        return handle.join(expression.span).map_err(RuntimeFault::Error);
                    }
                    if let Value::TcpListener(listener) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "accept" | "accept_timeout" => {
                                let timeout = if field == "accept_timeout" {
                                    let Value::Duration(duration) =
                                        self.evaluate(program, &arguments[0])?
                                    else {
                                        unreachable!("type checking validates accept timeout")
                                    };
                                    Some(duration)
                                } else {
                                    None
                                };
                                Ok(Value::Future(RuntimeFuture::operation(FutureWork::Accept(
                                    listener, timeout,
                                ))))
                            }
                            "local_port" => {
                                let guard = listener.0.lock().map_err(|_| {
                                    self.error("TCP listener state is poisoned", object.span)
                                })?;
                                let port = match guard.as_ref() {
                                    Some(listener) => listener
                                        .local_addr()
                                        .map(|address| Value::UInt(address.port() as u64)),
                                    None => Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TCP listener is closed",
                                    )),
                                };
                                Ok(runtime_result(port))
                            }
                            "close" => {
                                let mut guard = listener.0.lock().map_err(|_| {
                                    self.error("TCP listener state is poisoned", object.span)
                                })?;
                                guard.take();
                                Ok(Value::Unit)
                            }
                            _ => Err(self.error("unknown TcpListener operation", expression.span)),
                        };
                    }
                    if let Value::TcpStream(stream) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "read" => {
                                let limit = self.index_value(program, &arguments[0])?;
                                if limit > 16 * 1024 * 1024 {
                                    return Err(self.error(
                                        "TCP read limit exceeds the 16 MiB safety limit",
                                        arguments[0].span,
                                    ));
                                }
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TCP stream state is poisoned", object.span)
                                })?;
                                let result = if let Some(socket) = guard.as_mut() {
                                    let mut bytes = vec![0; limit];
                                    socket.read(&mut bytes).map(|count| {
                                        bytes.truncate(count);
                                        Value::List {
                                            capacity: bytes.len(),
                                            values: bytes
                                                .into_iter()
                                                .map(|byte| Value::Unsigned(byte as u128, 8))
                                                .collect(),
                                        }
                                    })
                                } else {
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TCP stream is closed",
                                    ))
                                };
                                Ok(runtime_result(result))
                            }
                            "write" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                let values = match value {
                                    Value::List { values, .. } | Value::Slice(values) => values,
                                    _ => unreachable!("type checking validates TCP bytes"),
                                };
                                let bytes = values
                                    .into_iter()
                                    .map(|value| match value {
                                        Value::Unsigned(byte, 8) => byte as u8,
                                        _ => unreachable!("type checking validates u8 elements"),
                                    })
                                    .collect::<Vec<_>>();
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TCP stream state is poisoned", object.span)
                                })?;
                                let result = if let Some(socket) = guard.as_mut() {
                                    socket
                                        .write_all(&bytes)
                                        .map(|()| Value::UInt(bytes.len() as u64))
                                } else {
                                    Err(std::io::Error::new(
                                        std::io::ErrorKind::NotConnected,
                                        "TCP stream is closed",
                                    ))
                                };
                                Ok(runtime_result(result))
                            }
                            "close" => {
                                let mut guard = stream.0.lock().map_err(|_| {
                                    self.error("TCP stream state is poisoned", object.span)
                                })?;
                                if let Some(socket) = guard.take() {
                                    let _ = socket.shutdown(Shutdown::Both);
                                }
                                Ok(Value::Unit)
                            }
                            _ => Err(self.error("unknown TcpStream operation", expression.span)),
                        };
                    }
                    if let Value::Path(path) = self.evaluate(program, object)? {
                        return match field.as_str() {
                            "join" => {
                                let child = runtime_path(self.evaluate(program, &arguments[0])?)
                                    .ok_or_else(|| {
                                        self.error(
                                            "Path.join expects Path, String, or str",
                                            arguments[0].span,
                                        )
                                    })?;
                                Ok(Value::Path(path.join(child)))
                            }
                            "len" => {
                                Ok(Value::UInt(path.as_os_str().to_string_lossy().len() as u64))
                            }
                            "is_empty" => Ok(Value::Bool(path.as_os_str().is_empty())),
                            "is_absolute" => Ok(Value::Bool(path.is_absolute())),
                            "as_string" => Ok(Value::String(RuntimeString::literal(
                                path.to_string_lossy().into_owned(),
                            ))),
                            "name" => Ok(option_value(path.file_name().map(|value| {
                                Value::String(RuntimeString::literal(
                                    value.to_string_lossy().into_owned(),
                                ))
                            }))),
                            "extension" => Ok(option_value(path.extension().map(|value| {
                                Value::String(RuntimeString::literal(
                                    value.to_string_lossy().into_owned(),
                                ))
                            }))),
                            "parent" => Ok(option_value(
                                path.parent().map(|value| Value::Path(value.to_path_buf())),
                            )),
                            _ => Err(self.error("unknown Path operation", expression.span)),
                        };
                    }
                    if let Value::Instant(instant) = self.evaluate(program, object)?
                        && field == "elapsed"
                    {
                        return Ok(Value::Duration(instant.elapsed()));
                    }
                    if let Value::Duration(duration) = self.evaluate(program, object)? {
                        return Ok(Value::UInt(match field.as_str() {
                            "nanos" => duration.as_nanos().min(u64::MAX as u128) as u64,
                            "millis" => duration.as_millis().min(u64::MAX as u128) as u64,
                            _ => duration.as_secs(),
                        }));
                    }
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::Map {
                            mut entries,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let method = match field.as_str() {
                            "count" => "len",
                            "empty" => "is_empty",
                            "contains_key" => "has",
                            "insert" => "set",
                            other => other,
                        };
                        match method {
                            "keys" => {
                                return Ok(Value::Slice(
                                    entries.iter().map(|(key, _)| key.clone()).collect(),
                                ));
                            }
                            "values" => {
                                return Ok(Value::Slice(
                                    entries.iter().map(|(_, value)| value.clone()).collect(),
                                ));
                            }
                            "len" => return Ok(Value::UInt(entries.len() as u64)),
                            "capacity" => return Ok(Value::UInt(capacity as u64)),
                            "is_empty" => return Ok(Value::Bool(entries.is_empty())),
                            "has" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                return Ok(Value::Bool(
                                    entries.iter().any(|(candidate, _)| candidate == &key),
                                ));
                            }
                            "get" | "get_mut" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                return Ok(
                                    match entries
                                        .iter()
                                        .position(|(candidate, _)| candidate == &key)
                                    {
                                        Some(index) => {
                                            let mut value = place.clone();
                                            value.fields.push(PlaceSegment::MapValue(index));
                                            Value::Enum {
                                                type_name: "Option".into(),
                                                variant: "Some".into(),
                                                payload: vec![Value::Reference(
                                                    value,
                                                    method == "get_mut",
                                                )],
                                            }
                                        }
                                        None => Value::Enum {
                                            type_name: "Option".into(),
                                            variant: "None".into(),
                                            payload: vec![],
                                        },
                                    },
                                );
                            }
                            "set" => {
                                let key = self.consume(program, &arguments[0])?;
                                let value = self.consume(program, &arguments[1])?;
                                let old = if let Some(entry) =
                                    entries.iter_mut().find(|(candidate, _)| candidate == &key)
                                {
                                    Some(std::mem::replace(&mut entry.1, value))
                                } else {
                                    grow_list_capacity(&mut capacity, entries.len() + 1)
                                        .map_err(|message| self.error(message, expression.span))?;
                                    entries.push((key, value));
                                    None
                                };
                                self.write_place(&place, Value::Map { entries, capacity })
                                    .ok_or_else(|| {
                                        self.error("Map mutation target is invalid", object.span)
                                    })?;
                                return Ok(option_value(old));
                            }
                            "remove" => {
                                let key = self.evaluate(program, &arguments[0])?;
                                let removed = entries
                                    .iter()
                                    .position(|(candidate, _)| candidate == &key)
                                    .map(|index| entries.remove(index).1);
                                self.write_place(&place, Value::Map { entries, capacity })
                                    .ok_or_else(|| {
                                        self.error("Map mutation target is invalid", object.span)
                                    })?;
                                return Ok(option_value(removed));
                            }
                            "clear" => entries.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::Map { entries, capacity })
                            .ok_or_else(|| {
                                self.error("Map mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::Set {
                            mut values,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let method = match field.as_str() {
                            "count" => "len",
                            "empty" => "is_empty",
                            "contains" => "has",
                            "insert" => "add",
                            other => other,
                        };
                        match method {
                            "iter" => return Ok(Value::Slice(values)),
                            "len" => return Ok(Value::UInt(values.len() as u64)),
                            "capacity" => return Ok(Value::UInt(capacity as u64)),
                            "is_empty" => return Ok(Value::Bool(values.is_empty())),
                            "has" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                return Ok(Value::Bool(values.contains(&value)));
                            }
                            "add" => {
                                let value = self.consume(program, &arguments[0])?;
                                let added = if values.contains(&value) {
                                    false
                                } else {
                                    grow_list_capacity(&mut capacity, values.len() + 1)
                                        .map_err(|message| self.error(message, expression.span))?;
                                    values.push(value);
                                    true
                                };
                                self.write_place(&place, Value::Set { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("Set mutation target is invalid", object.span)
                                    })?;
                                return Ok(Value::Bool(added));
                            }
                            "remove" => {
                                let value = self.evaluate(program, &arguments[0])?;
                                let removed = values
                                    .iter()
                                    .position(|candidate| candidate == &value)
                                    .map(|index| values.remove(index))
                                    .is_some();
                                self.write_place(&place, Value::Set { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("Set mutation target is invalid", object.span)
                                    })?;
                                return Ok(Value::Bool(removed));
                            }
                            "clear" => values.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::Set { values, capacity })
                            .ok_or_else(|| {
                                self.error("Set mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    if let Some(place) = self.expression_place(object)
                        && let Some(Value::List {
                            mut values,
                            mut capacity,
                        }) = self.read_place(&place)
                    {
                        let field = match field.as_str() {
                            "add" => "push",
                            "count" => "len",
                            "empty" => "is_empty",
                            other => other,
                        };
                        match field {
                            "iter" => return Ok(Value::Slice(values)),
                            "len" if arguments.is_empty() => {
                                return Ok(Value::UInt(values.len() as u64));
                            }
                            "capacity" if arguments.is_empty() => {
                                return Ok(Value::UInt(capacity as u64));
                            }
                            "is_empty" if arguments.is_empty() => {
                                return Ok(Value::Bool(values.is_empty()));
                            }
                            "push" => {
                                let value = self.consume(program, &arguments[0])?;
                                grow_list_capacity(&mut capacity, values.len() + 1)
                                    .map_err(|message| self.error(message, expression.span))?;
                                values.push(value);
                            }
                            "pop" => {
                                let value = values.pop();
                                self.write_place(&place, Value::List { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("List mutation target is invalid", object.span)
                                    })?;
                                return Ok(match value {
                                    Some(value) => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "Some".into(),
                                        payload: vec![value],
                                    },
                                    None => Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "None".into(),
                                        payload: vec![],
                                    },
                                });
                            }
                            "get" | "get_mut" => {
                                let index = self.index_value(program, &arguments[0])?;
                                if index >= values.len() {
                                    return Ok(Value::Enum {
                                        type_name: "Option".into(),
                                        variant: "None".into(),
                                        payload: vec![],
                                    });
                                }
                                let mut element = place.clone();
                                element.fields.push(PlaceSegment::Index(index));
                                return Ok(Value::Enum {
                                    type_name: "Option".into(),
                                    variant: "Some".into(),
                                    payload: vec![Value::Reference(element, field == "get_mut")],
                                });
                            }
                            "insert" => {
                                let index = self.index_value(program, &arguments[0])?;
                                if index > values.len() {
                                    return Err(self.error(
                                        format!(
                                            "List insertion index {index} is out of bounds for length {}",
                                            values.len()
                                        ),
                                        arguments[0].span,
                                    ));
                                }
                                let value = self.consume(program, &arguments[1])?;
                                grow_list_capacity(&mut capacity, values.len() + 1)
                                    .map_err(|message| self.error(message, expression.span))?;
                                values.insert(index, value);
                            }
                            "remove" => {
                                let index = self.index_value(program, &arguments[0])?;
                                if index >= values.len() {
                                    return Err(self.error(
                                        format!(
                                            "List removal index {index} is out of bounds for length {}",
                                            values.len()
                                        ),
                                        arguments[0].span,
                                    ));
                                }
                                let removed = values.remove(index);
                                self.write_place(&place, Value::List { values, capacity })
                                    .ok_or_else(|| {
                                        self.error("List mutation target is invalid", object.span)
                                    })?;
                                return Ok(removed);
                            }
                            "clear" => values.clear(),
                            _ => {}
                        }
                        self.write_place(&place, Value::List { values, capacity })
                            .ok_or_else(|| {
                                self.error("List mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    if matches!(field.as_str(), "len" | "is_empty")
                        && arguments.is_empty()
                        && let Value::Array(values) | Value::Slice(values) =
                            self.evaluate(program, object)?
                    {
                        return Ok(if field == "is_empty" {
                            Value::Bool(values.is_empty())
                        } else {
                            Value::UInt(values.len() as u64)
                        });
                    }
                    if field == "iter"
                        && arguments.is_empty()
                        && let Value::Array(values) | Value::Slice(values) =
                            self.evaluate(program, object)?
                    {
                        return Ok(Value::Slice(values));
                    }
                    if matches!(field.as_str(), "as_ptr" | "as_mut_ptr")
                        && arguments.is_empty()
                        && self
                            .dynamic_place(program, object)?
                            .and_then(|place| self.read_place(&place))
                            .is_some_and(|value| matches!(value, Value::Memory(_)))
                    {
                        let mut place = self.dynamic_place(program, object)?.ok_or_else(|| {
                            self.error("Memory pointer source is not a place", object.span)
                        })?;
                        place.fields.push(PlaceSegment::Index(0));
                        return Ok(Value::Reference(place, field == "as_mut_ptr"));
                    }
                    if matches!(
                        field.as_str(),
                        "len" | "alignment" | "is_empty" | "read" | "write" | "fill" | "copy_from"
                    ) && let Value::Memory(memory) = self.evaluate(program, object)?
                    {
                        let length = memory.len().ok_or_else(|| {
                            self.error("Memory state lock is poisoned", object.span)
                        })?;
                        return match field.as_str() {
                            "len" => Ok(Value::UInt(length as u64)),
                            "alignment" => Ok(Value::UInt(memory.alignment().ok_or_else(|| {
                                self.error("Memory state lock is poisoned", object.span)
                            })? as u64)),
                            "is_empty" => Ok(Value::Bool(length == 0)),
                            "read" => {
                                let index = self.index_value(program, &arguments[0])?;
                                memory
                                    .read(index)
                                    .map(|value| Value::Unsigned(value.into(), 8))
                                    .ok_or_else(|| {
                                        self.error(
                                            format!(
                                                "Memory index {index} is out of bounds for length {length}"
                                            ),
                                            arguments[0].span,
                                        )
                                    })
                            }
                            "write" => {
                                let index = self.index_value(program, &arguments[0])?;
                                let byte = runtime_byte(self.evaluate(program, &arguments[1])?)
                                    .ok_or_else(|| {
                                        self.error("Memory value must be a u8", arguments[1].span)
                                    })?;
                                memory.write(index, byte).ok_or_else(|| {
                                    self.error(
                                        format!(
                                            "Memory index {index} is out of bounds for length {length}"
                                        ),
                                        arguments[0].span,
                                    )
                                })?;
                                Ok(Value::Unit)
                            }
                            "fill" => {
                                let byte = runtime_byte(self.evaluate(program, &arguments[0])?)
                                    .ok_or_else(|| {
                                        self.error("Memory value must be a u8", arguments[0].span)
                                    })?;
                                memory.fill(byte).ok_or_else(|| {
                                    self.error("Memory state lock is poisoned", object.span)
                                })?;
                                Ok(Value::Unit)
                            }
                            "copy_from" => {
                                let destination = self.index_value(program, &arguments[0])?;
                                let Value::Memory(source) =
                                    self.evaluate(program, &arguments[1])?
                                else {
                                    unreachable!("type checking validates Memory copy source")
                                };
                                let source_offset = self.index_value(program, &arguments[2])?;
                                let count = self.index_value(program, &arguments[3])?;
                                memory
                                    .copy_from(destination, &source, source_offset, count)
                                    .ok_or_else(|| {
                                        self.error(
                                            "Memory copy range is out of bounds",
                                            expression.span,
                                        )
                                    })?;
                                Ok(Value::Unit)
                            }
                            _ => unreachable!(),
                        };
                    }
                    if arguments.is_empty()
                        && matches!(
                            field.as_str(),
                            "len" | "is_empty" | "to_string" | "as_c_str"
                        )
                        && let value @ (Value::CString(_) | Value::CStr(_)) =
                            self.evaluate(program, object)?
                    {
                        let (value, owned) = match value {
                            Value::CString(value) => (value, true),
                            Value::CStr(value) => (value, false),
                            _ => unreachable!(),
                        };
                        return Ok(match field.as_str() {
                            "len" => Value::UInt(value.len() as u64),
                            "is_empty" => Value::Bool(value.len() == 0),
                            "to_string" => Value::String(RuntimeString::literal(value.text())),
                            "as_c_str" if owned => Value::CStr(value),
                            _ => unreachable!("type checking validates CString methods"),
                        });
                    }
                    if arguments.is_empty()
                        && matches!(field.as_str(), "len" | "capacity" | "is_empty")
                        && let Value::String(value) = self.evaluate(program, object)?
                    {
                        return Ok(match field.as_str() {
                            "len" => Value::UInt(value.len() as u64),
                            "capacity" => Value::UInt(value.capacity() as u64),
                            _ => Value::Bool(value.is_empty()),
                        });
                    }
                    if matches!(field.as_str(), "contains" | "starts_with" | "ends_with") {
                        let Value::String(value) = self.evaluate(program, object)? else {
                            unreachable!()
                        };
                        let Value::String(pattern) = self.evaluate(program, &arguments[0])? else {
                            unreachable!()
                        };
                        return Ok(Value::Bool(match field.as_str() {
                            "contains" => value.text.contains(&pattern.text),
                            "starts_with" => value.text.starts_with(&pattern.text),
                            _ => value.text.ends_with(&pattern.text),
                        }));
                    }
                    if matches!(
                        field.as_str(),
                        "push" | "push_str" | "append" | "add" | "clear"
                    ) && self
                        .expression_place(object)
                        .and_then(|place| self.read_place(&place))
                        .is_some_and(|value| matches!(value, Value::String(_)))
                    {
                        let place = self.expression_place(object).ok_or_else(|| {
                            self.error("String mutation requires a mutable place", object.span)
                        })?;
                        let Some(Value::String(mut value)) = self.read_place(&place) else {
                            return Err(
                                self.error("String method receiver is invalid", object.span)
                            );
                        };
                        match field.as_str() {
                            "push" => match self.evaluate(program, &arguments[0])? {
                                Value::Char(ch) => value.push(ch),
                                _ => unreachable!(),
                            },
                            "push_str" | "append" | "add" => {
                                match self.evaluate(program, &arguments[0])? {
                                    Value::String(text) => value.push_str(&text),
                                    _ => unreachable!(),
                                }
                            }
                            _ => value.clear(),
                        }
                        self.write_place(&place, Value::String(value))
                            .ok_or_else(|| {
                                self.error("String mutation target is invalid", object.span)
                            })?;
                        return Ok(Value::Unit);
                    }
                    let receiver = self.evaluate(program, object)?;
                    let receiver_name = value_type_name(&receiver);
                    let methods = program
                        .implementations
                        .iter()
                        .filter_map(|implementation| {
                            (implementation.target.name == receiver_name)
                                .then(|| {
                                    implementation
                                        .methods
                                        .iter()
                                        .find(|method| method.name == *field)
                                })
                                .flatten()
                        })
                        .collect::<Vec<_>>();
                    if methods.len() == 1 {
                        let mut values = Vec::with_capacity(arguments.len() + 1);
                        let qualifier = methods[0]
                            .parameters
                            .first()
                            .map(|parameter| parameter.ty.qualifier)
                            .unwrap_or(crate::ast::TypeQualifier::Owned);
                        if matches!(
                            qualifier,
                            crate::ast::TypeQualifier::SharedReference
                                | crate::ast::TypeQualifier::MutableReference
                        ) {
                            let place = self.expression_place(object).ok_or_else(|| {
                                self.error("method receiver is not borrowable", object.span)
                            })?;
                            values.push(Value::Reference(
                                place,
                                qualifier == crate::ast::TypeQualifier::MutableReference,
                            ));
                        } else {
                            values.push(self.consume(program, object)?);
                        }
                        for (argument, parameter) in
                            arguments.iter().zip(methods[0].parameters.iter().skip(1))
                        {
                            values.push(
                                if parameter.ty.qualifier == crate::ast::TypeQualifier::Owned {
                                    self.consume(program, argument)?
                                } else {
                                    self.evaluate(program, argument)?
                                },
                            );
                        }
                        return self.call_function(program, methods[0], values, expression.span);
                    }
                }
                if matches!(&callee.node, Expression::Identifier(name) if name == "print") {
                    let value = self.evaluate(program, &arguments[0])?;
                    let value = match value {
                        Value::Reference(place, _) => self
                            .read_place(&place)
                            .ok_or_else(|| self.error("dangling reference", arguments[0].span))?,
                        value => value,
                    };
                    self.output
                        .lock()
                        .map_err(|_| {
                            self.error("interpreter output lock is poisoned", expression.span)
                        })?
                        .push(display_value(value));
                    return Ok(Value::Unit);
                }
                let callee_value = self.evaluate(program, callee)?;
                if let Value::Constructor { type_name, variant } = callee_value {
                    let mut payload = Vec::with_capacity(arguments.len());
                    for argument in arguments {
                        payload.push(self.consume(program, argument)?);
                    }
                    return Ok(Value::Enum {
                        type_name,
                        variant,
                        payload,
                    });
                }
                if let Value::Closure(closure) = callee_value {
                    let mut values = Vec::with_capacity(arguments.len());
                    for (argument, parameter) in arguments.iter().zip(&closure.parameters) {
                        values.push(
                            if parameter.ty.qualifier == crate::ast::TypeQualifier::Owned {
                                self.consume(program, argument)?
                            } else {
                                self.evaluate(program, argument)?
                            },
                        );
                    }
                    return self.call_closure(program, *closure, values, expression.span);
                }
                let Value::Function(name) = callee_value else {
                    return Err(self.error("expression is not callable", callee.span));
                };
                let mut values = Vec::with_capacity(arguments.len());
                let function = program
                    .functions
                    .iter()
                    .find(|function| function.name == name)
                    .ok_or_else(|| self.error(format!("unknown function `{name}`"), callee.span))?
                    .clone();
                for (argument, parameter) in arguments.iter().zip(&function.parameters) {
                    values.push(
                        if parameter.ty.qualifier == crate::ast::TypeQualifier::Owned {
                            self.consume(program, argument)?
                        } else {
                            let value = self.evaluate(program, argument)?;
                            if matches!(value, Value::Reference(_, _)) {
                                value
                            } else if parameter.ty.qualifier
                                == crate::ast::TypeQualifier::SharedReference
                            {
                                Value::Reference(
                                    self.expression_place(argument).ok_or_else(|| {
                                        self.error(
                                            "implicit borrow argument has no storage place",
                                            argument.span,
                                        )
                                    })?,
                                    false,
                                )
                            } else {
                                value
                            }
                        },
                    );
                }
                self.call_function(program, &function, values, expression.span)
            }
            Expression::Match { value, arms } => {
                let value = self.consume(program, value)?;
                for arm in arms {
                    let mut bindings = HashMap::new();
                    if pattern_matches(&arm.pattern.node, &value, &mut bindings) {
                        self.push_scope(bindings);
                        let result = self.evaluate(program, &arm.value);
                        self.pop_scope();
                        return result;
                    }
                }
                Err(self.error("exhaustive match had no matching arm", expression.span))
            }
            Expression::Try(operand) => {
                let value = self.consume(program, operand)?;
                match &value {
                    Value::Enum {
                        type_name,
                        variant,
                        payload,
                    } if type_name == "Option" && variant == "Some" => Ok(payload[0].clone()),
                    Value::Enum {
                        type_name, variant, ..
                    } if type_name == "Option" && variant == "None" => {
                        Err(RuntimeFault::Propagate(value))
                    }
                    Value::Enum {
                        type_name,
                        variant,
                        payload,
                    } if type_name == "Result" && variant == "Ok" => Ok(payload[0].clone()),
                    Value::Enum {
                        type_name, variant, ..
                    } if type_name == "Result" && variant == "Err" => {
                        Err(RuntimeFault::Propagate(value))
                    }
                    _ => Err(self.error("`?` requires Option or Result", expression.span)),
                }
            }
            Expression::Spawn(task) => self.spawn_task(program, task, expression.span),
            Expression::Await(future) => {
                let value = self.consume(program, future)?;
                match value {
                    Value::Future(future) => self.await_future(program, future, expression.span),
                    Value::Task(task) => {
                        let work = {
                            let mut work = task
                                .0
                                .lock()
                                .map_err(|_| self.error("task state is poisoned", future.span))?;
                            std::mem::replace(&mut *work, RuntimeTaskWork::Consumed)
                        };
                        match work {
                            RuntimeTaskWork::Future(future) => {
                                self.await_future(program, future, expression.span)
                            }
                            RuntimeTaskWork::Ready(value) => Ok(value),
                            RuntimeTaskWork::Running => {
                                Err(self.error("task cannot await itself", future.span))
                            }
                            RuntimeTaskWork::Consumed => {
                                Err(self.error("task has already been awaited", future.span))
                            }
                        }
                    }
                    _ => Err(self.error("`await` requires a Future or Task", future.span)),
                }
            }
            Expression::Borrow { mutable, target } => {
                let place = self
                    .dynamic_place(program, target)?
                    .ok_or_else(|| self.error("borrow target is not a place", target.span))?;
                Ok(Value::Reference(place, *mutable))
            }
            Expression::Dereference(target) => match self.evaluate(program, target)? {
                Value::Reference(place, _) => self
                    .read_place(&place)
                    .ok_or_else(|| self.error("dangling or invalid pointer", expression.span)),
                Value::MutexGuard(guard) => guard
                    .read()
                    .ok_or_else(|| self.error("invalid Mutex guard", expression.span)),
                _ => Err(self.error("value cannot be dereferenced", expression.span)),
            },
            Expression::Move(target) => {
                let place = self
                    .dynamic_place(program, target)?
                    .ok_or_else(|| self.error("move target is not a place", target.span))?;
                let value = self
                    .read_place(&place)
                    .ok_or_else(|| self.error("invalid move target", target.span))?;
                self.write_place(&place, Value::Uninitialized)
                    .ok_or_else(|| self.error("invalid move target", target.span))?;
                Ok(value)
            }
        }
    }

    fn spawn_task(&mut self, program: &Program, task: &Expr, span: Span) -> RuntimeResult<Value> {
        let Expression::Call { callee, arguments } = &task.node else {
            return Err(self.error("`spawn` requires a direct function call", task.span));
        };
        let Expression::Identifier(name) = &callee.node else {
            return Err(self.error("`spawn` requires a named DISP function", callee.span));
        };
        let function = program
            .functions
            .iter()
            .find(|function| function.name == *name)
            .ok_or_else(|| self.error("unknown spawned function", callee.span))?
            .clone();
        let mut values = Vec::with_capacity(arguments.len());
        for argument in arguments {
            values.push(self.consume(program, argument)?);
        }
        let child_program = program.clone();
        let output = Arc::clone(&self.output);
        let call_span = task.span;
        let handle = thread::Builder::new()
            .name(format!("disp-{name}"))
            .stack_size(INTERPRETER_STACK_BYTES)
            .spawn(move || {
                let mut child = Interpreter {
                    scopes: Vec::new(),
                    scope_orders: Vec::new(),
                    output,
                    call_depth: 0,
                    tasks: Vec::new(),
                };
                child
                    .call_function(&child_program, &function, values, call_span)
                    .map(|value| Box::new(value) as Box<dyn Any + Send>)
                    .map_err(RuntimeFault::into_diagnostic)
            })
            .map_err(|error| self.error(format!("could not spawn thread: {error}"), span))?;
        Ok(Value::Thread(RuntimeThread::new(handle)))
    }

    fn push_scope(&mut self, values: HashMap<String, Value>) {
        let order = values.keys().cloned().collect();
        self.scopes.push(values);
        self.scope_orders.push(order);
    }

    fn consume(&mut self, program: &Program, expression: &Expr) -> RuntimeResult<Value> {
        if matches!(expression.node, Expression::Move(_)) {
            return self.evaluate(program, expression);
        }
        if let Some(place) = self.expression_place(expression) {
            let value = self
                .read_place(&place)
                .ok_or_else(|| self.error("invalid move source", expression.span))?;
            if !value_is_copy(program, &value) {
                self.write_place(&place, Value::Uninitialized)
                    .ok_or_else(|| self.error("invalid move source", expression.span))?;
            }
            Ok(value)
        } else {
            self.evaluate(program, expression)
        }
    }

    fn pop_scope(&mut self) {
        if let (Some(scope), Some(order)) = (self.scopes.last_mut(), self.scope_orders.last()) {
            for name in order.iter().rev() {
                scope.remove(name);
            }
        }
        self.scopes.pop();
        self.scope_orders.pop();
    }

    fn evaluate_binary(
        &self,
        operator: BinaryOperator,
        left: Value,
        right: Value,
        span: Span,
    ) -> RuntimeResult<Value> {
        let invalid = || self.error(format!("invalid operands for `{operator:?}`"), span);
        match operator {
            BinaryOperator::Add => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(a), Value::UInt(b)) => a
                    .checked_add(b)
                    .map(Value::UInt)
                    .ok_or_else(|| self.error("uint overflow in addition", span)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a + b)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_add(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in addition", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a + b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Subtract => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(a), Value::UInt(b)) => a
                    .checked_sub(b)
                    .map(Value::UInt)
                    .ok_or_else(|| self.error("uint overflow in subtraction", span)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a - b)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_sub(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in subtraction", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a - b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Multiply => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(a), Value::UInt(b)) => a
                    .checked_mul(b)
                    .map(Value::UInt)
                    .ok_or_else(|| self.error("uint overflow in multiplication", span)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a * b)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_mul(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in multiplication", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a * b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Divide => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(_), Value::UInt(0)) => Err(self.error("division by zero", span)),
                (Value::UInt(a), Value::UInt(b)) => Ok(Value::UInt(a / b)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a / b)),
                (Value::Int(_), Value::Int(0)) => Err(self.error("division by zero", span)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_div(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in division", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a / b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Remainder => match (left, right) {
                (Value::Signed(a, w), Value::Signed(b, v)) if w == v => {
                    checked_signed(a, b, w, operator, span, self)
                }
                (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => {
                    checked_unsigned(a, b, w, operator, span, self)
                }
                (Value::UInt(_), Value::UInt(0)) => Err(self.error("remainder by zero", span)),
                (Value::UInt(a), Value::UInt(b)) => Ok(Value::UInt(a % b)),
                (Value::Float32(a), Value::Float32(b)) => Ok(Value::Float32(a % b)),
                (Value::Int(_), Value::Int(0)) => Err(self.error("remainder by zero", span)),
                (Value::Int(a), Value::Int(b)) => a
                    .checked_rem(b)
                    .map(Value::Int)
                    .ok_or_else(|| self.error("integer overflow in remainder", span)),
                (Value::Float(a), Value::Float(b)) => Ok(Value::Float(a % b)),
                _ => Err(invalid()),
            },
            BinaryOperator::Equal => Ok(Value::Bool(left == right)),
            BinaryOperator::NotEqual => Ok(Value::Bool(left != right)),
            BinaryOperator::Less
            | BinaryOperator::LessEqual
            | BinaryOperator::Greater
            | BinaryOperator::GreaterEqual => compare(left, right, operator, span, self),
            BinaryOperator::And | BinaryOperator::Or => {
                Err(self.error("logical operator was not short-circuited", span))
            }
        }
    }

    fn lookup(&self, name: &str) -> Option<Value> {
        self.scopes
            .iter()
            .rev()
            .find_map(|scope| scope.get(name).cloned())
    }

    fn assign(&mut self, name: &str, value: Value) -> Option<()> {
        if let Some(Value::CaptureReference(place, mutable)) = self.lookup(name) {
            if !mutable {
                return None;
            }
            return self.write_place(&place, value);
        }
        let scope = self
            .scopes
            .iter_mut()
            .rev()
            .find(|scope| scope.contains_key(name))?;
        scope.insert(name.into(), value);
        Some(())
    }

    fn index_value(&mut self, program: &Program, expression: &Expr) -> RuntimeResult<usize> {
        match self.evaluate(program, expression)? {
            Value::Int(value) if value >= 0 => Ok(value as usize),
            Value::UInt(value) => Ok(value as usize),
            Value::Signed(value, _) if value >= 0 => Ok(value as usize),
            Value::Unsigned(value, _) => usize::try_from(value)
                .map_err(|_| self.error("index is too large", expression.span)),
            _ => Err(self.error("index must be a non-negative integer", expression.span)),
        }
    }

    fn expression_place(&self, expression: &Expr) -> Option<Place> {
        match &expression.node {
            Expression::Identifier(name) => {
                if let Some(Value::CaptureReference(place, _)) = self.lookup(name) {
                    return Some(place);
                }
                let scope = self
                    .scopes
                    .iter()
                    .rposition(|scope| scope.contains_key(name))?;
                Some(Place {
                    scope,
                    name: name.clone(),
                    fields: vec![],
                })
            }
            Expression::FieldAccess { object, field, .. } => {
                let mut place = if let Expression::Identifier(name) = &object.node {
                    match self.lookup(name) {
                        Some(Value::Reference(place, _)) => place,
                        _ => self.expression_place(object)?,
                    }
                } else {
                    self.expression_place(object)?
                };
                place.fields.push(PlaceSegment::Field(field.clone()));
                Some(place)
            }
            Expression::Dereference(target) => match &target.node {
                Expression::Identifier(name) => match self.lookup(name)? {
                    Value::Reference(place, _) => Some(place),
                    Value::MutexGuard(_) => {
                        let scope = self
                            .scopes
                            .iter()
                            .rposition(|scope| scope.contains_key(name))?;
                        Some(Place {
                            scope,
                            name: name.clone(),
                            fields: vec![PlaceSegment::MutexValue],
                        })
                    }
                    _ => None,
                },
                _ => None,
            },
            _ => None,
        }
    }

    fn dynamic_place(
        &mut self,
        program: &Program,
        expression: &Expr,
    ) -> RuntimeResult<Option<Place>> {
        match &expression.node {
            Expression::Index { object, index } => {
                let Some(mut place) = self.dynamic_place(program, object)? else {
                    return Ok(None);
                };
                let index = self.index_value(program, index)?;
                let length = match self.read_place(&place) {
                    Some(Value::Array(values) | Value::Slice(values)) => values.len(),
                    Some(Value::List { values, .. }) => values.len(),
                    _ => return Ok(None),
                };
                if index >= length {
                    return Err(self.error(
                        format!("array index {index} is out of bounds for length {length}"),
                        expression.span,
                    ));
                }
                place.fields.push(PlaceSegment::Index(index));
                Ok(Some(place))
            }
            Expression::Subslice { object, start, end } => {
                let Some(mut place) = self.dynamic_place(program, object)? else {
                    return Ok(None);
                };
                let start = self.index_value(program, start)?;
                let end = self.index_value(program, end)?;
                let source = self.read_place(&place);
                let length = match &source {
                    Some(Value::Array(values) | Value::Slice(values)) => values.len(),
                    Some(Value::List { values, .. }) => values.len(),
                    Some(Value::String(value)) => value.text.len(),
                    _ => return Ok(None),
                };
                let invalid_utf8 = matches!(
                    &source,
                    Some(Value::String(value))
                        if !value.text.is_char_boundary(start) || !value.text.is_char_boundary(end)
                );
                if start > end || end > length || invalid_utf8 {
                    return Err(self.error(
                        format!("subslice range {start}..{end} is out of bounds or not on UTF-8 boundaries for length {length}"),
                        expression.span,
                    ));
                }
                place.fields.push(PlaceSegment::Subslice(start, end));
                Ok(Some(place))
            }
            _ => Ok(self.expression_place(expression)),
        }
    }

    fn read_place(&self, place: &Place) -> Option<Value> {
        read_value_path(
            self.scopes.get(place.scope)?.get(&place.name)?,
            &place.fields,
        )
    }

    fn write_place(&mut self, place: &Place, value: Value) -> Option<()> {
        let root = self.scopes.get_mut(place.scope)?.get_mut(&place.name)?;
        write_value_path(root, &place.fields, value)
    }

    fn error(&self, message: impl Into<String>, span: Span) -> RuntimeFault {
        RuntimeFault::Error(self.diagnostic(message, span))
    }

    fn diagnostic(&self, message: impl Into<String>, span: Span) -> Diagnostic {
        Diagnostic::new(DiagnosticKind::Runtime, message, span)
    }
}

fn write_value_path(target: &mut Value, fields: &[PlaceSegment], value: Value) -> Option<()> {
    if fields.is_empty() {
        *target = value;
        return Some(());
    }
    match &fields[0] {
        PlaceSegment::Field(field) => {
            let Value::Struct {
                fields: members, ..
            } = target
            else {
                return None;
            };
            write_value_path(members.get_mut(field)?, &fields[1..], value)
        }
        PlaceSegment::Index(index) => {
            if let Value::Memory(memory) = target {
                if fields.len() != 1 {
                    return None;
                }
                return memory.write(*index, runtime_byte(value)?);
            }
            let values = match target {
                Value::Array(values) | Value::Slice(values) => values,
                Value::List { values, .. } => values,
                _ => return None,
            };
            write_value_path(values.get_mut(*index)?, &fields[1..], value)
        }
        PlaceSegment::MapValue(index) => {
            let Value::Map { entries, .. } = target else {
                return None;
            };
            write_value_path(&mut entries.get_mut(*index)?.1, &fields[1..], value)
        }
        PlaceSegment::Subslice(start, end) => {
            let values = match target {
                Value::Array(values) | Value::Slice(values) => values,
                Value::List { values, .. } => values,
                _ => return None,
            };
            if *start > *end || *end > values.len() {
                return None;
            }
            match fields.get(1)? {
                PlaceSegment::Index(index) if *index < end - start => {
                    write_value_path(values.get_mut(start + index)?, &fields[2..], value)
                }
                _ => None,
            }
        }
        PlaceSegment::MutexValue => {
            let Value::MutexGuard(guard) = target else {
                return None;
            };
            if fields.len() == 1 {
                guard.write(value)
            } else {
                let mut inner = guard.read()?;
                write_value_path(&mut inner, &fields[1..], value)?;
                guard.write(inner)
            }
        }
    }
}

fn read_value_path(target: &Value, fields: &[PlaceSegment]) -> Option<Value> {
    if fields.is_empty() {
        return (!matches!(target, Value::Uninitialized)).then(|| target.clone());
    }
    match &fields[0] {
        PlaceSegment::Field(field) => {
            let Value::Struct {
                fields: members, ..
            } = target
            else {
                return None;
            };
            read_value_path(members.get(field)?, &fields[1..])
        }
        PlaceSegment::Index(index) => {
            if let Value::Memory(memory) = target {
                if fields.len() != 1 {
                    return None;
                }
                return memory
                    .read(*index)
                    .map(|value| Value::Unsigned(value.into(), 8));
            }
            let values = match target {
                Value::Array(values) | Value::Slice(values) => values,
                Value::List { values, .. } => values,
                _ => return None,
            };
            read_value_path(values.get(*index)?, &fields[1..])
        }
        PlaceSegment::MapValue(index) => {
            let Value::Map { entries, .. } = target else {
                return None;
            };
            read_value_path(&entries.get(*index)?.1, &fields[1..])
        }
        PlaceSegment::Subslice(start, end) => {
            if let Value::String(text) = target {
                if *start > *end
                    || *end > text.text.len()
                    || !text.text.is_char_boundary(*start)
                    || !text.text.is_char_boundary(*end)
                {
                    return None;
                }
                let sliced =
                    Value::String(RuntimeString::literal(text.text[*start..*end].to_owned()));
                return read_value_path(&sliced, &fields[1..]);
            }
            let values = match target {
                Value::Array(values) | Value::Slice(values) => values,
                Value::List { values, .. } => values,
                _ => return None,
            };
            if *start > *end || *end > values.len() {
                return None;
            }
            if fields.len() == 1 {
                return Some(Value::Slice(values[*start..*end].to_vec()));
            }
            match &fields[1] {
                PlaceSegment::Index(index) if *index < end - start => {
                    read_value_path(values.get(start + index)?, &fields[2..])
                }
                _ => None,
            }
        }
        PlaceSegment::MutexValue => {
            let Value::MutexGuard(guard) = target else {
                return None;
            };
            read_value_path(&guard.read()?, &fields[1..])
        }
    }
}

impl RuntimeFault {
    fn into_diagnostic(self) -> Diagnostic {
        match self {
            Self::Error(diagnostic) => diagnostic,
            Self::Propagate(_) => Diagnostic::new(
                DiagnosticKind::Runtime,
                "error propagation escaped the program entry point",
                Span::point(1, 1),
            ),
        }
    }
}

impl Default for Interpreter {
    fn default() -> Self {
        Self::new()
    }
}

fn compare(
    left: Value,
    right: Value,
    operator: BinaryOperator,
    span: Span,
    interpreter: &Interpreter,
) -> RuntimeResult<Value> {
    let signed = |a: i128, b: i128| match operator {
        BinaryOperator::Less => a < b,
        BinaryOperator::LessEqual => a <= b,
        BinaryOperator::Greater => a > b,
        BinaryOperator::GreaterEqual => a >= b,
        _ => unreachable!(),
    };
    let unsigned = |a: u128, b: u128| match operator {
        BinaryOperator::Less => a < b,
        BinaryOperator::LessEqual => a <= b,
        BinaryOperator::Greater => a > b,
        BinaryOperator::GreaterEqual => a >= b,
        _ => unreachable!(),
    };
    let float = |a: f64, b: f64| match operator {
        BinaryOperator::Less => a < b,
        BinaryOperator::LessEqual => a <= b,
        BinaryOperator::Greater => a > b,
        BinaryOperator::GreaterEqual => a >= b,
        _ => unreachable!(),
    };
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Bool(signed(a as i128, b as i128))),
        (Value::UInt(a), Value::UInt(b)) => Ok(Value::Bool(unsigned(a as u128, b as u128))),
        (Value::Signed(a, w), Value::Signed(b, v)) if w == v => Ok(Value::Bool(signed(a, b))),
        (Value::Unsigned(a, w), Value::Unsigned(b, v)) if w == v => Ok(Value::Bool(unsigned(a, b))),
        (Value::Float(a), Value::Float(b)) => Ok(Value::Bool(float(a, b))),
        (Value::Float32(a), Value::Float32(b)) => Ok(Value::Bool(float(a as f64, b as f64))),
        _ => Err(interpreter.error("invalid comparison operands", span)),
    }
}

fn checked_signed(
    a: i128,
    b: i128,
    width: u16,
    operator: BinaryOperator,
    span: Span,
    interpreter: &Interpreter,
) -> RuntimeResult<Value> {
    if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder) && b == 0 {
        return Err(interpreter.error("division by zero", span));
    }
    let value = match operator {
        BinaryOperator::Add => a.checked_add(b),
        BinaryOperator::Subtract => a.checked_sub(b),
        BinaryOperator::Multiply => a.checked_mul(b),
        BinaryOperator::Divide => a.checked_div(b),
        BinaryOperator::Remainder => a.checked_rem(b),
        _ => None,
    };
    let value = value.filter(|value| {
        width == 128 || (-(1_i128 << (width - 1))..=(1_i128 << (width - 1)) - 1).contains(value)
    });
    value
        .map(|value| Value::Signed(value, width))
        .ok_or_else(|| interpreter.error(format!("i{width} overflow in arithmetic"), span))
}

fn checked_unsigned(
    a: u128,
    b: u128,
    width: u16,
    operator: BinaryOperator,
    span: Span,
    interpreter: &Interpreter,
) -> RuntimeResult<Value> {
    if matches!(operator, BinaryOperator::Divide | BinaryOperator::Remainder) && b == 0 {
        return Err(interpreter.error("division by zero", span));
    }
    let value = match operator {
        BinaryOperator::Add => a.checked_add(b),
        BinaryOperator::Subtract => a.checked_sub(b),
        BinaryOperator::Multiply => a.checked_mul(b),
        BinaryOperator::Divide => a.checked_div(b),
        BinaryOperator::Remainder => a.checked_rem(b),
        _ => None,
    };
    let value = value.filter(|value| width == 128 || *value < (1_u128 << width));
    value
        .map(|value| Value::Unsigned(value, width))
        .ok_or_else(|| interpreter.error(format!("u{width} overflow in arithmetic"), span))
}

fn value_type_name(value: &Value) -> &str {
    match value {
        Value::Int(_) => "int",
        Value::UInt(_) => "uint",
        Value::Signed(_, 8) => "i8",
        Value::Signed(_, 16) => "i16",
        Value::Signed(_, 32) => "i32",
        Value::Signed(_, 64) => "i64",
        Value::Signed(_, 128) => "i128",
        Value::Signed(_, _) => "<signed>",
        Value::Unsigned(_, 8) => "u8",
        Value::Unsigned(_, 16) => "u16",
        Value::Unsigned(_, 32) => "u32",
        Value::Unsigned(_, 64) => "u64",
        Value::Unsigned(_, 128) => "u128",
        Value::Unsigned(_, _) => "<unsigned>",
        Value::Float(_) => "f64",
        Value::Float32(_) => "f32",
        Value::Reference(_, false) | Value::CaptureReference(_, false) => "&",
        Value::Reference(_, true) | Value::CaptureReference(_, true) => "&mut",
        Value::String(_) => "String",
        Value::CString(_) => "CString",
        Value::CStr(_) => "CStr",
        Value::Memory(_) => "Memory",
        Value::Path(_) => "Path",
        Value::SocketAddress(_) => "SocketAddress",
        Value::TcpStream(_) => "TcpStream",
        Value::TcpListener(_) => "TcpListener",
        Value::Instant(_) => "Instant",
        Value::Duration(_) => "Duration",
        Value::Thread(_) => "Thread",
        Value::Future(_) => "Future",
        Value::Task(_) => "Task",
        Value::Mutex(_) => "Mutex",
        Value::MutexGuard(_) => "MutexGuard",
        Value::AtomicInt(_) => "AtomicInt",
        Value::Array(_) => "Array",
        Value::Slice(_) => "Slice",
        Value::List { .. } => "List",
        Value::Map { .. } => "Map",
        Value::Set { .. } => "Set",
        Value::Char(_) => "char",
        Value::Bool(_) => "bool",
        Value::Struct { type_name, .. } | Value::Enum { type_name, .. } => type_name,
        Value::Unit => "Unit",
        Value::Function(_) | Value::Closure(_) | Value::Constructor { .. } => "<callable>",
        Value::Uninitialized => "<uninitialized>",
    }
}

fn value_is_copy(program: &Program, value: &Value) -> bool {
    match value {
        Value::Int(_)
        | Value::UInt(_)
        | Value::Signed(_, _)
        | Value::Unsigned(_, _)
        | Value::Float(_)
        | Value::Float32(_)
        | Value::Instant(_)
        | Value::Duration(_)
        | Value::Char(_)
        | Value::Bool(_)
        | Value::Reference(_, _)
        | Value::CaptureReference(_, _)
        | Value::CStr(_)
        | Value::Function(_)
        | Value::Constructor { .. }
        | Value::Unit => true,
        Value::Enum {
            type_name, payload, ..
        } if matches!(type_name.as_str(), "Option" | "Result") => {
            payload.iter().all(|value| value_is_copy(program, value))
        }
        Value::Struct { type_name, .. } | Value::Enum { type_name, .. } => {
            program.implementations.iter().any(|implementation| {
                implementation
                    .trait_name
                    .as_ref()
                    .is_some_and(|trait_name| trait_name.name == "Copy")
                    && implementation.target.name == *type_name
            })
        }
        Value::Array(values) => values.iter().all(|value| value_is_copy(program, value)),
        Value::Slice(_) => true,
        Value::List { .. }
        | Value::Map { .. }
        | Value::Set { .. }
        | Value::Thread(_)
        | Value::Future(_)
        | Value::Task(_)
        | Value::Mutex(_)
        | Value::MutexGuard(_)
        | Value::AtomicInt(_)
        | Value::Path(_)
        | Value::SocketAddress(_)
        | Value::TcpStream(_)
        | Value::TcpListener(_)
        | Value::String(_)
        | Value::CString(_)
        | Value::Memory(_)
        | Value::Closure(_)
        | Value::Uninitialized => false,
    }
}

fn coerce_value(value: Value, ty: &crate::ast::TypeName) -> Result<Value, String> {
    let signed_width = match ty.name.as_str() {
        "i8" | "CChar" => Some(8),
        "i16" | "CShort" => Some(16),
        "i32" | "CInt" => Some(32),
        "i64" | "CLongLong" => Some(64),
        "i128" => Some(128),
        _ => None,
    };
    let unsigned_width = match ty.name.as_str() {
        "u8" | "CUChar" => Some(8),
        "u16" | "CUShort" => Some(16),
        "u32" | "CUInt" => Some(32),
        "u64" | "CULongLong" => Some(64),
        "u128" => Some(128),
        _ => None,
    };
    if let Some(width) = signed_width {
        let number = match value {
            Value::Int(value) => value as i128,
            Value::Signed(value, _) => value,
            Value::UInt(value) => value as i128,
            Value::Unsigned(value, _) if value <= i128::MAX as u128 => value as i128,
            Value::Float(value)
                if value.is_finite()
                    && value.fract() == 0.0
                    && value >= i128::MIN as f64
                    && value <= i128::MAX as f64 =>
            {
                value as i128
            }
            Value::Float32(value) if value.is_finite() && value.fract() == 0.0 => value as i128,
            _ => return Err(format!("value cannot be represented as i{width}")),
        };
        let fits = width == 128
            || (-(1_i128 << (width - 1))..=(1_i128 << (width - 1)) - 1).contains(&number);
        return fits
            .then_some(Value::Signed(number, width))
            .ok_or_else(|| format!("value {number} is outside i{width} range"));
    }
    if let Some(width) = unsigned_width {
        let number = match value {
            Value::Int(value) if value >= 0 => value as u128,
            Value::UInt(value) => value as u128,
            Value::Unsigned(value, _) => value,
            Value::Signed(value, _) if value >= 0 => value as u128,
            Value::Float(value)
                if value.is_finite()
                    && value.fract() == 0.0
                    && value >= 0.0
                    && value <= u128::MAX as f64 =>
            {
                value as u128
            }
            Value::Float32(value) if value.is_finite() && value.fract() == 0.0 && value >= 0.0 => {
                value as u128
            }
            _ => return Err(format!("value cannot be represented as u{width}")),
        };
        let fits = width == 128 || number < (1_u128 << width);
        return fits
            .then_some(Value::Unsigned(number, width))
            .ok_or_else(|| format!("value {number} is outside u{width} range"));
    }
    match (ty.name.as_str(), value) {
        ("uint", Value::Int(value)) if value >= 0 => Ok(Value::UInt(value as u64)),
        ("int", Value::UInt(value)) if value <= i64::MAX as u64 => Ok(Value::Int(value as i64)),
        ("f32" | "CFloat", value) => numeric_as_f64(&value)
            .map(|number| Value::Float32(number as f32))
            .ok_or_else(|| "value is not numeric".into()),
        ("f64" | "CDouble", value) => numeric_as_f64(&value)
            .map(Value::Float)
            .ok_or_else(|| "value is not numeric".into()),
        (_, value) => Ok(value),
    }
}

fn numeric_as_f64(value: &Value) -> Option<f64> {
    Some(match value {
        Value::Int(v) => *v as f64,
        Value::UInt(v) => *v as f64,
        Value::Signed(v, _) => *v as f64,
        Value::Unsigned(v, _) => *v as f64,
        Value::Float(v) => *v,
        Value::Float32(v) => *v as f64,
        _ => return None,
    })
}

fn runtime_byte(value: Value) -> Option<u8> {
    match value {
        Value::Unsigned(value, 8) => u8::try_from(value).ok(),
        Value::UInt(value) => u8::try_from(value).ok(),
        Value::Int(value) => u8::try_from(value).ok(),
        _ => None,
    }
}

fn coerce_like(value: Value, target: &Value) -> Result<Value, String> {
    let name = value_type_name(target);
    coerce_value(
        value,
        &crate::ast::TypeName {
            name: name.into(),
            arguments: vec![],
            qualifier: crate::ast::TypeQualifier::Owned,
            span: Span::point(1, 1),
        },
    )
}

fn coerce_numeric_pair(left: Value, right: Value) -> Result<(Value, Value), String> {
    match (&left, &right) {
        (Value::Signed(_, a), Value::Signed(_, b)) if a < b => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Signed(_, a), Value::Signed(_, b)) if b < a => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::Unsigned(_, a), Value::Unsigned(_, b)) if a < b => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Unsigned(_, a), Value::Unsigned(_, b)) if b < a => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::Unsigned(_, unsigned), Value::Signed(_, signed)) if signed > unsigned => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Signed(_, signed), Value::Unsigned(_, unsigned)) if signed > unsigned => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::Int(_), Value::Signed(_, width)) if *width >= 128 => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Signed(_, width), Value::Int(_)) if *width >= 128 => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::UInt(_), Value::Unsigned(_, width)) if *width >= 128 => {
            Ok((coerce_like(left, &right)?, right))
        }
        (Value::Unsigned(_, width), Value::UInt(_)) if *width >= 128 => {
            Ok((left.clone(), coerce_like(right, &left)?))
        }
        (Value::Float32(_), Value::Float(_)) => Ok((coerce_like(left, &right)?, right)),
        (Value::Float(_), Value::Float32(_)) => Ok((left.clone(), coerce_like(right, &left)?)),
        (
            Value::Signed(_, _) | Value::Unsigned(_, _) | Value::UInt(_) | Value::Float32(_),
            Value::Int(_) | Value::Float(_),
        ) => Ok((left.clone(), coerce_like(right, &left)?)),
        _ => Ok((left, right)),
    }
}

fn is_numeric_name(name: &str) -> bool {
    matches!(
        name,
        "int"
            | "uint"
            | "i8"
            | "i16"
            | "i32"
            | "i64"
            | "i128"
            | "u8"
            | "u16"
            | "u32"
            | "u64"
            | "u128"
            | "f32"
            | "f64"
    )
}

fn integer_method(method: &str, left: Value, right: Value) -> Result<Value, String> {
    let wrapping = method.starts_with("wrapping_");
    let operation = method.rsplit('_').next().unwrap();
    match (left, right) {
        (Value::Int(a), Value::Int(b)) => Ok(Value::Int(match (wrapping, operation) {
            (true, "add") => a.wrapping_add(b),
            (true, "sub") => a.wrapping_sub(b),
            (true, "mul") => a.wrapping_mul(b),
            (false, "add") => a.saturating_add(b),
            (false, "sub") => a.saturating_sub(b),
            (false, "mul") => a.saturating_mul(b),
            _ => unreachable!(),
        })),
        (Value::UInt(a), Value::UInt(b)) => Ok(Value::UInt(match (wrapping, operation) {
            (true, "add") => a.wrapping_add(b),
            (true, "sub") => a.wrapping_sub(b),
            (true, "mul") => a.wrapping_mul(b),
            (false, "add") => a.saturating_add(b),
            (false, "sub") => a.saturating_sub(b),
            (false, "mul") => a.saturating_mul(b),
            _ => unreachable!(),
        })),
        (Value::Signed(a, width), Value::Signed(b, other)) if width == other => {
            let (min, max) = if width == 128 {
                (i128::MIN, i128::MAX)
            } else {
                (-(1_i128 << (width - 1)), (1_i128 << (width - 1)) - 1)
            };
            let value = if wrapping {
                let raw = match operation {
                    "add" => a.wrapping_add(b),
                    "sub" => a.wrapping_sub(b),
                    "mul" => a.wrapping_mul(b),
                    _ => unreachable!(),
                };
                if width == 128 {
                    raw
                } else {
                    let modulus = 1_i128 << width;
                    let masked = raw & (modulus - 1);
                    if masked > max {
                        masked - modulus
                    } else {
                        masked
                    }
                }
            } else {
                match operation {
                    "add" => a.saturating_add(b),
                    "sub" => a.saturating_sub(b),
                    "mul" => a.saturating_mul(b),
                    _ => unreachable!(),
                }
                .clamp(min, max)
            };
            Ok(Value::Signed(value, width))
        }
        (Value::Unsigned(a, width), Value::Unsigned(b, other)) if width == other => {
            let max = if width == 128 {
                u128::MAX
            } else {
                (1_u128 << width) - 1
            };
            let value = if wrapping {
                let raw = match operation {
                    "add" => a.wrapping_add(b),
                    "sub" => a.wrapping_sub(b),
                    "mul" => a.wrapping_mul(b),
                    _ => unreachable!(),
                };
                raw & max
            } else {
                match operation {
                    "add" => a.saturating_add(b),
                    "sub" => a.saturating_sub(b),
                    "mul" => a.saturating_mul(b),
                    _ => unreachable!(),
                }
                .min(max)
            };
            Ok(Value::Unsigned(value, width))
        }
        _ => Err(format!("invalid operands for `{method}`")),
    }
}

fn find_variants<'a>(
    program: &'a Program,
    name: &str,
) -> Vec<(&'a EnumDeclaration, &'a VariantDeclaration)> {
    program
        .enums
        .iter()
        .flat_map(|owner| {
            owner
                .variants
                .iter()
                .filter(move |variant| variant.name == name)
                .map(move |variant| (owner, variant))
        })
        .collect()
}

fn find_qualified_variant<'a>(
    program: &'a Program,
    type_name: &str,
    variant_name: &str,
) -> Option<(&'a EnumDeclaration, &'a VariantDeclaration)> {
    let owner = program.enums.iter().find(|owner| owner.name == type_name)?;
    let variant = owner
        .variants
        .iter()
        .find(|variant| variant.name == variant_name)?;
    Some((owner, variant))
}

fn pattern_matches(
    pattern: &Pattern,
    value: &Value,
    bindings: &mut HashMap<String, Value>,
) -> bool {
    match pattern {
        Pattern::Wildcard => true,
        Pattern::Binding(name) => {
            bindings.insert(name.clone(), value.clone());
            true
        }
        Pattern::Integer(pattern) => match value {
            Value::Int(value) => *value >= 0 && *value as u128 == *pattern,
            Value::UInt(value) => *value as u128 == *pattern,
            Value::Signed(value, _) => *value >= 0 && *value as u128 == *pattern,
            Value::Unsigned(value, _) => *value == *pattern,
            _ => false,
        },
        Pattern::String(pattern) => matches!(value, Value::String(value) if value.text == *pattern),
        Pattern::Character(pattern) => matches!(value, Value::Char(value) if value == pattern),
        Pattern::Bool(pattern) => matches!(value, Value::Bool(value) if value == pattern),
        Pattern::Variant {
            type_name,
            variant,
            arguments,
        } => {
            let Value::Enum {
                type_name: value_type,
                variant: value_variant,
                payload,
            } = value
            else {
                return false;
            };
            if value_variant != variant
                || type_name
                    .as_ref()
                    .is_some_and(|pattern_type| pattern_type != value_type)
                || arguments.len() != payload.len()
            {
                return false;
            }
            arguments
                .iter()
                .zip(payload)
                .all(|(pattern, value)| pattern_matches(&pattern.node, value, bindings))
        }
    }
}

fn display_value(value: Value) -> String {
    match value {
        Value::Int(value) => value.to_string(),
        Value::UInt(value) => value.to_string(),
        Value::Signed(value, _) => value.to_string(),
        Value::Unsigned(value, _) => value.to_string(),
        Value::Float(value) => value.to_string(),
        Value::Float32(value) => value.to_string(),
        Value::Reference(_, false) | Value::CaptureReference(_, false) => {
            "<shared reference>".into()
        }
        Value::Reference(_, true) | Value::CaptureReference(_, true) => {
            "<mutable reference>".into()
        }
        Value::String(value) => value.text,
        Value::CString(value) | Value::CStr(value) => value.text(),
        Value::Memory(_) => "<Memory>".into(),
        Value::Path(value) => value.to_string_lossy().into_owned(),
        Value::SocketAddress(value) => format!("{}:{}", value.host, value.port),
        Value::TcpStream(_) => "<TcpStream>".into(),
        Value::TcpListener(_) => "<TcpListener>".into(),
        Value::Instant(_) => "<Instant>".into(),
        Value::Duration(value) => format!("{}ns", value.as_nanos()),
        Value::Thread(_) => "<Thread>".into(),
        Value::Future(_) => "<Future>".into(),
        Value::Task(_) => "<Task>".into(),
        Value::Mutex(_) => "<Mutex>".into(),
        Value::MutexGuard(_) => "<MutexGuard>".into(),
        Value::AtomicInt(_) => "<AtomicInt>".into(),
        Value::Array(values) => format!(
            "[{}]",
            values
                .into_iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Slice(values) => format!(
            "[{}]",
            values
                .into_iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::List { values, .. } => format!(
            "[{}]",
            values
                .into_iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Map { entries, .. } => format!(
            "{{{}}}",
            entries
                .into_iter()
                .map(|(key, value)| format!("{}: {}", display_value(key), display_value(value)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Set { values, .. } => format!(
            "{{{}}}",
            values
                .into_iter()
                .map(display_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Char(value) => value.to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Function(name) => format!("<fn {name}>"),
        Value::Closure(_) => "<closure>".into(),
        Value::Constructor { type_name, variant } => format!("<{type_name}.{variant}>"),
        Value::Struct { type_name, .. } => format!("{type_name} {{ .. }}"),
        Value::Enum {
            type_name,
            variant,
            payload,
        } => {
            if payload.is_empty() {
                format!("{type_name}.{variant}")
            } else {
                let values = payload
                    .into_iter()
                    .map(display_value)
                    .collect::<Vec<_>>()
                    .join(", ");
                format!("{type_name}.{variant}({values})")
            }
        }
        Value::Unit => "()".into(),
        Value::Uninitialized => "<uninitialized>".into(),
    }
}

#[cfg(test)]
mod tests {
    use crate::run_source;

    #[test]
    fn executes_functions_control_flow_and_assignments() {
        let output = run_source(
            "fn sum_to(n: int) -> int { var total = 0 var i = 0 while i <= n { total += i i += 1 } return total } fn main() { print(sum_to(10)) }",
        )
        .expect("program should run");
        assert_eq!(output, ["55"]);
    }

    #[test]
    fn executes_exclusive_and_inclusive_for_ranges() {
        let output = run_source(
            "fn main() { var total = 0 for i in 0..5 { total += i } print(total) var count = 0 for i in 0..=5 { count += 1 } print(count) }",
        )
        .expect("for ranges should run");
        assert_eq!(output, ["10", "6"]);
    }

    #[test]
    fn return_break_and_continue_propagate_correctly() {
        let output = run_source(
            "fn first() -> int { var i = 0 loop { i += 1 if i == 2 { continue } if i == 4 { return i } } } fn main() { print(first()) }",
        )
        .expect("program should run");
        assert_eq!(output, ["4"]);
    }

    #[test]
    fn logical_operators_short_circuit() {
        let output =
            run_source("fn main() { print(false && (1 / 0 == 0)) print(true || (1 / 0 == 0)) }")
                .expect("short-circuited expressions must not divide by zero");
        assert_eq!(output, ["false", "true"]);
    }

    #[test]
    fn executes_recursive_functions_with_isolated_call_frames() {
        let output = run_source(
            "fn factorial(n: int) -> int { if n <= 1 { return 1 } else { return n * factorial(n - 1) } } fn main() { print(factorial(6)) }",
        )
        .expect("recursive program should run");
        assert_eq!(output, ["720"]);
    }

    #[test]
    fn reports_checked_integer_failures() {
        let error = run_source("fn main() { print(9223372036854775807 + 1) }")
            .expect_err("overflow should fail");
        assert!(error.message.contains("overflow"));
        let error = run_source("fn main() { print(1 / 0) }").expect_err("division should fail");
        assert!(error.message.contains("zero"));
    }

    #[test]
    fn limits_recursive_runtime_calls() {
        let error =
            run_source("fn recurse() -> int { return recurse() } fn main() { print(recurse()) }")
                .expect_err("unbounded recursion should be stopped");
        assert!(error.message.contains("call depth"));
    }
}
