//! The native engine: one driver child process per session (`spec/toolchain.md` 1.14; `spec/cabi.md` 3.8). Owner:
//! the native-engine track (N).

use crate::protocol::{self, FaultAnswer, Request, Response};
use crate::wire;
use cpl_diag::{Category, DiagCode, Diagnostic};
use cpl_emit_c::package::{DriverBuild, DriverConfig};
use cpl_emit_c::InteropPlan;
use cpl_eval::{
    Budget, EvalConfig, Fault, Handle, HandleId, HostSource, RangeSide, RunEngine, TestEngine,
    Value,
};
use cpl_hir::{DefId, LocalId, ParamDef, Program};
use cpl_project::Project;
use serde::Deserialize;
use serde_json::json;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::rc::Rc;

struct Inner {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    renderings: Option<Renderings>,
    error: Option<NativeError>,
    violation: Option<(DefId, Fault)>,
    dead: bool,
    /// How the driver ended, once it died: the `<status>` of the evaluator's `CAdapter` (`spec/cabi.md` 7.2 item 4).
    end: Option<cpl_eval::boundary::HostEnd>,
    /// The watchdog of each request: a driver that does not answer within it is killed (toolchain 12.6).
    timeout: Option<std::time::Duration>,
}

/// A handle to one driver child: clones share the child, so that the tests of one `cpl test` run share one driver.
#[derive(Clone)]
pub struct NativeSession<'p> {
    p: &'p Program,
    inner: Rc<RefCell<Inner>>,
    build: Rc<DriverBuild>,
}

fn internal(kind: &str, detail: String) -> Diagnostic {
    Diagnostic::error(
        DiagCode("internal_error"),
        Category::Internal,
        format!("internal error ({kind}): {detail}"),
    )
    .with_data("kind", json!(kind))
    .with_data("detail", json!(detail))
}

/// The static code of an error a driver sent (`interop_*` or `internal_error`).
fn static_code(code: &str) -> &'static str {
    for c in cpl_rtspec::interop::CODES.iter() {
        if c.code == code {
            return c.code;
        }
    }
    "internal_error"
}

fn diagnostic_of(e: &cpl_diag::json::DiagnosticJson) -> Diagnostic {
    let mut d = Diagnostic::error(
        DiagCode(static_code(&e.code)),
        e.category,
        e.message.clone(),
    );
    for n in &e.notes {
        d = d.with_note(n.message.clone(), None);
    }
    for (k, v) in &e.data {
        d = d.with_data(k, v.clone());
    }
    d
}

/// One response line read with serde_json's recursion limit disabled (T71): fault payloads can be as deep as the
/// values they hold.
fn parse_json<T: serde::de::DeserializeOwned>(line: &str) -> Result<T, serde_json::Error> {
    let mut de = serde_json::Deserializer::from_str(line);
    de.disable_recursion_limit();
    let r = T::deserialize(&mut de)?;
    de.end()?;
    Ok(r)
}

fn parse_response(line: &str) -> Result<Response, serde_json::Error> {
    parse_json(line)
}

/// Value-returning requests decode the flat nodes directly, avoiding the untagged response's temporary JSON tree.
/// Faults and other answers retain the ordinary protocol parser, including arbitrary-precision diagnostic data.
pub(crate) enum ValueResponse {
    Value(wire::Nodes),
    Other(Response),
}

fn parse_value_response(line: &str) -> Result<ValueResponse, serde_json::Error> {
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Answer {
        ok: wire::Nodes,
    }
    if let Ok(answer) = parse_json::<Answer>(line) {
        return Ok(ValueResponse::Value(answer.ok));
    }
    match parse_response(line)? {
        Response::Ok { ok } => serde_json::from_value(ok).map(ValueResponse::Value),
        other => Ok(ValueResponse::Other(other)),
    }
}

#[cfg(test)]
mod response_tests {
    use super::*;

