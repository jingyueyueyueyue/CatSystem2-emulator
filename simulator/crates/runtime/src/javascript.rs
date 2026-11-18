//! Portable JavaScript task, using Boa 0.21.0 (MIT OR Unlicense).
//!
//! The interpreter and its suspended future stay on one owned thread. Only
//! UTF-16 strings, primitive values and explicit 32-bit object identities cross
//! the bridge; neither borrowed host state nor VM/native pointers cross it.
//! A bytecode-budget yield keeps the same future, including call frames.
use std::cell::{Cell, RefCell};
use std::future::Future;
use std::pin::pin;
use std::rc::Rc;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc::{self, Receiver, Sender, SyncSender, TryRecvError},
};
use std::task::{Context as PollContext, Poll, Wake, Waker};

use boa_engine::job::{Job, JobExecutor};
use boa_engine::{
    Context, JsNativeError, JsResult, JsString, JsValue, NativeFunction, Script, Source,
};

use crate::{RuntimeError, RuntimeResult};

const MAX_SOURCE: usize = 16 * 1024 * 1024;
const MAX_ARGUMENTS: usize = 64;
const MAX_STRING: usize = 1024 * 1024;

/// Strings are UTF-16, including unpaired code units. The platform must use
/// its actual script encoding and report unrepresentable resource names.
#[derive(Clone, Debug, PartialEq)]
pub enum JavaScriptValue {
    Undefined,
    Null,
    Boolean(bool),
    Number(f64),
    String(Vec<u16>),
}

impl JavaScriptValue {
    /// ECMAScript ToUint32 for an already converted numeric primitive.
    /// This does not cast a native pointer or truncate a 64-bit host handle.
    pub fn uint32(&self) -> RuntimeResult<u32> {
        let value = match self {
            Self::Number(value) => *value,
            Self::Boolean(value) => f64::from(u8::from(*value)),
            Self::Null => 0.0,
            Self::Undefined => f64::NAN,
            Self::String(_) => return Err(error("bridge expected a converted numeric value")),
        };
        if !value.is_finite() || value == 0.0 {
            return Ok(0);
        }
        Ok(value.trunc().rem_euclid(4294967296.0) as u32)
    }

