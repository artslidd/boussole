//! One thread for every [`Host::start`] and [`Host::stop`] call.
//!
//! The embedded server and its tunnel are a single resource: a `stop` must never run while a
//! `start` is still in flight, and a `start` that was abandoned must not come up after (and
//! stop, or be stopped by) a newer one. So every call goes through one queue and runs strictly
//! after the previous one; "cancel" is a `Stop` queued behind whatever is in flight, plus
//! [`Host::cancel_start`] to make a slow start give up at once.

use crate::host::{Host, HostError, HostInfo, HostMode};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Sender, channel};

enum Command {
    Start { session: u64, mode: HostMode },
    Stop,
}

/// What the worker reports (from its own thread).
pub enum HostEvent {
    /// The worker picked this start up: it is running now (before that it was queued behind
    /// an earlier attempt being closed).
    Began { session: u64 },
    /// A start ended. A start whose session is no longer the live one is never run: it ends at
    /// once with [`HostError::Cancelled`].
    Started {
        session: u64,
        result: Result<HostInfo, HostError>,
    },
}

/// The queue in front of a [`Host`].
pub struct HostWorker {
    commands: Sender<Command>,
    /// Calls queued or running.
    pending: Arc<AtomicUsize>,
    host: Arc<dyn Host>,
}

impl HostWorker {
    /// Start the worker thread. `live_session` is the session the UI is in now: a queued start
    /// of any other session is dropped without being run. `report` is called from the worker
    /// thread for every [`HostEvent`].
    pub fn spawn(
        host: Arc<dyn Host>,
        live_session: Arc<AtomicU64>,
        report: impl Fn(HostEvent) + Send + 'static,
    ) -> Self {
        let (commands, queue) = channel::<Command>();
        let pending = Arc::new(AtomicUsize::new(0));
        let worker_host = host.clone();
        let worker_pending = pending.clone();
        std::thread::Builder::new()
            .name("boussole-host-worker".into())
            .spawn(move || {
                // Ends when the `HostWorker` is dropped, after the queue has been served.
                while let Ok(command) = queue.recv() {
                    match command {
                        Command::Start { session, mode } => {
                            if live_session.load(Ordering::SeqCst) != session {
                                report(HostEvent::Started {
                                    session,
                                    result: Err(HostError::Cancelled),
                                });
                            } else {
                                report(HostEvent::Began { session });
                                let result = worker_host.start(mode);
                                report(HostEvent::Started { session, result });
                            }
                        }
                        Command::Stop => worker_host.stop(),
                    }
                    worker_pending.fetch_sub(1, Ordering::SeqCst);
                }
            })
            .expect("spawning the host worker");
        Self {
            commands,
            pending,
            host,
        }
    }

    /// Queue a start for `session`.
    pub fn start(&self, session: u64, mode: HostMode) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let _ = self.commands.send(Command::Start { session, mode });
    }

    /// Queue a stop behind whatever is already queued or running.
    pub fn stop(&self) {
        self.pending.fetch_add(1, Ordering::SeqCst);
        let _ = self.commands.send(Command::Stop);
    }

    /// Make the start in flight (if any) give up now. Returns at once.
    pub fn cancel_start(&self) {
        self.host.cancel_start();
    }

    /// Calls queued or running.
    pub fn pending(&self) -> usize {
        self.pending.load(Ordering::SeqCst)
    }
}