    #[test]
    fn value_answers_stream_nodes_without_changing_response_precedence() {
        for line in [
            r#"{"ok":{"nodes":[["s","λ\u0000"],["l",0,0]]}}"#,
            r#"{"extension":true,"ok":{"nodes":[["s","λ\u0000"],["l",0,0]]}}"#,
        ] {
            let ValueResponse::Value(actual) = parse_value_response(line).unwrap() else {
                panic!("a value answer");
            };
            let Response::Ok { ok } = parse_response(line).unwrap() else {
                panic!("an ok answer");
            };
            assert_eq!(actual, serde_json::from_value::<wire::Nodes>(ok).unwrap());
        }
        for line in [
            r#"{"handle":7,"ok":{"nodes":[["t"]]}}"#,
            r#"{"ok":{"nodes":[["t"]]},"handle":7}"#,
        ] {
            assert!(matches!(
                parse_value_response(line).unwrap(),
                ValueResponse::Other(Response::Handle { handle: 7 })
            ));
        }
        for line in [
            r#"{"ok":{"nodes":[["i","007"]]}}"#,
            r#"{"ok":{"nodes":[["l",-1]]}}"#,
            r#"{"ok":{"nodes":[["t"]]}} trailing"#,
        ] {
            assert!(parse_value_response(line).is_err(), "{line}");
        }
    }
}

impl<'p> NativeSession<'p> {
    /// Build or reuse the driver (`cpl_emit_c::package::driver`), spawn it, refuse a banner whose program digest is not
    /// the one built (`spec/cabi.md` 3.8), and send `configure` with `cfg`'s `max_call_depth`, `time_budget`, and
    /// `check_contracts` and with `max_heap_bytes` 0 (no heap budget).
    pub fn start(
        p: &'p Program,
        project: &Project,
        plan: &InteropPlan,
        cfg: &EvalConfig,
        driver: &DriverConfig,
    ) -> Result<Self, NativeError> {
        let build =
            cpl_emit_c::package::driver(project, p, plan, driver).map_err(NativeError::Build)?;
        let mut child = Command::new(&build.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                NativeError::DriverFailed(internal(
                    "native_driver_failed",
                    format!("the driver cannot start: {e}"),
                ))
            })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let mut banner = String::new();
        if stdout.read_line(&mut banner).map_err(|e| {
            NativeError::DriverFailed(internal(
                "native_driver_failed",
                format!("the driver sent no banner: {e}"),
            ))
        })? == 0
        {
            return Err(NativeError::DriverFailed(internal(
                "native_driver_failed",
                "the driver exited before its banner".to_string(),
            )));
        }
        let parsed: protocol::Banner = serde_json::from_str(banner.trim_end()).map_err(|e| {
            NativeError::Protocol(internal(
                "native_protocol",
                format!("the driver's banner is malformed: {e}"),
            ))
        })?;
        if parsed.protocol != 1 || parsed.program != build.program_digest.as_str() {
            return Err(NativeError::Protocol(internal(
                "native_protocol",
                format!(
                    "the driver runs program {} with protocol {}, not program {}",
                    parsed.program,
                    parsed.protocol,
                    build.program_digest.as_str()
                ),
            )));
        }
        let session = NativeSession {
            p,
            inner: Rc::new(RefCell::new(Inner {
                child,
                stdin,
                stdout,
                renderings: None,
                error: None,
                violation: None,
                dead: false,
                end: None,
                timeout: None,
            })),
            build: Rc::new(build),
        };
        let mut s = session;
        s.reconfigure(cfg)?;
        Ok(s)
    }

    /// The driver as built: its digests are the receipt's `native` object (toolchain 8.2).
    pub fn build(&self) -> &DriverBuild {
        &self.build
    }

    fn request(&self, req: &Request) -> Result<Response, NativeError> {
        request_on(&self.inner, req, parse_response)
    }

    fn request_value(&self, req: &Request) -> Result<ValueResponse, NativeError> {
        request_on(&self.inner, req, parse_value_response)
    }

    /// The driver connection without the program, for the evaluator's `CAdapter` (whose `CapabilityImpl` must not
    /// borrow the program).
    pub(crate) fn raw(&self) -> RawSession {
        RawSession {
            inner: Rc::clone(&self.inner),
        }
    }
}

/// A driver connection that borrows no program.
#[derive(Clone)]
pub(crate) struct RawSession {
    inner: Rc<RefCell<Inner>>,
}

impl RawSession {
    pub(crate) fn request_value(&self, req: &Request) -> Result<ValueResponse, NativeError> {
        request_on(&self.inner, req, parse_value_response)
    }