    pub fn number(&self) -> RuntimeResult<f64> {
        match self {
            Self::Number(value) => Ok(*value),
            _ => Err(error("bridge expected a number")),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct JavaScriptCall {
    pub operation: String,
    pub arguments: Vec<JavaScriptValue>,
}

/// None means an actual suspended host operation. The task retains the exact
/// request and reply channel and calls it again with identical arguments.
pub trait JavaScriptHost {
    fn javascript_call(
        &mut self,
        call: &JavaScriptCall,
        budget: usize,
    ) -> RuntimeResult<Option<JavaScriptValue>>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JavaScriptStep {
    Pending,
    Complete,
}

/// The actual type55 logical callback arguments. `value` is message data1;
/// `kind` is one for command9 and zero for command10. Coordinates are already
/// converted by the input service; they must not be converted a second time.
/// The current v4.01 adapter preserves its duplicated-Y callback argument;
/// the original point remains available to cvMouseX/Y in the host bridge.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JavaScriptInput {
    pub value: u32,
    pub kind: u32,
    pub pointer: [i32; 2],
}

enum WorkerCommand {
    Initialize,
    Frame(u32),
    Function(String, Vec<Vec<u16>>),
    Input(JavaScriptInput),
    Continue,
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum Invocation {
    Initialize(u32),
    Frame(u32),
    Function(String, Vec<Vec<u16>>),
    Input(JavaScriptInput),
}
enum WorkerEvent {
    Call(JavaScriptCall, SyncSender<Result<JavaScriptValue, String>>),
    Yield,
    Complete,
    Error(String),
}

struct Bridge {
    events: Sender<WorkerEvent>,
    cancelled: Arc<AtomicBool>,
}

thread_local! {
    static BRIDGE: RefCell<Option<Bridge>> = const { RefCell::new(None) };
}

/// Dropping a task cancels a pending bytecode poll or bridge call. A worker
/// never owns the scene resources: the platform releases those explicitly.
pub struct JavaScriptTask {
    name: Vec<u8>,
    commands: Sender<WorkerCommand>,
    events: Receiver<WorkerEvent>,
    cancelled: Arc<AtomicBool>,
    started: bool,
    initialized: bool,
    invocation: Option<Invocation>,
    yielded: bool,
    pending: Option<(JavaScriptCall, SyncSender<Result<JavaScriptValue, String>>)>,
    failed: Option<String>,
}

impl JavaScriptTask {
    /// Source decoding precedes creation, using the resource's actual encoding.
    /// The original name bytes remain available for diagnostics/provenance.
    pub fn new(name: Vec<u8>, source: String, bytecode_budget: u32) -> RuntimeResult<Self> {
        if source.len() > MAX_SOURCE || name.len() > 4096 {
            return Err(error("JavaScript source/name exceeds its storage limit"));
        }
        if source.contains('\0') {
            return Err(error("JavaScript source contains an embedded NUL"));
        }
        let (commands, incoming) = mpsc::channel();
        let (outgoing, events) = mpsc::channel();
        let cancelled = Arc::new(AtomicBool::new(false));
        let worker_cancelled = cancelled.clone();
        std::thread::Builder::new()
            .name("cs2-javascript".into())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    worker(
                        source,
                        bytecode_budget.max(1),
                        incoming,
                        outgoing.clone(),
                        worker_cancelled,
                    )
                }));
                if result.is_err() {
                    let _ =
                        outgoing.send(WorkerEvent::Error("JavaScript interpreter panicked".into()));
                }
            })
            .map_err(|cause| error(&format!("cannot create JavaScript interpreter: {cause}")))?;
        Ok(Self {
            name,
            commands,
            events,
            cancelled,
            started: false,
            initialized: false,
            invocation: None,
            yielded: false,
            pending: None,
            failed: None,
        })
    }

    pub fn resource_name(&self) -> &[u8] {
        &self.name
    }
    pub fn initialized(&self) -> bool {
        self.initialized
    }

    /// Initialize once, then execute one GameLoop call for this task time.
    /// A Pending result must be resumed with the same time before advancing it.
    pub fn advance(
        &mut self,
        time: u32,
        host: &mut impl JavaScriptHost,
        budget: usize,
    ) -> RuntimeResult<JavaScriptStep> {
        let invocation = if self.initialized {
            Invocation::Frame(time)
        } else {
            Invocation::Initialize(time)
        };
        self.poll_invocation(invocation, host, budget)
    }

    /// Synchronous KCS command523 invokes the named function with the actual
    /// captured string-table values, retaining it across bytecode/host yields.
    pub fn call_function(
        &mut self,
        name: String,
        arguments: Vec<Vec<u16>>,
        host: &mut impl JavaScriptHost,
        budget: usize,
    ) -> RuntimeResult<JavaScriptStep> {
        if !self.initialized {
            return Err(error("cannot call an unloaded JavaScript program"));
        }
        if name.len() > 4096
            || arguments.len() > MAX_ARGUMENTS
            || arguments.iter().any(|value| value.len() > MAX_STRING)
        {
            return Err(error(
                "JavaScript function invocation exceeds bridge limits",
            ));
        }
        self.poll_invocation(Invocation::Function(name, arguments), host, budget)
    }

    pub fn dispatch_input(
        &mut self,
        input: JavaScriptInput,
        host: &mut impl JavaScriptHost,
        budget: usize,
    ) -> RuntimeResult<JavaScriptStep> {
        if !self.initialized {
            return Err(error(
                "JavaScript input cannot run before source initialization",
            ));
        }
        self.poll_invocation(Invocation::Input(input), host, budget)
    }

    fn poll_invocation(
        &mut self,
        invocation: Invocation,
        host: &mut impl JavaScriptHost,
        budget: usize,
    ) -> RuntimeResult<JavaScriptStep> {
        if let Some(cause) = &self.failed {
            return Err(error(cause));
        }
        if budget == 0 {
            return Ok(JavaScriptStep::Pending);
        }
        if self
            .invocation
            .as_ref()
            .is_some_and(|saved| saved != &invocation)
        {
            return Err(error(
                "JavaScript budget continuation changed its invocation",
            ));
        }
        if !self.started {
            let command = match &invocation {
                Invocation::Initialize(_) => WorkerCommand::Initialize,
                Invocation::Frame(time) => WorkerCommand::Frame(*time),
                Invocation::Function(name, args) => {
                    WorkerCommand::Function(name.clone(), args.clone())
                }
                Invocation::Input(input) => WorkerCommand::Input(*input),
            };
            self.invocation = Some(invocation);
            self.send(command)?;
            self.started = true;
        } else if self.yielded {
            self.send(WorkerCommand::Continue)?;
            self.yielded = false;
        }
        for _ in 0..budget {
            if let Some((call, reply)) = self.pending.take() {
                match host.javascript_call(&call, budget) {
                    Ok(Some(value)) => {
                        reply
                            .send(Ok(value))
                            .map_err(|_| error("JavaScript bridge reply was abandoned"))?;
                    }
                    Ok(None) => {
                        self.pending = Some((call, reply));
                        return Ok(JavaScriptStep::Pending);
                    }
                    Err(cause) => {
                        // Tell the actual interpreter to throw. Keep the host
                        // failure sticky even if game code catches the error.
                        let message = cause.to_string();
                        let _ = reply.send(Err(message.clone()));
                        self.failed = Some(message);
                        return Err(cause);
                    }
                }
                continue;
            }
            match self.events.try_recv() {
                Ok(WorkerEvent::Call(call, reply)) => self.pending = Some((call, reply)),
                Ok(WorkerEvent::Yield) => {
                    self.yielded = true;
                    return Ok(JavaScriptStep::Pending);
                }
                Ok(WorkerEvent::Complete) => {
                    self.started = false;
                    self.invocation = None;
                    self.initialized = true;
                    return Ok(JavaScriptStep::Complete);
                }
                Ok(WorkerEvent::Error(cause)) => {
                    self.failed = Some(cause.clone());
                    return Err(error(&cause));
                }
                Err(TryRecvError::Empty) => return Ok(JavaScriptStep::Pending),
                Err(TryRecvError::Disconnected) => {
                    return Err(error("JavaScript interpreter disconnected"));
                }
            }
        }
        Ok(JavaScriptStep::Pending)
    }

    fn send(&self, command: WorkerCommand) -> RuntimeResult<()> {
        self.commands
            .send(command)
            .map_err(|_| error("JavaScript interpreter is unavailable"))
    }
}

