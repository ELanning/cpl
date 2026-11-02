//! The evaluator and C adapters (`spec/toolchain.md` 1.14; `spec/cabi.md` Section 7): `CAdapter`, the
//! `CapabilityImpl` through which the reference evaluator reaches a backend-`c` adapter compiled into the program's
//! driver (the `cap_call` request: the binding layer only, no clauses and no validation, which the evaluator does
//! itself), and the hosts of `cpl run` (semantics S78). Owner: the imports and C hosts track (F1).

use crate::protocol::{Request, Response};
use crate::session::RawSession;
use crate::NativeSession;
use cpl_diag::Diagnostic;
use cpl_emit_c::package::DriverConfig;
use cpl_emit_c::InteropPlan;
use cpl_eval::{BindOptions, CapabilityImpl, EvalConfig, Fault, HostContext, RunHosts, Value};
use cpl_hir::{DefId, ParamDef, Program, Ty};
use cpl_project::Project;
use std::cell::RefCell;
use std::rc::Rc;

/// One granted backend-c binding: its handle in the driver child that the CAdapters of one `ManifestHosts` share.
pub struct CAdapter {
    session: RawSession,
    handle: u32,
    capability: DefId,
    binding: String,
}

fn engine_fault(e: crate::NativeError) -> Fault {
    let d = e.into_diagnostics().into_iter().next();
    let message = d
        .map(|d| d.message)
        .unwrap_or_else(|| "the driver failed".to_string());
    Fault::internal("native_driver_failed", message)
}

impl CapabilityImpl for CAdapter {
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
        let req = Request::CapCall {
            handle: self.handle,
            operation: index,
            args: args.iter().map(crate::wire::to_nodes).collect(),
        };
        match self.session.request(&req) {
            Ok(Response::Ok { ok }) => {
                let nodes: crate::wire::Nodes = serde_json::from_value(ok).map_err(|e| {
                    Fault::internal(
                        "native_protocol",
                        format!("a value of the driver is malformed: {e}"),
                    )
                })?;
                crate::wire::from_nodes(p, &nodes)
                    .map_err(|e| Fault::internal("native_protocol", e.message))
            }
            Ok(Response::ForeignError { foreign_error: f }) => cpl_eval::boundary::foreign_failure(
                p,
                self.capability,
                index,
                &self.binding,
                &f.kind,
                &f.message,
                &f.details,
            ),
            Ok(Response::Fault(answer)) => Err(crate::fault_from_record(p, &answer.fault)),
            Ok(Response::Error { error }) => {
                Err(Fault::internal("native_protocol", error.message.clone()))
            }
            Ok(_) => Err(Fault::internal(
                "native_protocol",
                "cap_call got an unexpected answer".to_string(),
            )),
            // the driver died, during this call or before it (spec/cabi.md 7.2 item 4): Transport, or
            // binding_host_failed without a foreign-error return type
            Err(e) => match self.session.ended() {
                Some(end) => {
                    cpl_eval::boundary::host_ended(p, self.capability, index, &self.binding, end)
                }
                None => Err(engine_fault(e)),
            },
        }
    }
}

/// The hosts of `cpl run` (semantics S78): `available` is whether the plan has a backend-c binding of the parameter's
/// capability (`InteropPlan::binding_for`) and reads nothing else; `create`, asked in `using`-parameter order, builds
/// (or takes from the cache), spawns, and configures the program's driver at its first call and grants the binding
/// in it (`spec/cabi.md` 7.2), giving a CAdapter bound with `BindOptions { label: "<binding>", validate_results: true }`.
pub struct ManifestHosts<'a> {
    p: &'a Program,
    project: &'a Project,
    plan: &'a InteropPlan,
    driver: &'a DriverConfig,
    session: Rc<RefCell<Option<NativeSession<'a>>>>,
    /// (parameter name, binding, handle) of every host created, in creation order
    pub created: Vec<(String, String, u32)>,
    /// Milestone 3: the Python workers' configuration (`spec/python.md` 6.5).
    workers: crate::worker::WorkerConfig,
    /// The Python worker source and its digest, generated at the first Python grant.
    python: Option<(String, String)>,
    /// The banner `runtime` of every Python worker started, in creation order (receipts' `environment`).
    pub runtimes: Vec<serde_json::Value>,
}