    /// How the driver ended, when it has died (its exit status or signal).
    pub(crate) fn ended(&self) -> Option<cpl_eval::boundary::HostEnd> {
        self.inner.borrow().end
    }
}

fn request_on<T>(
    cell: &RefCell<Inner>,
    req: &Request,
    parse: fn(&str) -> Result<T, serde_json::Error>,
) -> Result<T, NativeError> {
    {
        let mut inner = cell.borrow_mut();
        if inner.dead {
            return Err(NativeError::DriverFailed(internal(
                "native_driver_failed",
                "the driver has already exited".to_string(),
            )));
        }
        let line = serde_json::to_string(req).expect("a request serializes");
        let written = writeln!(inner.stdin, "{line}").and_then(|_| inner.stdin.flush());
        let mut answer = String::new();
        // the watchdog: kill a driver that does not answer in time, so that the read below ends
        let watchdog = inner.timeout.map(|limit| {
            let pid = inner.child.id();
            let (done, wait) = std::sync::mpsc::channel::<()>();
            std::thread::spawn(move || {
                if wait.recv_timeout(limit) == Err(std::sync::mpsc::RecvTimeoutError::Timeout) {
                    let _ = std::process::Command::new("kill")
                        .arg("-9")
                        .arg(pid.to_string())
                        .status();
                }
            });
            done
        });
        let read = match written {
            Ok(()) => inner.stdout.read_line(&mut answer),
            Err(e) => Err(e),
        };
        if let Some(done) = watchdog {
            let _ = done.send(());
        }
        match read {
            Ok(n) if n > 0 => parse(answer.trim_end()).map_err(|e| {
                NativeError::Protocol(internal(
                    "native_protocol",
                    format!("the driver's answer is malformed: {e}"),
                ))
            }),
            _ => {
                inner.dead = true;
                let status = inner.child.wait();
                let mut d = internal("native_driver_failed", "the driver died".to_string());
                if let Ok(status) = status {
                    use std::os::unix::process::ExitStatusExt;
                    inner.end = match (status.signal(), status.code()) {
                        (Some(sig), _) => Some(cpl_eval::boundary::HostEnd::Signal(sig)),
                        (None, Some(code)) => Some(cpl_eval::boundary::HostEnd::Status(code)),
                        _ => None,
                    };
                    if let Some(sig) = status.signal() {
                        d = internal(
                            "native_driver_failed",
                            format!("the driver died of signal {sig}"),
                        )
                        .with_data("signal", json!(sig));
                    } else if let Some(code) = status.code() {
                        d = internal(
                            "native_driver_failed",
                            format!("the driver exited with status {code}"),
                        )
                        .with_data("exit_status", json!(code));
                    }
                }
                Err(NativeError::DriverFailed(d))
            }
        }
    }
}

impl<'p> NativeSession<'p> {
    /// A watchdog for every request from now on: a driver that does not answer within `limit` is killed, and the
    /// request fails as for a driver that died (`native_driver_failed` with `signal` 9). The differential suites set
    /// 60 seconds (toolchain 12.6); `None`, the default, waits indefinitely.
    pub fn set_request_timeout(&mut self, limit: Option<std::time::Duration>) {
        self.inner.borrow_mut().timeout = limit;
    }

    /// `configure` again (`spec/cabi.md` 3.8): a fresh runtime in the driver, handles numbered from 0, `cfg`'s limits.
    pub fn reconfigure(&mut self, cfg: &EvalConfig) -> Result<(), NativeError> {
        let req = Request::Configure {
            max_call_depth: cfg.max_call_depth,
            time_budget_ms: cfg
                .time_budget
                .map(|d| u64::try_from(d.as_millis()).unwrap_or(u64::MAX)),
            check_contracts: cfg.check_contracts,
            max_heap_bytes: 0,
        };
        match self.request(&req)? {
            Response::Ok { .. } => Ok(()),
            Response::Error { error } => Err(NativeError::Refused(diagnostic_of(&error))),
            _ => Err(NativeError::Protocol(internal(
                "native_protocol",
                "configure got an unexpected answer".into(),
            ))),
        }
    }

