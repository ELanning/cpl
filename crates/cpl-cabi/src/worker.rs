//! The evaluator's worker client (`spec/python.md` Sections 6 and 7): [`Worker`], one worker process that hosts one
//! grant and speaks cpl-worker protocol 1 over a socket that replaces its standard input and output, and
//! [`WorkerAdapter`], the `CapabilityImpl` through which `cpl run` and `cpl test` reach a backend-`python` binding's
//! adapter. The native runtime has its own client (`cpl_rt_worker.c`); both start the same worker source.

use crate::wire::{self, Nodes};
use cpl_diag::{Category, DiagCode, Diagnostic};
use cpl_eval::boundary::{self, HostEnd};
use cpl_eval::{CapabilityImpl, Fault, HostContext, Value};
use cpl_hir::{DefId, Program};
use serde::Deserialize;
use serde_json::{json, Value as Json};
use std::io::{self, BufRead, BufReader, Write};
use std::net::Shutdown;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::os::unix::process::ExitStatusExt;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long a worker may take to send its banner when its binding sets no `timeout_ms` (`spec/python.md` 6.4).
pub const BANNER_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a released worker may take to exit before it is killed (`spec/python.md` 6.3).
pub const EXIT_GRACE: Duration = Duration::from_secs(5);

/// One budget shared by every read and write of an exchange. `None` really means unlimited; it must not be
/// represented by a huge duration that can overflow an Instant or the operating system's socket timeout.
struct Deadline {
    started: Instant,
    limit: Option<Duration>,
}

impl Deadline {
    fn new(limit: Option<Duration>) -> Self {
        Self {
            started: Instant::now(),
            limit,
        }
    }

    fn remaining(&self) -> io::Result<Option<Duration>> {
        match self
            .limit
            .map(|limit| limit.saturating_sub(self.started.elapsed()))
        {
            Some(left) if left.is_zero() => Err(io::ErrorKind::TimedOut.into()),
            left => Ok(left),
        }
    }

    fn expired(&self) -> WorkerDeath {
        WorkerDeath::Timeout(
            self.limit
                .unwrap_or_default()
                .as_millis()
                .min(u64::MAX as u128) as u64,
        )
    }
}

/// The interpreter and environment of the Python workers a host starts (`spec/python.md` 6.5).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WorkerConfig {
    pub python: PathBuf,
    /// Variables set in the worker's environment, over the host's.
    pub env: Vec<(String, String)>,
}

impl WorkerConfig {
    /// The toolchain's configuration for a project: `python` (`--python`), else `CPL_PYTHON`, else `python3`; and
    /// `PYTHONPATH` = the absolute `[interop.python] paths`, then the inherited `PYTHONPATH`.
    pub fn for_project(
        project: &cpl_project::Project,
        python: Option<&std::path::Path>,
    ) -> WorkerConfig {
        let python = python
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("CPL_PYTHON").map(PathBuf::from))
            .unwrap_or_else(|| PathBuf::from("python3"));
        let mut parts: Vec<String> = Vec::new();
        if let Some(root) = &project.root {
            for p in project.manifest.python_paths() {
                let dir = if p == "." {
                    root.clone()
                } else {
                    root.join(&p)
                };
                parts.push(dir.display().to_string());
            }
        }
        if let Some(existing) = std::env::var_os("PYTHONPATH") {
            let existing = existing.to_string_lossy().to_string();
            if !existing.is_empty() {
                parts.push(existing);
            }
        }
        WorkerConfig {
            python,
            env: vec![("PYTHONPATH".to_string(), parts.join(":"))],
        }
    }
}

/// The wire form of a foreign failure `F` (`spec/cabi.md` 3.8).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
pub struct WireFailure {
    pub kind: String,
    pub message: String,
    #[serde(default)]
    pub details: Vec<(String, String)>,
}

/// The answer of a `cap_call`.
#[derive(Clone, Debug, PartialEq)]
pub enum WorkerAnswer {
    Ok(Nodes),
    Foreign(WireFailure),
}