impl ManifestHosts<'_> {
    /// The Python interpreter of the workers (`--python`), else `CPL_PYTHON`, else `python3` (`spec/python.md` 6.5).
    pub fn with_python(mut self, python: Option<&std::path::Path>) -> Self {
        self.workers = crate::worker::WorkerConfig::for_project(self.project, python);
        self
    }

    /// The backend-python binding of a capability, by the manifest (`spec/python.md` 7).
    fn python_binding(&self, cap: DefId) -> Option<(&String, &cpl_project::BindingSection)> {
        let path = self.p.display_path(cap);
        self.project
            .manifest
            .bindings
            .iter()
            .find(|(_, b)| b.backend == "python" && b.capability == path)
    }
}

impl RunHosts for ManifestHosts<'_> {
    fn available(&self, param: &ParamDef) -> bool {
        match &param.ty {
            Ty::Capability(c) => {
                self.plan.binding_for(*c).is_some() || self.python_binding(*c).is_some()
            }
            _ => false,
        }
    }

    fn create(
        &mut self,
        param: &ParamDef,
    ) -> Result<(Box<dyn CapabilityImpl>, BindOptions), Vec<Diagnostic>> {
        let Ty::Capability(cap) = &param.ty else {
            return Err(Vec::new());
        };
        if self.plan.binding_for(*cap).is_none() {
            // Milestone 3: a backend-python binding, in a worker whatever its isolation (spec/python.md 7)
            let Some((name, _)) = self.python_binding(*cap) else {
                return Err(Vec::new());
            };
            let name = name.clone();
            if self.python.is_none() {
                let exports: Vec<DefId> = self.plan.exports.iter().map(|e| e.function).collect();
                let model =
                    cpl_emit_c::python::PyModel::new(self.p, &self.project.manifest, &exports);
                self.python = Some((model.worker_source(), model.digest.clone()));
            }
            let (source, digest) = self.python.as_ref().expect("the worker source");
            let timeout = self
                .project
                .manifest
                .binding_timeouts
                .get(&name)
                .map(|ms| std::time::Duration::from_millis(*ms as u64));
            let worker =
                crate::worker::Worker::start_python(source, digest, &name, &self.workers, timeout)
                    .map_err(|d| vec![d])?;
            self.runtimes.push(worker.runtime.clone());
            let handle = self.created.len() as u32;
            self.created
                .push((param.name.clone(), name.clone(), handle));
            return Ok((
                Box::new(crate::worker::WorkerAdapter::new(worker, *cap)),
                BindOptions {
                    label: name,
                    validate_results: true,
                },
            ));
        }
        let Some(b) = self.plan.binding_for(*cap) else {
            return Err(Vec::new());
        };
        let mut slot = self.session.borrow_mut();
        if slot.is_none() {
            let s = NativeSession::start(
                self.p,
                self.project,
                self.plan,
                &EvalConfig::default(),
                self.driver,
            )
            .map_err(|e| e.into_diagnostics())?;
            *slot = Some(s);
        }
        let session = slot.as_mut().expect("a session");
        let handle = session.bind(&b.name).map_err(|e| e.into_diagnostics())?;
        // the handle a run reports is its grant's position in parameter order, as the native engine's driver numbers
        // every grant of a run (C and Python alike) in parameter order
        let number = self.created.len() as u32;
        self.created
            .push((param.name.clone(), b.name.clone(), number));
        let adapter = CAdapter {
            session: session.raw(),
            handle: handle.0,
            capability: *cap,
            binding: b.name.clone(),
        };
        Ok((
            Box::new(adapter),
            BindOptions {
                label: b.name.clone(),
                validate_results: true,
            },
        ))
    }
}

pub fn manifest_hosts<'a>(
    p: &'a Program,
    project: &'a Project,
    plan: &'a InteropPlan,
    driver: &'a DriverConfig,
) -> ManifestHosts<'a> {
    ManifestHosts {
        p,
        project,
        plan,
        driver,
        session: Rc::new(RefCell::new(None)),
        created: Vec::new(),
        workers: crate::worker::WorkerConfig::for_project(project, None),
        python: None,
        runtimes: Vec::new(),
    }
}