    /// The compiled-in backend-`c` adapter of `binding` (the `bind` request); a grant that fails is
    /// [`NativeError::Refused`].
    pub fn bind(&mut self, binding: &str) -> Result<HandleId, NativeError> {
        match self.request(&Request::Bind {
            binding: binding.to_string(),
        })? {
            Response::Handle { handle } => Ok(HandleId(handle)),
            Response::Error { error } => Err(NativeError::Refused(diagnostic_of(&error))),
            _ => Err(NativeError::Protocol(internal(
                "native_protocol",
                "bind got an unexpected answer".into(),
            ))),
        }
    }

    /// The interpreter and environment of the driver's later Python workers (the `workers` request, `spec/python.md`
    /// 6.5).
    pub fn configure_workers(
        &mut self,
        config: &crate::worker::WorkerConfig,
    ) -> Result<(), NativeError> {
        let req = Request::Workers {
            python: config.python.display().to_string(),
            env: config.env.iter().cloned().collect(),
        };
        match self.request(&req)? {
            Response::Ok { .. } => Ok(()),
            Response::Error { error } => Err(NativeError::Refused(diagnostic_of(&error))),
            _ => Err(NativeError::Protocol(internal(
                "native_protocol",
                "workers got an unexpected answer".into(),
            ))),
        }
    }

    /// A scripted host of `capability` under the binding name `label` (the `bind_scripted` request).
    pub fn bind_scripted(
        &mut self,
        capability: DefId,
        label: &str,
        script: protocol::Script,
    ) -> Result<HandleId, NativeError> {
        let req = Request::BindScripted {
            capability: capability.0,
            label: label.to_string(),
            script,
        };
        match self.request(&req)? {
            Response::Handle { handle } => Ok(HandleId(handle)),
            Response::Error { error } => Err(NativeError::Refused(diagnostic_of(&error))),
            _ => Err(NativeError::Protocol(internal(
                "native_protocol",
                "bind_scripted got an unexpected answer".into(),
            ))),
        }
    }

    fn value_of(&self, nodes: wire::Nodes) -> Result<Value, NativeError> {
        let v = wire::from_nodes(self.p, &nodes)
            .map_err(|e| NativeError::Protocol(internal("native_protocol", format!("{e}"))))?;
        self.inner.borrow_mut().renderings = Some(Renderings {
            json: nodes.json,
            text: nodes.text,
            diagnostic: None,
        });
        Ok(v)
    }

    fn fault_of(&self, answer: &FaultAnswer) -> Fault {
        let f = crate::fault_from_record(self.p, &answer.fault);
        let mut inner = self.inner.borrow_mut();
        inner.renderings = Some(Renderings {
            json: None,
            text: None,
            diagnostic: Some(answer.fault.diagnostic.clone()),
        });
        inner.violation = match &answer.violation {
            Some(Some(v)) => Some((DefId(v.def), crate::fault_from_record(self.p, &v.fault))),
            _ => None,
        };
        f
    }

    /// One top-level evaluation of `f` (the `call` request), with the refined positions of the arguments checked first
    /// when `check_arguments` is set.
    pub fn call(
        &mut self,
        f: DefId,
        args: &[Value],
        using: &[HandleId],
        check_arguments: bool,
    ) -> Result<Result<Value, Fault>, NativeError> {
        let req = Request::Call {
            function: f.0,
            args: args.iter().map(wire::to_nodes).collect(),
            using: using.iter().map(|h| h.0).collect(),
            check_arguments,
        };
        match self.request_value(&req)? {
            ValueResponse::Value(nodes) => Ok(Ok(self.value_of(nodes)?)),
            ValueResponse::Other(Response::Fault(answer)) => Ok(Err(self.fault_of(&answer))),
            ValueResponse::Other(Response::Error { error }) => {
                Err(NativeError::Refused(diagnostic_of(&error)))
            }
            _ => Err(NativeError::Protocol(internal(
                "native_protocol",
                "call got an unexpected answer".into(),
            ))),
        }
    }