impl Drop for JavaScriptTask {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
        // A pending bridge wakes on reply; a suspended future wakes on Continue.
        if let Some((_, reply)) = self.pending.take() {
            let _ = reply.send(Err("JavaScript task was released".into()));
        }
        let _ = self.commands.send(WorkerCommand::Continue);
    }
}

struct NoWake;
impl Wake for NoWake {
    fn wake(self: Arc<Self>) {}
}

// The original task's promise/microtask checkpoint is not yet recovered.
// A real queued continuation must fail explicitly rather than disappearing
// into the default queue and producing an apparently successful blank task.
#[derive(Default)]
struct PendingJobs(Cell<bool>);
impl JobExecutor for PendingJobs {
    fn enqueue_job(self: Rc<Self>, _job: Job, _context: &mut Context) {
        self.0.set(true);
    }
    fn run_jobs(self: Rc<Self>, _context: &mut Context) -> JsResult<()> {
        Err(js_error(
            "CatSystem JavaScript microtask checkpoint is not recovered",
        ))
    }
}

fn worker(
    source: String,
    budget: u32,
    commands: Receiver<WorkerCommand>,
    events: Sender<WorkerEvent>,
    cancelled: Arc<AtomicBool>,
) {
    BRIDGE.with(|cell| {
        *cell.borrow_mut() = Some(Bridge {
            events: events.clone(),
            cancelled: cancelled.clone(),
        });
    });
    let run = || -> Result<(), String> {
        let jobs = Rc::new(PendingJobs::default());
        let mut context = Context::builder()
            .job_executor(jobs.clone())
            .build()
            .map_err(|cause| cause.to_string())?;
        context
            .register_global_builtin_callable(
                JsString::from("__cs2_bridge"),
                2,
                NativeFunction::from_fn_ptr(bridge_call),
            )
            .map_err(|cause| cause.to_string())?;
        // Prelude is our own binding code. Parsing and execution are also
        // budgeted; this is not a separate sample-specific interpreter.
        let bootstrap = Script::parse(
            Source::from_bytes(include_str!("javascript_objects.js")),
            None,
            &mut context,
        )
        .map_err(|cause| cause.to_string())?;
        let mut bootstrapped = false;
        loop {
            if cancelled.load(Ordering::Acquire) {
                return Ok(());
            }
            let command = commands
                .recv()
                .map_err(|_| "JavaScript owner was released".to_string())?;
            let script = match command {
                WorkerCommand::Initialize => {
                    if bootstrapped {
                        return Err("JavaScript source was initialized twice".into());
                    }
                    drive(
                        bootstrap.evaluate_async_with_budget(&mut context, budget),
                        &commands,
                        &events,
                        &cancelled,
                        &jobs,
                    )?;
                    bootstrapped = true;
                    Script::parse(Source::from_bytes(source.as_bytes()), None, &mut context)
                }
                WorkerCommand::Frame(time) => {
                    let frame = format!("__cs2_frame({time});");
                    Script::parse(Source::from_bytes(frame.as_bytes()), None, &mut context)
                }
                WorkerCommand::Function(name, arguments) => {
                    let name = utf16_literal(&name.encode_utf16().collect::<Vec<_>>());
                    let args = arguments
                        .iter()
                        .map(|value| utf16_literal(value))
                        .collect::<Vec<_>>()
                        .join(",");
                    let call = format!("__cs2_call_named({name},[{args}]);");
                    Script::parse(Source::from_bytes(call.as_bytes()), None, &mut context)
                }
                WorkerCommand::Input(input) => {
                    // v4.01 0050ec15 and 0050ec29 both convert EBX (Y).
                    let call = format!(
                        "__cs2_input({},{},{},{});",
                        input.value as i32, input.kind as i32, input.pointer[1], input.pointer[1]
                    );
                    Script::parse(Source::from_bytes(call.as_bytes()), None, &mut context)
                }
                WorkerCommand::Continue => return Err("unexpected JavaScript continuation".into()),
            }
            .map_err(|cause| cause.to_string())?;
            drive(
                script.evaluate_async_with_budget(&mut context, budget),
                &commands,
                &events,
                &cancelled,
                &jobs,
            )?;
            if events.send(WorkerEvent::Complete).is_err() {
                return Ok(());
            }
        }
    };
    if let Err(cause) = run()
        && !cancelled.load(Ordering::Acquire)
    {
        let _ = events.send(WorkerEvent::Error(cause));
    }
    BRIDGE.with(|cell| {
        cell.borrow_mut().take();
    });
}