/// Why a worker can no longer answer (`spec/python.md` 6.4).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WorkerDeath {
    /// It ended, or its channel closed.
    Ended(HostEnd),
    /// It did not answer within its timeout, in milliseconds.
    Timeout(u64),
    /// It sent an answer that is not cpl-worker protocol 1: the S82 `<reason>`.
    Protocol(&'static str),
}

/// One worker process hosting one grant.
pub struct Worker {
    child: Child,
    stream: UnixStream,
    reader: BufReader<UnixStream>,
    binding: String,
    timeout: Option<Duration>,
    /// The banner's `runtime` (`{"python": "3.10.12"}`).
    pub runtime: Json,
    death: Option<WorkerDeath>,
}

fn bind_failed(binding: &str, note: String) -> Diagnostic {
    Diagnostic::error(
        DiagCode("interop_bind_failed"),
        Category::Interop,
        format!("the binding `{binding}` could not be granted"),
    )
    .with_data("binding", json!(binding))
    .with_note(note, None)
}

impl Worker {
    /// Starts a Python worker for `binding` with `source` (`spec/python.md` 6.1): spawns `<python> -c BOOTSTRAP`,
    /// sends the source, checks the banner (protocol 1, kind `python`, `program` = `digest`), and binds. Every
    /// failure is the grant's `interop_bind_failed`.
    pub fn start_python(
        source: &str,
        digest: &str,
        binding: &str,
        config: &WorkerConfig,
        timeout: Option<Duration>,
    ) -> Result<Worker, Diagnostic> {
        let (parent, child_end) = UnixStream::pair().map_err(|e| {
            bind_failed(binding, format!("cannot create the worker's channel: {e}"))
        })?;
        let child_in = child_end.try_clone().map_err(|e| {
            bind_failed(binding, format!("cannot create the worker's channel: {e}"))
        })?;
        let mut cmd = Command::new(&config.python);
        cmd.arg("-c")
            .arg(cpl_emit_c::python::BOOTSTRAP)
            .stdin(Stdio::from(OwnedFd::from(child_in)))
            .stdout(Stdio::from(OwnedFd::from(child_end)))
            .stderr(Stdio::inherit());
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        // Clone before spawning: if this fails, there is no child to leak.
        let reader =
            BufReader::new(parent.try_clone().map_err(|e| {
                bind_failed(binding, format!("cannot read the worker's channel: {e}"))
            })?);
        let child = cmd.spawn().map_err(|e| {
            bind_failed(
                binding,
                format!(
                    "cannot start the Python worker `{}`: {e}",
                    config.python.display()
                ),
            )
        })?;
        // Command retains its configured Stdio handles for another spawn. Keeping those copies of the child's
        // socket open here would hide EOF if the worker exited before sending its banner or bind response.
        drop(cmd);
        let mut worker = Worker {
            child,
            stream: parent,
            reader,
            binding: binding.to_string(),
            timeout,
            runtime: Json::Null,
            death: None,
        };
        let mut header = format!("{}\n", source.len()).into_bytes();
        header.extend_from_slice(source.as_bytes());
        let deadline = Deadline::new(Some(timeout.unwrap_or(BANNER_TIMEOUT)));
        let banner = match worker
            .write(&header, &deadline)
            .and_then(|()| worker.read_line(&deadline))
        {
            Ok(line) => line,
            Err(death) => {
                let note = match death {
                    WorkerDeath::Ended(end) => {
                        format!("the worker exited with {} before its banner", end.render())
                    }
                    WorkerDeath::Timeout(ms) => format!("the worker sent no banner within {ms} ms"),
                    WorkerDeath::Protocol(reason) => format!("the worker's banner is {reason}"),
                };
                worker.kill();
                return Err(bind_failed(binding, note));
            }
        };
        let ok = banner.get("protocol") == Some(&json!(1))
            && banner.get("worker") == Some(&json!("python"))
            && banner.get("program") == Some(&json!(digest));
        if !ok {
            worker.kill();
            return Err(bind_failed(
                binding,
                format!(
                    "the worker's banner is not cpl-worker protocol 1 for this program: {banner}"
                ),
            ));
        }
        worker.runtime = banner.get("runtime").cloned().unwrap_or(Json::Null);
        match worker.request(
            &json!({"op": "bind", "binding": binding}),
            &Deadline::new(Some(timeout.unwrap_or(BANNER_TIMEOUT))),
        ) {
            Ok(answer) if answer.get("handle") == Some(&json!(0)) => Ok(worker),
            Ok(answer) => {
                worker.kill();
                let mut d = bind_failed(binding, "the adapter's create() failed".to_string());
                if let Some(error) = answer.get("error") {
                    if let Some(notes) = error.get("notes").and_then(Json::as_array) {
                        d.notes.clear();
                        for n in notes {
                            if let Some(m) = n.get("message").and_then(Json::as_str) {
                                d = d.with_note(m.to_string(), None);
                            }
                        }
                    }
                }
                Err(d)
            }
            Err(death) => {
                let note = match death {
                    WorkerDeath::Ended(end) => {
                        format!("the worker exited with {} while binding", end.render())
                    }
                    WorkerDeath::Timeout(ms) => format!("the worker did not bind within {ms} ms"),
                    WorkerDeath::Protocol(reason) => {
                        format!("the worker's answer to bind is {reason}")
                    }
                };
                worker.kill();
                Err(bind_failed(binding, note))
            }
        }
    }