    /// Entry `index` of a compiled table, or without `index` its length (the `tables` request, `spec/cabi.md` 3.8):
    /// what the table-fidelity test reads.
    pub fn tables(
        &mut self,
        table: protocol::Table,
        index: Option<u32>,
    ) -> Result<serde_json::Value, NativeError> {
        match self.request(&Request::Tables { table, index })? {
            Response::Ok { ok } => Ok(ok),
            Response::Error { error } => Err(NativeError::Refused(diagnostic_of(&error))),
            _ => Err(NativeError::Protocol(internal(
                "native_protocol",
                "tables got an unexpected answer".into(),
            ))),
        }
    }

    /// The value with the C writer's renderings (the `render` request, `spec/cabi.md` 3.8): the value as the driver
    /// decoded it, and its value JSON and value text.
    pub fn render(&mut self, v: &Value) -> Result<(Value, Renderings), NativeError> {
        match self.request_value(&Request::Render {
            value: wire::to_nodes(v),
        })? {
            ValueResponse::Value(nodes) => {
                let value = self.value_of(nodes)?;
                Ok((value, self.native_renderings().unwrap_or_default()))
            }
            ValueResponse::Other(Response::Error { error }) => {
                Err(NativeError::Refused(diagnostic_of(&error)))
            }
            _ => Err(NativeError::Protocol(internal(
                "native_protocol",
                "render got an unexpected answer".into(),
            ))),
        }
    }

    /// The C reader of value JSON (the `from_json` request): the value at the type named by its qualified text, with
    /// the C writer's renderings, or the refusal's diagnostic (`interop_type_unknown`, `interop_json_invalid`).
    pub fn from_json(
        &mut self,
        ty: &str,
        json: &str,
    ) -> Result<Result<(Value, Renderings), Diagnostic>, NativeError> {
        match self.request_value(&Request::FromJson {
            ty: ty.to_string(),
            json: json.to_string(),
        })? {
            ValueResponse::Value(nodes) => {
                let value = self.value_of(nodes)?;
                Ok(Ok((value, self.native_renderings().unwrap_or_default())))
            }
            ValueResponse::Other(Response::Error { error }) => Ok(Err(diagnostic_of(&error))),
            _ => Err(NativeError::Protocol(internal(
                "native_protocol",
                "from_json got an unexpected answer".into(),
            ))),
        }
    }

    /// The C side's renderings of the last outcome.
    pub fn native_renderings(&self) -> Option<Renderings> {
        self.inner.borrow().renderings.clone()
    }

    /// The counters of a coverage driver (toolchain 12.6).
    pub fn coverage(&mut self) -> Result<Coverage, NativeError> {
        match self.request(&Request::Coverage)? {
            Response::Ok { ok } => {
                let c: protocol::CoverageCounts = serde_json::from_value(ok).map_err(|e| {
                    NativeError::Protocol(internal(
                        "native_protocol",
                        format!("coverage is malformed: {e}"),
                    ))
                })?;
                Ok(Coverage {
                    nodes: c.nodes,
                    builtins: c.builtins,
                    claims: c.claims,
                })
            }
            Response::Error { error } => Err(NativeError::Refused(diagnostic_of(&error))),
            _ => Err(NativeError::Protocol(internal(
                "native_protocol",
                "coverage got an unexpected answer".into(),
            ))),
        }
    }

    /// The [`NativeError`] that a `TestEngine` or `RunEngine` request of this session or of a clone met, if any.
    pub fn take_error(&mut self) -> Option<NativeError> {
        self.inner.borrow_mut().error.take()
    }

    /// Live handles, capabilities, objects, and buffers of the driver's runtime after the driver dropped every handle
    /// it numbered (the `shutdown` answer), and its exit status.
    pub fn shutdown(self) -> Result<ShutdownReport, NativeError> {
        let stats = match self.request(&Request::Shutdown)? {
            Response::Ok { ok } => ok,
            _ => serde_json::Value::Null,
        };
        let get = |k: &str| stats.get(k).and_then(|v| v.as_u64()).unwrap_or(0);
        let report = (
            get("live_handles"),
            get("live_caps"),
            get("live_objects"),
            get("live_buffers"),
        );
        let status = self.inner.borrow_mut().child.wait().map_err(|e| {
            NativeError::DriverFailed(internal("native_driver_failed", format!("{e}")))
        })?;
        Ok(ShutdownReport {
            live_handles: report.0,
            live_caps: report.1,
            live_objects: report.2,
            live_buffers: report.3,
            exit_status: status,
        })
    }

