//! Structured application concurrency: a bounded mailbox, one state owner, and replies after atomic commit.
//! Multiple producers may submit events concurrently; the receive order determines the replayable sequence.
use crate::{
    machine::{MachineContext, TransitionRecord},
    EvalConfig, Interpreter,
};
use cpl_diag::{Category, DiagCode, Diagnostic};
use cpl_hir::{DefId, Program};
use serde_json::Value as Json;
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};

fn closed() -> Diagnostic {
    Diagnostic::error(
        DiagCode("machine_mailbox_closed"),
        Category::Runtime,
        "the machine task has finished",
    )
}

enum Message {
    Event(Json, SyncSender<Result<TransitionRecord, Diagnostic>>),
    Stop,
}

/// A cloneable, thread-safe producer. Queue capacity bounds pending events; a full queue never blocks submission.
#[derive(Clone)]
pub struct MachineMailbox {
    sender: SyncSender<Message>,
}

/// A rejected submission, retaining the event so a host can retry without reconstructing it.
#[derive(Debug)]
pub enum SubmitError {
    Full(Json),
    Closed(Json),
}

/// The completion of one accepted event. Dropping a reply does not cancel an accepted transition.
pub struct TransitionReply {
    receiver: Receiver<Result<TransitionRecord, Diagnostic>>,
}
impl TransitionReply {
    pub fn wait(self) -> Result<TransitionRecord, Diagnostic> {
        self.receiver.recv().unwrap_or_else(|_| Err(closed()))
    }
    pub fn try_recv(&self) -> Result<Option<Result<TransitionRecord, Diagnostic>>, Diagnostic> {
        match self.receiver.try_recv() {
            Ok(result) => Ok(Some(result)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(mpsc::TryRecvError::Disconnected) => Err(closed()),
        }
    }
}
impl MachineMailbox {
    pub fn try_dispatch(&self, event: Json) -> Result<TransitionReply, SubmitError> {
        let (sender, receiver) = mpsc::sync_channel(1);
        match self.sender.try_send(Message::Event(event, sender)) {
            Ok(()) => Ok(TransitionReply { receiver }),
            Err(TrySendError::Full(Message::Event(event, _))) => Err(SubmitError::Full(event)),
            Err(TrySendError::Disconnected(Message::Event(event, _))) => {
                Err(SubmitError::Closed(event))
            }
            _ => unreachable!("only event messages are submitted"),
        }
    }
}

/// Run a machine on an owned evaluation thread while `run` uses its mailbox. Join on normal or panic exit.
/// Events queued before scope exit are drained before shutdown. Commands remain data, never executed here.
/// Escaped mailbox clones become closed after the scope. Choose a time budget to bound each event's computation.
pub fn with_machine<R>(
    program: &Program,
    machine: DefId,
    state: Json,
    config: EvalConfig,
    capacity: usize,
    run: impl FnOnce(MachineMailbox) -> R,
) -> Result<R, Diagnostic> {
    if capacity == 0 {
        return Err(Diagnostic::error(
            DiagCode("machine_mailbox_capacity"),
            Category::Runtime,
            "mailbox capacity must be positive",
        ));
    }
    let (sender, receiver) = mpsc::sync_channel(capacity);
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    std::thread::scope(|scope| {
        let worker = std::thread::Builder::new()
            .name("cpl-machine".into())
            .stack_size(crate::stack::EVAL_STACK_SIZE)
            .spawn_scoped(scope, move || {
                crate::stack::set_guard(crate::stack::EVAL_STACK_SIZE);
                let mut context = match MachineContext::new(program, machine, &state, &config) {
                    Ok(c) => c,
                    Err(d) => {
                        let _ = ready_tx.send(Err(d));
                        return;
                    }
                };
                if ready_tx.send(Ok(())).is_err() {
                    return;
                }
                let mut engine = Interpreter::new(program, config);
                while let Ok(message) = receiver.recv() {
                    match message {
                        Message::Event(event, reply) => {
                            let _ = reply.send(context.dispatch(&mut engine, &event));
                        }
                        Message::Stop => break,
                    }
                }
            })
            .map_err(|e| {
                Diagnostic::error(
                    DiagCode("machine_task_unavailable"),
                    Category::Runtime,
                    format!("cannot start machine task: {e}"),
                )
            })?;
        ready_rx.recv().map_err(|_| closed())??;
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            run(MachineMailbox {
                sender: sender.clone(),
            })
        }));
        let _ = sender.send(Message::Stop);
        worker
            .join()
            .unwrap_or_else(|p| std::panic::resume_unwind(p));
        match result {
            Ok(value) => Ok(value),
            Err(panic) => std::panic::resume_unwind(panic),
        }
    })
}