    /// A channel can close while the child is still running. Give an exiting child time to publish its status,
    /// but never wait indefinitely for a process that can no longer answer.
    fn reap(&mut self, grace: Duration) -> HostEnd {
        let started = Instant::now();
        let status = loop {
            match self.child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if started.elapsed() < grace => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                _ => {
                    let _ = self.child.kill();
                    break self.child.wait();
                }
            }
        };
        match status {
            Ok(status) => match (status.code(), status.signal()) {
                (Some(code), _) => HostEnd::Status(code),
                (None, Some(signal)) => HostEnd::Signal(signal),
                _ => HostEnd::Status(-1),
            },
            Err(_) => HostEnd::Status(-1),
        }
    }

    fn kill(&mut self) {
        let _ = self.stream.shutdown(Shutdown::Both);
        let _ = self.child.kill();
        let _ = self.child.wait();
    }

    fn channel_ended(&mut self) -> WorkerDeath {
        let _ = self.stream.shutdown(Shutdown::Both);
        WorkerDeath::Ended(self.reap(Duration::from_secs(1)))
    }

    fn write(&mut self, mut bytes: &[u8], deadline: &Deadline) -> Result<(), WorkerDeath> {
        while !bytes.is_empty() {
            let left = deadline.remaining().map_err(|_| deadline.expired())?;
            if self.stream.set_write_timeout(left).is_err() {
                return Err(self.channel_ended());
            }
            match self.stream.write(bytes) {
                Ok(0) => return Err(self.channel_ended()),
                Ok(n) => bytes = &bytes[n..],
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue
                }
                Err(_) => return Err(self.channel_ended()),
            }
        }
        Ok(())
    }

    /// Read chunks instead of `BufRead::read_line`: its internal reads could each renew a socket timeout while a
    /// worker trickles bytes forever. JSON is decoded only after the complete UTF-8 line arrives.
    fn read_line(&mut self, deadline: &Deadline) -> Result<Json, WorkerDeath> {
        let mut line = Vec::new();
        loop {
            let left = deadline.remaining().map_err(|_| deadline.expired())?;
            if self.reader.get_ref().set_read_timeout(left).is_err() {
                return Err(self.channel_ended());
            }
            match self.reader.fill_buf() {
                Ok([]) => return Err(self.channel_ended()),
                Ok(bytes) => {
                    let end = bytes.iter().position(|b| *b == b'\n');
                    let n = end.map_or(bytes.len(), |n| n + 1);
                    if line.len().saturating_add(n) > 1 << 30 {
                        return Err(WorkerDeath::Protocol("not a response to cap_call"));
                    }
                    line.extend_from_slice(&bytes[..n]);
                    self.reader.consume(n);
                    if end.is_some() {
                        break;
                    }
                }
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock
                            | io::ErrorKind::TimedOut
                            | io::ErrorKind::Interrupted
                    ) =>
                {
                    continue
                }
                Err(_) => return Err(self.channel_ended()),
            }
        }
        serde_json::from_slice(&line).map_err(|_| WorkerDeath::Protocol("not JSON"))
    }

    fn request(&mut self, req: &Json, deadline: &Deadline) -> Result<Json, WorkerDeath> {
        let mut text = serde_json::to_vec(req).expect("a request serializes");
        text.push(b'\n');
        self.write(&text, deadline)?;
        self.read_line(deadline)
    }

    /// One operation call (`spec/python.md` 6.2): the answer, or why the worker can no longer answer. A worker that
    /// failed once answers every later call with the same death, at once (6.4).
    pub fn call(&mut self, operation: u32, args: &[Nodes]) -> Result<WorkerAnswer, WorkerDeath> {
        if let Some(death) = &self.death {
            return Err(death.clone());
        }
        let req = json!({"op": "cap_call", "handle": 0, "operation": operation, "args": args});
        let answer = match self.request(&req, &Deadline::new(self.timeout)) {
            Ok(answer) => answer,
            Err(death) => {
                if !matches!(death, WorkerDeath::Ended(_)) {
                    self.kill();
                }
                self.death = Some(death.clone());
                return Err(death);
            }
        };
        let response_keys = ["ok", "foreign_error", "error"]
            .iter()
            .filter(|key| answer.get(**key).is_some())
            .count();
        let parsed = if response_keys != 1 {
            Err(WorkerDeath::Protocol("not a response to cap_call"))
        } else if let Some(ok) = answer.get("ok") {
            serde_json::from_value::<Nodes>(ok.clone())
                .ok()
                .filter(|n| !n.nodes.is_empty())
                .map(WorkerAnswer::Ok)
                .ok_or(WorkerDeath::Protocol("a value that is not a node list"))
        } else if let Some(f) = answer.get("foreign_error") {
            serde_json::from_value::<WireFailure>(f.clone())
                .ok()
                .filter(|f| cpl_rtspec::foreign::KINDS.contains(&f.kind.as_str()))
                .map(WorkerAnswer::Foreign)
                .ok_or(WorkerDeath::Protocol("not a response to cap_call"))
        } else {
            Err(WorkerDeath::Protocol("not a response to cap_call"))
        };
        if let Err(death) = &parsed {
            self.kill();
            self.death = Some(death.clone());
        }
        parsed
    }

    /// Releases the grant and ends the worker (`spec/python.md` 6.3): `release`, `shutdown`, up to five seconds for
    /// the exit, then `SIGKILL`; the process is always reaped.
    pub fn shutdown(&mut self) {
        let deadline = Deadline::new(Some(EXIT_GRACE));
        if self.death.is_none() && matches!(self.child.try_wait(), Ok(None)) {
            for req in [
                json!({"op": "release", "handle": 0}),
                json!({"op": "shutdown"}),
            ] {
                if !matches!(self.request(&req, &deadline), Ok(answer) if answer.get("ok") == Some(&Json::Null))
                {
                    self.kill();
                    return;
                }
            }
        }
        let _ = self.stream.shutdown(Shutdown::Both);
        self.reap(deadline.remaining().ok().flatten().unwrap_or_default());
    }

    /// The binding this worker hosts.
    pub fn binding(&self) -> &str {
        &self.binding
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// A backend-python binding's adapter in a worker, as the evaluator's `CapabilityImpl` (`spec/python.md` 7): the
/// binding layer only; validation and `ensures` stay the evaluator's, which binds every WorkerAdapter with
/// `validate_results: true`.
pub struct WorkerAdapter {
    worker: Worker,
    capability: DefId,
    /// Subsequent calls return Transport with the first failure's message, without restarting the grant.
    death_message: Option<String>,
}

impl WorkerAdapter {
    pub fn new(worker: Worker, capability: DefId) -> WorkerAdapter {
        WorkerAdapter {
            worker,
            capability,
            death_message: None,
        }
    }

    /// The banner's `runtime` of the worker.
    pub fn runtime(&self) -> &Json {
        &self.worker.runtime
    }
}

fn death_message(
    p: &Program,
    capability: DefId,
    op: u32,
    binding: &str,
    death: &WorkerDeath,
) -> String {
    use cpl_rtspec::messages;
    match death {
        WorkerDeath::Ended(end) => messages::render(
            &messages::ADAPTER_HOST_EXITED,
            &[("binding", binding), ("status", &end.render())],
        ),
        WorkerDeath::Timeout(ms) => messages::render(
            &messages::ADAPTER_HOST_TIMEOUT,
            &[("binding", binding), ("ms", &ms.to_string())],
        ),
        WorkerDeath::Protocol(reason) => messages::render(
            &messages::DETAIL_BINDING_PROTOCOL_WORKER,
            &[
                ("binding", binding),
                (
                    "operation",
                    &boundary::operation_identity_of(p, capability, op),
                ),
                ("reason", reason),
            ],
        ),
    }
}

/// The outcome of a dead worker for the operation `op` (`spec/python.md` 6.4; stdlib S48; semantics S82).
pub fn death_outcome(
    p: &Program,
    capability: DefId,
    op: u32,
    binding: &str,
    death: &WorkerDeath,
) -> Result<Value, Fault> {
    let message = death_message(p, capability, op, binding, death);
    match death {
        WorkerDeath::Ended(_) => boundary::host_failed(p, capability, op, binding, &message),
        WorkerDeath::Timeout(_) => {
            boundary::foreign_failure(p, capability, op, binding, "Timeout", &message, &[])
        }
        WorkerDeath::Protocol(_) => {
            let operation = boundary::operation_identity_of(p, capability, op);
            Err(Fault::internal("binding_protocol", message)
                .with_data("operation", json!(operation))
                .with_data("binding", json!(binding)))
        }
    }
}

impl CapabilityImpl for WorkerAdapter {
    fn call(
        &mut self,
        op: &str,
        args: Vec<Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<Value, Fault> {
        let p = ctx.program;
        let index = p
            .def(self.capability)
            .kind
            .as_capability()
            .and_then(|c| c.ops.iter().position(|o| o.name == op))
            .unwrap_or(usize::MAX) as u32;
        let binding = self.worker.binding().to_string();
        if let Some(message) = &self.death_message {
            return boundary::host_failed(p, self.capability, index, &binding, message);
        }
        let nodes: Vec<Nodes> = args.iter().map(wire::to_nodes).collect();
        match self.worker.call(index, &nodes) {
            Ok(WorkerAnswer::Ok(nodes)) => match wire::from_nodes(p, &nodes) {
                Ok(value) => Ok(value),
                Err(_) => {
                    let death = WorkerDeath::Protocol("a value that is not a node list");
                    self.worker.kill();
                    self.worker.death = Some(death.clone());
                    self.death_message =
                        Some(death_message(p, self.capability, index, &binding, &death));
                    death_outcome(p, self.capability, index, &binding, &death)
                }
            },
            Ok(WorkerAnswer::Foreign(f)) => boundary::foreign_failure(
                p,
                self.capability,
                index,
                &binding,
                &f.kind,
                &f.message,
                &f.details,
            ),
            Err(death) => {
                self.death_message =
                    Some(death_message(p, self.capability, index, &binding, &death));
                death_outcome(p, self.capability, index, &binding, &death)
            }
        }
    }
}