    /// Keeps a NativeError for take_error and answers with the fault of its kind.
    fn engine_fault(&self, e: NativeError) -> Fault {
        let (kind, message) = match &e {
            NativeError::DriverFailed(d) => ("native_driver_failed", d.message.clone()),
            NativeError::Protocol(d) => ("native_protocol", d.message.clone()),
            NativeError::Refused(d) => ("native_protocol", d.message.clone()),
            NativeError::Build(ds) => (
                "native_driver_failed",
                ds.first()
                    .map(|d| d.message.clone())
                    .unwrap_or_else(|| "the driver could not be built".into()),
            ),
        };
        self.inner.borrow_mut().error = Some(e);
        Fault::internal(kind, message)
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        if !self.dead {
            let _ = writeln!(self.stdin, "{{\"op\":\"shutdown\"}}");
            let _ = self.stdin.flush();
            let _ = self.child.wait();
        }
    }
}

/// The C side's renderings of an outcome: C value JSON, value text, S49 JSON.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Renderings {
    pub json: Option<String>,
    pub text: Option<String>,
    pub diagnostic: Option<String>,
}

/// The counters of a coverage driver (the `coverage` request, `spec/cabi.md` 3.8): executed HIR nodes by variant
/// (`"ExprKind::Call"`, `"Stmt::While"`, `"PatternKind::Variant"`), builtin calls by registry id, and claims by the
/// kind of their claim site, as the coverage gate of toolchain 12.6 counts them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Coverage {
    pub nodes: BTreeMap<String, u64>,
    pub builtins: BTreeMap<String, u64>,
    pub claims: BTreeMap<String, u64>,
}

/// What [`NativeSession::shutdown`] observed: the driver runtime's live handles (value handles and cancellation
/// tokens), capabilities, heap objects, and buffers before it closed, each 0 for a clean shutdown (`spec/cabi.md`
/// 3.9), and how the driver process ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShutdownReport {
    pub live_handles: u64,
    pub live_caps: u64,
    pub live_objects: u64,
    pub live_buffers: u64,
    pub exit_status: std::process::ExitStatus,
}

/// A failure of the native engine or of a grant, never of the program: the driver could not be built (its
/// diagnostics, which the commands report as `cpl build` reports them), it died (`internal_error` kind
/// `native_driver_failed`, with `signal` or `exit_status`), it sent a malformed message (kind `native_protocol`), or
/// it refused a request (its `{"error": E}` answer, `spec/cabi.md` 3.8, as a Diagnostic: `interop_bind_failed` for a
/// grant that failed, exit 1 under semantics S78; any other refusal, `internal_error` kind `runtime_invariant`
/// included, is a toolchain defect, exit 2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NativeError {
    Build(Vec<Diagnostic>),
    DriverFailed(Diagnostic),
    Protocol(Diagnostic),
    Refused(Diagnostic),
}

impl NativeError {
    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        match self {
            NativeError::Build(ds) => ds,
            NativeError::DriverFailed(d) | NativeError::Protocol(d) | NativeError::Refused(d) => {
                vec![d]
            }
        }
    }
}

