//! Effect-aware task scopes. Resource identities and read classifications are supplied by the host.
//! Aliases share a resource identity; every unspecified operation is exclusive. Conflicts require an
//! explicit dependency before any effect runs. Replay serves recorded results without contacting hosts.
use crate::{error, verify::VerifyConfig, Snapshot};
use cpl_diag::Diagnostic;
use cpl_eval::{BindOptions, CapabilityImpl, Fault, HostContext, RunHosts};
use cpl_hir::{DefId, DefKind, ParamDef, Ty};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Access {
    Read,
    #[default]
    Exclusive,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resource {
    /// A canonical identity for the actual host resource, shared by every alias of it.
    pub identity: String,
    pub capability: String,
    #[serde(default)]
    pub operations: BTreeMap<String, Access>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub id: String,
    pub function: String,
    #[serde(default)]
    pub arguments: Vec<Value>,
    #[serde(default)]
    pub using: BTreeMap<String, String>,
    #[serde(default)]
    pub after: BTreeSet<String>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Scope {
    pub tasks: Vec<Task>,
    #[serde(default)]
    pub resources: BTreeMap<String, Resource>,
    #[serde(default)]
    pub verification: VerifyConfig,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Plan {
    pub waves: Vec<Vec<String>>,
    pub footprints: BTreeMap<String, BTreeMap<String, Access>>,
    pub digest: String,
}

pub trait EffectHost: Send + Sync {
    fn call(&self, resource: &str, operation: &str, arguments: &[Value]) -> Result<Value, String>;
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Event {
    pub sequence: u64,
    pub task: String,
    pub resource: String,
    pub operation: String,
    pub arguments: Vec<Value>,
    pub response: Result<Value, String>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Execution {
    pub snapshot: String,
    pub plan: Plan,
    pub results: Vec<Value>,
    pub events: Vec<Event>,
    pub completed: bool,
}

pub fn plan(s: &Snapshot, scope: &Scope) -> Result<Plan, Diagnostic> {
    scope.verification.validate()?;
    let p = &s.checked.program;
    let ids: BTreeSet<_> = scope.tasks.iter().map(|t| t.id.clone()).collect();
    if ids.len() != scope.tasks.len() || ids.iter().any(|id| id.is_empty()) {
        return Err(error(
            "concurrent_task_identity",
            "task identities must be nonempty and unique",
        ));
    }
    let mut capabilities = BTreeMap::new();
    for (name, resource) in &scope.resources {
        if resource.identity.is_empty() {
            return Err(error(
                "concurrent_resource_identity",
                "resource identities must be nonempty",
            ));
        }
        let id = s.resolve(&resource.capability)?;
        let DefKind::Capability(cap) = &p.def(id).kind else {
            return Err(error(
                "concurrent_capability",
                "a resource must name a capability",
            ));
        };
        if resource
            .operations
            .keys()
            .any(|name| !cap.ops.iter().any(|op| &op.name == name))
        {
            return Err(error(
                "concurrent_operation",
                "resource metadata names an unknown operation",
            ));
        }
        capabilities.insert(name, id);
    }
    let mut ancestors = BTreeMap::<String, BTreeSet<String>>::new();
    let mut footprints = BTreeMap::new();
    for task in &scope.tasks {
        if task
            .after
            .iter()
            .any(|id| !ids.contains(id) || id == &task.id)
        {
            return Err(error(
                "concurrent_dependency",
                "unknown or self task dependency",
            ));
        }
        ancestors.insert(task.id.clone(), task.after.clone());
        let id = s.resolve(&task.function)?;
        let f = p
            .def(id)
            .kind
            .as_fn()
            .ok_or_else(|| error("concurrent_function", "tasks must name functions"))?;
        if task.using.len() != f.using.len() {
            return Err(error(
                "concurrent_binding",
                "bind exactly the declared capability parameters",
            ));
        }
        for param in &f.using {
            let resource = task
                .using
                .get(&param.name)
                .and_then(|name| capabilities.get(name));
            if !resource.is_some_and(|id| param.ty == Ty::Capability(*id)) {
                return Err(error(
                    "concurrent_binding",
                    format!("invalid resource for {}.{}", task.id, param.name),
                ));
            }
        }
        cpl_eval::prepare_run(
            p,
            &p.symbol_path(id),
            &task
                .arguments
                .iter()
                .cloned()
                .map(cpl_eval::RunArg::Json)
                .collect::<Vec<_>>(),
            &|_| true,
        )
        .map_err(|e| error("concurrent_arguments", e.to_string()))?;
        let mut uses = BTreeMap::new();
        for effect in f
            .effects
            .declared
            .iter()
            .flatten()
            .chain(f.effects.inferred.iter())
        {
            let param = f.using.iter().find(|u| u.local == effect.param).unwrap();
            let resource = &scope.resources[&task.using[&param.name]];
            let access = resource
                .operations
                .get(&effect.op)
                .copied()
                .unwrap_or_default();
            let previous = uses.entry(resource.identity.clone()).or_insert(access);
            if access == Access::Exclusive {
                *previous = access;
            }
        }
        footprints.insert(task.id.clone(), uses);
    }
    loop {
        let old = ancestors.clone();
        for parents in ancestors.values_mut() {
            for id in parents.clone() {
                parents.extend(old[&id].iter().cloned());
            }
        }
        if ancestors == old {
            break;
        }
    }
    if ancestors.iter().any(|(id, parents)| parents.contains(id)) {
        return Err(error("concurrent_cycle", "task dependencies form a cycle"));
    }
    for (i, a) in scope.tasks.iter().enumerate() {
        for b in &scope.tasks[i + 1..] {
            let conflict = footprints[&a.id].iter().any(|(resource, access)| {
                footprints[&b.id].get(resource).is_some_and(|other| {
                    *access == Access::Exclusive || *other == Access::Exclusive
                })
            });
            if conflict && !ancestors[&a.id].contains(&b.id) && !ancestors[&b.id].contains(&a.id) {
                return Err(error("concurrent_order_required",format!("{} and {} may access the same resource exclusively; add an explicit after dependency",a.id,b.id)));
            }
        }
    }
    let mut done = BTreeSet::new();
    let mut waves = Vec::new();
    while done.len() < ids.len() {
        let wave: Vec<_> = ids
            .iter()
            .filter(|id| !done.contains(*id) && ancestors[*id].is_subset(&done))
            .cloned()
            .collect();
        done.extend(wave.iter().cloned());
        waves.push(wave);
    }
    let digest =
        blake3::hash(&serde_json::to_vec(&json!({"snapshot":s.digest(),"scope":scope})).unwrap())
            .to_hex()
            .to_string();
    Ok(Plan {
        waves,
        footprints,
        digest,
    })
}

struct Hosts {
    task: String,
    resources: BTreeMap<String, (Resource, DefId)>,
    host: Arc<dyn EffectHost>,
    events: Arc<Mutex<Vec<Event>>>,
}
impl RunHosts for Hosts {
    fn available(&self, param: &ParamDef) -> bool {
        self.resources.contains_key(&param.name)
    }
    fn create(
        &mut self,
        param: &ParamDef,
    ) -> Result<(Box<dyn CapabilityImpl>, BindOptions), Vec<Diagnostic>> {
        let (resource, capability) = self.resources[&param.name].clone();
        Ok((
            Box::new(Proxy {
                task: self.task.clone(),
                resource: resource.identity.clone(),
                capability,
                host: self.host.clone(),
                events: self.events.clone(),
                forks: Default::default(),
            }),
            BindOptions {
                label: resource.identity,
                validate_results: true,
            },
        ))
    }
}
struct Proxy {
    task: String,
    resource: String,
    capability: DefId,
    host: Arc<dyn EffectHost>,
    events: Arc<Mutex<Vec<Event>>>,
    /// The record buffers of this handle's forks, oldest first: forks join in task order.
    forks: std::collections::VecDeque<Arc<Mutex<Vec<Event>>>>,
}
impl CapabilityImpl for Proxy {
    fn call(
        &mut self,
        op: &str,
        args: Vec<cpl_eval::Value>,
        ctx: &mut HostContext<'_>,
    ) -> Result<cpl_eval::Value, Fault> {
        let DefKind::Capability(cap) = &ctx.program.def(self.capability).kind else {
            unreachable!()
        };
        let operation = cap.ops.iter().find(|o| o.name == op).unwrap();
        let arguments: Vec<_> = args
            .iter()
            .zip(&operation.params)
            .map(|(arg, param)| cpl_eval::value_to_json(ctx.program, arg, &param.ty))
            .collect();
        let response = self.host.call(&self.resource, op, &arguments);
        self.events.lock().unwrap().push(Event {
            sequence: 0,
            task: self.task.clone(),
            resource: self.resource.clone(),
            operation: op.into(),
            arguments,
            response: response.clone(),
        });
        let value = response.map_err(|e| Fault::internal("concurrent_host", e))?;
        cpl_eval::value_from_json(ctx.program, &value, &operation.ret)
            .map_err(|e| Fault::internal("concurrent_host_result", e.to_string()))
    }

    /// Hosts are thread-safe (`EffectHost: Send + Sync`): a fork records into its own buffer.
    fn fork(&mut self) -> Option<Box<dyn CapabilityImpl + Send>> {
        let buffer = Arc::new(Mutex::new(Vec::new()));
        self.forks.push_back(buffer.clone());
        Some(Box::new(Proxy {
            task: self.task.clone(),
            resource: self.resource.clone(),
            capability: self.capability,
            host: self.host.clone(),
            events: buffer,
            forks: Default::default(),
        }))
    }

    /// Forks join in task order, so appending their buffers keeps the trace deterministic.
    fn join(&mut self, _forked: Box<dyn CapabilityImpl + Send>) {
        if let Some(buffer) = self.forks.pop_front() {
            let recorded = std::mem::take(&mut *buffer.lock().unwrap());
            self.events.lock().unwrap().extend(recorded);
        }
    }

    fn resource(&mut self) -> Option<String> {
        Some(self.resource.clone())
    }
}

/// Join every started task, including on fault. Results and event numbers are in deterministic plan order,
/// not wall-clock completion order. Events within each task retain host invocation order.
pub fn execute(
    s: &Snapshot,
    scope: &Scope,
    host: Arc<dyn EffectHost>,
    workers: usize,
) -> Result<Execution, Diagnostic> {
    let plan = plan(s, scope)?;
    execute_with(s, scope, plan, workers, |_| host.clone())
}
fn execute_with(
    s: &Snapshot,
    scope: &Scope,
    plan: Plan,
    workers: usize,
    host: impl Fn(&str) -> Arc<dyn EffectHost> + Sync,
) -> Result<Execution, Diagnostic> {
    if workers == 0 {
        return Err(error("concurrent_workers", "worker limit must be positive"));
    }
    let mut execution = Execution {
        snapshot: s.digest(),
        plan: plan.clone(),
        results: vec![],
        events: vec![],
        completed: true,
    };
    for wave in &plan.waves {
        for chunk in wave.chunks(workers) {
            let results = std::thread::scope(|threads| {
                let handles:Vec<_>=chunk.iter().map(|name| {
                    let task=scope.tasks.iter().find(|t| &t.id==name).unwrap(); let host=host(&task.id);
                    threads.spawn(move || {
                        let p=&s.checked.program; let id=s.resolve(&task.function).unwrap();
                        let events=Arc::new(Mutex::new(vec![]));
                        let mut hosts=Hosts {task:task.id.clone(),resources:task.using.iter().map(|(param,name)|(param.clone(),(scope.resources[name].clone(),s.resolve(&scope.resources[name].capability).unwrap()))).collect(),host,events:events.clone()};
                        let outcome=cpl_eval::run_function_with_hosts(p,&p.symbol_path(id),&task.arguments.iter().cloned().map(cpl_eval::RunArg::Json).collect::<Vec<_>>(),&scope.verification.eval(),&mut hosts);
                        let result=match outcome {
                            Ok(r)=>match r.value {Ok(v)=>json!({"task":task.id,"value":cpl_eval::value_to_json(p,&v,&r.return_ty)}),Err(f)=>json!({"task":task.id,"fault":f.to_diagnostic(p).to_json(&p.source_map)})},
                            Err(e)=>json!({"task":task.id,"error":e.to_string()}),
                        };
                        let events=events.lock().unwrap().clone(); (result,events)
                    })
                }).collect();
                handles
                    .into_iter()
                    .map(|h| {
                        h.join().map_err(|_| {
                            error(
                                "concurrent_worker_failed",
                                "a host worker panicked; all started workers were joined",
                            )
                        })
                    })
                    .collect::<Result<Vec<_>, _>>()
            })?;
            for (result, events) in results {
                if result.get("value").is_none() {
                    execution.completed = false;
                }
                execution.results.push(result);
                execution.events.extend(events);
            }
            // Complete the logical wave, irrespective of worker count, so fault traces replay deterministically.
        }
        if !execution.completed {
            break;
        }
    }
    for (i, event) in execution.events.iter_mut().enumerate() {
        event.sequence = i as u64 + 1;
    }
    Ok(execution)
}
struct ReplayHost {
    events: Mutex<std::collections::VecDeque<Event>>,
}
impl EffectHost for ReplayHost {
    /// Overlapped tasks of a `concurrent` block may call in any order, so a call takes the first
    /// recorded event with its resource, operation, and arguments; the complete trace, rebuilt in
    /// task order, is compared afterwards.
    fn call(&self, resource: &str, operation: &str, arguments: &[Value]) -> Result<Value, String> {
        let mut events = self.events.lock().unwrap();
        let Some(position) = events.iter().position(|e| {
            e.resource == resource && e.operation == operation && e.arguments == arguments
        }) else {
            return Err(if events.is_empty() {
                "unexpected effect during replay".into()
            } else {
                "effect arguments or resource diverged during replay".into()
            });
        };
        events
            .remove(position)
            .expect("the position is in range")
            .response
    }
}
pub fn replay(
    s: &Snapshot,
    scope: &Scope,
    expected: &Execution,
    workers: usize,
) -> Result<Execution, Diagnostic> {
    let plan = plan(s, scope)?;
    if expected.snapshot != s.digest()
        || expected.plan != plan
        || expected
            .events
            .iter()
            .enumerate()
            .any(|(i, e)| e.sequence != i as u64 + 1)
    {
        return Err(error(
            "concurrent_replay_stale",
            "the trace does not match this snapshot, plan, or event sequence",
        ));
    }
    let hosts: BTreeMap<_, Arc<ReplayHost>> = scope
        .tasks
        .iter()
        .map(|t| {
            (
                t.id.clone(),
                Arc::new(ReplayHost {
                    events: Mutex::new(
                        expected
                            .events
                            .iter()
                            .filter(|e| e.task == t.id)
                            .cloned()
                            .collect(),
                    ),
                }),
            )
        })
        .collect();
    let actual = execute_with(s, scope, plan, workers, |id| hosts[id].clone())?;
    if &actual != expected || hosts.values().any(|h| !h.events.lock().unwrap().is_empty()) {
        return Err(error(
            "concurrent_replay_mismatch",
            "recomputed tasks disagree with the recorded effects or results",
        ));
    }
    Ok(actual)
}