fn drive(
    future: impl Future<Output = JsResult<JsValue>>,
    commands: &Receiver<WorkerCommand>,
    events: &Sender<WorkerEvent>,
    cancelled: &AtomicBool,
    jobs: &PendingJobs,
) -> Result<(), String> {
    let mut future = pin!(future);
    let waker = Waker::from(Arc::new(NoWake));
    let mut cx = PollContext::from_waker(&waker);
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("JavaScript task was released".into());
        }
        let result = future.as_mut().poll(&mut cx);
        if jobs.0.get() {
            return Err("CatSystem JavaScript microtask checkpoint is not recovered".into());
        }
        match result {
            Poll::Ready(result) => return result.map(|_| ()).map_err(|cause| cause.to_string()),
            Poll::Pending => {
                events
                    .send(WorkerEvent::Yield)
                    .map_err(|_| "JavaScript owner was released".to_string())?;
                match commands.recv() {
                    Ok(WorkerCommand::Continue) => {}
                    _ => return Err("JavaScript continuation protocol changed".into()),
                }
            }
        }
    }
}

fn bridge_call(_this: &JsValue, args: &[JsValue], context: &mut Context) -> JsResult<JsValue> {
    let operation = args
        .first()
        .ok_or_else(|| js_error("missing bridge operation"))?
        .to_string(context)?
        .to_std_string()
        .map_err(|_| js_error("bridge operation is not Unicode"))?;
    let array = args
        .get(1)
        .and_then(JsValue::as_object)
        .ok_or_else(|| js_error("bridge argument list is not an object"))?;
    let count = array
        .get(JsString::from("length"), context)?
        .to_u32(context)? as usize;
    if count > MAX_ARGUMENTS {
        return Err(js_error("too many JavaScript host arguments"));
    }
    let mut arguments = Vec::with_capacity(count);
    for index in 0..count {
        let value = array.get(index as u32, context)?;
        arguments.push(if value.is_undefined() {
            JavaScriptValue::Undefined
        } else if value.is_null() {
            JavaScriptValue::Null
        } else if let Some(value) = value.as_boolean() {
            JavaScriptValue::Boolean(value)
        } else if let Some(value) = value.as_number() {
            JavaScriptValue::Number(value)
        } else if let Some(value) = value.as_string() {
            if value.len() > MAX_STRING {
                return Err(js_error("JavaScript bridge string exceeds 1 Mi code units"));
            }
            JavaScriptValue::String(value.to_vec())
        } else {
            return Err(js_error(
                "host argument must be a primitive or a registered object identity",
            ));
        });
    }
    let (reply, receive) = mpsc::sync_channel(1);
    BRIDGE.with(|cell| -> JsResult<()> {
        let cell = cell.borrow();
        let bridge = cell
            .as_ref()
            .ok_or_else(|| js_error("JavaScript bridge is unavailable"))?;
        if bridge.cancelled.load(Ordering::Acquire) {
            return Err(js_error("JavaScript task was released"));
        }
        bridge
            .events
            .send(WorkerEvent::Call(
                JavaScriptCall {
                    operation,
                    arguments,
                },
                reply,
            ))
            .map_err(|_| js_error("JavaScript owner was released"))
    })?;
    let value = receive
        .recv()
        .map_err(|_| js_error("JavaScript host abandoned its reply"))?
        .map_err(|cause| js_error(&cause))?;
    Ok(match value {
        JavaScriptValue::Undefined => JsValue::undefined(),
        JavaScriptValue::Null => JsValue::null(),
        JavaScriptValue::Boolean(value) => value.into(),
        JavaScriptValue::Number(value) => value.into(),
        JavaScriptValue::String(value) => JsString::from(value.as_slice()).into(),
    })
}

fn js_error(message: &str) -> boa_engine::JsError {
    JsNativeError::error().with_message(message.to_owned()).into()
}

fn utf16_literal(value: &[u16]) -> String {
    use std::fmt::Write;
    let mut text = String::from("\"");
    for unit in value {
        let _ = write!(&mut text, "\\u{unit:04x}");
    }
    text.push('"');
    text
}

fn error(message: &str) -> RuntimeError {
    RuntimeError::Host(format!("JavaScript: {message}"))
}