/// `run_body`, `failing_refinement`, and `range_bound` requests. A request that meets a [`NativeError`] keeps it for
/// [`NativeSession::take_error`] and answers with the fault `internal_error` of its kind (`native_driver_failed` or
/// `native_protocol`); `cpl test` calls `take_error` after every test and then ends the command with that error
/// (exit 2). The factory that `cpl test --engine native` passes to `run_tests_with` returns, for each test, a clone of
/// the run's one session after `reconfigure`, whose failure is kept for `take_error` in the same way.
impl<'p> TestEngine<'p> for NativeSession<'p> {
    fn program(&self) -> &'p Program {
        self.p
    }

    fn run_body(&mut self, test: DefId, binders: &[(LocalId, Value)]) -> Result<(), Fault> {
        self.inner.borrow_mut().violation = None;
        let req = Request::RunBody {
            test: test.0,
            binders: binders.iter().map(|(_, v)| wire::to_nodes(v)).collect(),
        };
        match self.request(&req) {
            Ok(Response::Ok { .. }) => Ok(()),
            Ok(Response::Fault(answer)) => Err(self.fault_of(&answer)),
            Ok(Response::Error { error }) => {
                Err(self.engine_fault(NativeError::Refused(diagnostic_of(&error))))
            }
            Ok(_) => Err(self.engine_fault(NativeError::Protocol(internal(
                "native_protocol",
                "run_body got an unexpected answer".into(),
            )))),
            Err(e) => Err(self.engine_fault(e)),
        }
    }

    fn take_violation(&mut self) -> Option<(DefId, Fault)> {
        self.inner.borrow_mut().violation.take()
    }

    fn failing_refinement(
        &mut self,
        refined: DefId,
        value: &Value,
        budget: Budget,
    ) -> Result<Option<DefId>, Fault> {
        let req = Request::FailingRefinement {
            refined: refined.0,
            value: wire::to_nodes(value),
            budget: match budget {
                Budget::Fresh => protocol::Budget::Fresh,
                Budget::None => protocol::Budget::None,
            },
        };
        match self.request(&req) {
            Ok(Response::Ok { ok }) => Ok(ok.as_u64().map(|d| DefId(d as u32))),
            Ok(Response::Fault(answer)) => Err(self.fault_of(&answer)),
            Ok(Response::Error { error }) => {
                Err(self.engine_fault(NativeError::Refused(diagnostic_of(&error))))
            }
            Ok(_) => Err(self.engine_fault(NativeError::Protocol(internal(
                "native_protocol",
                "failing_refinement got an unexpected answer".into(),
            )))),
            Err(e) => Err(self.engine_fault(e)),
        }
    }

    fn eval_range_bound(
        &mut self,
        property: DefId,
        binder: usize,
        side: RangeSide,
        clause_text: &str,
    ) -> Result<Value, Fault> {
        self.inner.borrow_mut().violation = None;
        let req = Request::RangeBound {
            property: property.0,
            binder: binder as u32,
            side: match side {
                RangeSide::Lo => protocol::Side::Lo,
                RangeSide::Hi => protocol::Side::Hi,
            },
            clause: clause_text.to_string(),
        };
        match self.request_value(&req) {
            Ok(ValueResponse::Value(nodes)) => {
                self.value_of(nodes).map_err(|e| self.engine_fault(e))
            }
            Ok(ValueResponse::Other(Response::Fault(answer))) => Err(self.fault_of(&answer)),
            Ok(ValueResponse::Other(Response::Error { error })) => {
                Err(self.engine_fault(NativeError::Refused(diagnostic_of(&error))))
            }
            Ok(_) => Err(self.engine_fault(NativeError::Protocol(internal(
                "native_protocol",
                "range_bound got an unexpected answer".into(),
            )))),
            Err(e) => Err(self.engine_fault(e)),
        }
    }
}

/// `bind` takes `HostSource::Binding` alone and sends the driver's `bind` request for it (`HostSource::Impl` is
/// refused, `internal_error` kind `call_mismatch`); `call_checked` is `call` with `check_arguments` true. `cpl run`
/// passes, for each `using` parameter, the plan's binding of its capability (`InteropPlan::binding_for`).
impl<'p> RunEngine for NativeSession<'p> {
    fn bind(&mut self, param: &ParamDef, host: HostSource) -> Result<Handle, Vec<Diagnostic>> {
        let capability = match &param.ty {
            cpl_hir::Ty::Capability(c) => *c,
            _ => DefId(u32::MAX),
        };
        match host {
            HostSource::Binding(name) => match NativeSession::bind(self, &name) {
                Ok(id) => Ok(Handle {
                    id,
                    capability,
                    label: name,
                }),
                Err(e) => Err(e.into_diagnostics()),
            },
            HostSource::Impl(..) => Err(vec![internal(
                "call_mismatch",
                format!(
                    "{} is bound by a host implementation, which the native engine cannot run",
                    param.name
                ),
            )]),
        }
    }

    fn call_checked(
        &mut self,
        f: DefId,
        args: Vec<Value>,
        using: Vec<Handle>,
    ) -> Result<Value, Fault> {
        let ids: Vec<HandleId> = using.iter().map(|h| h.id).collect();
        match self.call(f, &args, &ids, true) {
            Ok(outcome) => outcome,
            Err(e) => Err(self.engine_fault(e)),
        }
    }
}
