//! Structured parallel maps. Only flat owned values cross threads; capability hosts stay on their owner.
use super::{Env, Interpreter};
use crate::interp::eval::{Eval, Flow};
use crate::run::Portable;
use crate::{Fault, Value};
use cpl_hir::{Expr, LocalId};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock};

static LIMIT: OnceLock<usize> = OnceLock::new();
static ACTIVE: AtomicUsize = AtomicUsize::new(0);

fn limit() -> usize {
    *LIMIT.get_or_init(|| {
        std::env::var("CPL_PARALLELISM")
            .ok()
            .filter(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit()))
            .and_then(|v| v.parse::<usize>().ok())
            .filter(|n| (1..=64).contains(n))
            .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get().min(8)))
    })
}

struct Permits(usize);
impl Permits {
    fn acquire(wanted: usize) -> Self {
        let max = limit().saturating_sub(1);
        let mut current = ACTIVE.load(Ordering::Relaxed);
        loop {
            let n = wanted.min(max.saturating_sub(current));
            match ACTIVE.compare_exchange_weak(
                current,
                current + n,
                Ordering::AcqRel,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Self(n),
                Err(actual) => current = actual,
            }
        }
    }
}
impl Drop for Permits {
    fn drop(&mut self) {
        ACTIVE.fetch_sub(self.0, Ordering::Release);
    }
}

impl<'p> Interpreter<'p> {
    pub(super) fn eval_parallel(
        &mut self,
        binder: LocalId,
        collection: &'p Expr,
        body: &'p Expr,
    ) -> Eval<Value> {
        let items = self.eval_expr(collection)?;
        let Value::List(items) = &items else {
            return self.confusion("a list", &items, collection.span);
        };
        let permits = Permits::acquire(if self.parallel_depth == 0 {
            items.len().saturating_sub(1)
        } else {
            0
        });
        if permits.0 == 0 {
            let mut values = Vec::with_capacity(items.len());
            self.parallel_depth += 1;
            for item in items.iter() {
                if let Err(flow) = self.check_limits(Some(body.span)) {
                    self.parallel_depth -= 1;
                    return Err(flow);
                }
                let mark = self.scope_mark();
                self.bind(binder, item.clone());
                let result = self.eval_expr(body);
                self.restore_scope(mark);
                match result {
                    Ok(value) => values.push(value),
                    Err(flow) => {
                        self.parallel_depth -= 1;
                        return Err(flow);
                    }
                }
            }
            self.parallel_depth -= 1;
            return Ok(Value::list(values));
        }
        let workers = permits.0 + 1;
        let chunk = items.len().div_ceil(workers);
        let inputs: Vec<Portable> = items
            .chunks(chunk)
            .map(|xs| Portable::from(&Value::list(xs.to_vec())))
            .collect();
        let env = self.env();
        let captures = Portable::from(&Value::list(
            env.values
                .iter()
                .map(|v| v.clone().unwrap_or(Value::Bool(false)))
                .collect(),
        ));
        let present: Vec<bool> = env.values.iter().map(Option::is_some).collect();
        let table = env.table;
        let scope = env.scope.clone();
        let program = self.program;
        let config = self.config.clone();
        let frames = self.frames.clone();
        let measures = self.measures.clone();
        let calls = self.calls;
        let deadline = self.deadline;
        let stop = Arc::new(AtomicUsize::new(usize::MAX));
        let run = |part: usize, input: &Portable| {
            let mut child = Interpreter::new(program, config.clone());
            child.parallel_depth = 1;
            child.frames = frames.clone();
            child.measures = measures.clone();
            child.calls = calls;
            child.deadline = deadline;
            let values = Value::from(captures.clone());
            let Value::List(values) = &values else {
                unreachable!()
            };
            child.envs.push(Env {
                table,
                scope: scope.clone(),
                values: values
                    .iter()
                    .zip(&present)
                    .map(|(v, present)| present.then(|| v.clone()))
                    .collect(),
            });
            let input = Value::from(input.clone());
            let Value::List(input) = &input else {
                unreachable!()
            };
            let result: Result<Portable, Fault> = (|| {
                let mut output = Vec::with_capacity(input.len());
                for (offset, item) in input.iter().enumerate() {
                    let index = part * chunk + offset;
                    child.parallel_cancel = Some((stop.clone(), index));
                    if let Err(flow) = child.check_limits(Some(body.span)) {
                        stop.fetch_min(index, Ordering::AcqRel);
                        return Err(flow.into_fault());
                    }
                    let mark = child.scope_mark();
                    child.bind(binder, item.clone());
                    let result = child.eval_expr(body);
                    child.restore_scope(mark);
                    match result {
                        Ok(value) => output.push(value),
                        Err(flow) => {
                            stop.fetch_min(index, Ordering::AcqRel);
                            return Err(flow.into_fault());
                        }
                    }
                }
                Ok(Portable::from(&Value::list(output)))
            })();
            (result, child.attributed, child.violation)
        };
        let results = std::thread::scope(|s| {
            let mut jobs = Vec::new();
            for (part, input) in inputs[..inputs.len() - 1].iter().enumerate() {
                let run = &run;
                let handle = std::thread::Builder::new()
                    .name("cpl-parallel".into())
                    .stack_size(crate::stack::EVAL_STACK_SIZE)
                    .spawn_scoped(s, move || {
                        crate::stack::set_guard(crate::stack::EVAL_STACK_SIZE);
                        run(part, input)
                    });
                jobs.push(match handle {
                    Ok(h) => Ok(h),
                    Err(_) => Err(run(part, input)),
                });
            }
            let last = run(inputs.len() - 1, inputs.last().unwrap());
            let mut results = Vec::with_capacity(inputs.len());
            for job in jobs {
                results.push(match job {
                    Ok(h) => h.join().unwrap_or_else(|p| std::panic::resume_unwind(p)),
                    Err(result) => result,
                });
            }
            results.push(last);
            results
        });
        let mut output = Vec::with_capacity(items.len());
        for (result, attributed, violation) in results {
            match result {
                Ok(value) => {
                    let value = Value::from(value);
                    let Value::List(xs) = &value else {
                        unreachable!()
                    };
                    output.extend(xs.iter().cloned());
                }
                Err(fault) => {
                    self.attributed = attributed;
                    self.violation = violation;
                    return Err(Flow::Fault(Box::new(fault)));
                }
            }
        }
        Ok(Value::list(output))
    }
}
