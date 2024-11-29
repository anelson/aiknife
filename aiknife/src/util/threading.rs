//! Some utility code for cases when some work needs to be done in a separate thread, which can be
//! aborted, can report an error, and produces a stream of work product that can be consumed by the
//! caller of this work in another thread.
//!
//! The basic pattern here is to use crossbeam channels in a few ways:
//! - A bounded(0) channel of type `()` is used to signal to the thread that it should stop its
//!   work.
//! - A bounded(0) channel of type `Result<T>` is used indicate the success or failure of the
//!   initialization part of the work.
//! - A higher-bounded channel of type `Result<T>` is used to produce results of the work.
//!
//! # Thread Workers
//!
//! The bit of work that needs to happen in another thread is generalized into the concept of a
//! "thread worker".  Not much is assumed about this work, other than that it's blocking (no async
//! runtime is provided), it's fallible, it has a (potentially fallible) initialization step that
//! also must be run in the worker thread, and it produces a stream of work product.
//!
//! To keep things reasonably simple, traits are used to represent both the initialization step and
//! the running step.  Unfortunately that means that the implementation of the thread worker isn't
//! just a simple function but needs a type to hang the trait impl on.  This isn't ideal but this
//! entire module is a yak-shaving exercise to indulge my need for clean abstractions, and I'm not
//! about to waste days getting all of the short and curly hairs off of this yak.  Actually maybe I
//! am, but I'm trying very hard not to.

use anyhow::Result;
use crossbeam::channel::{bounded, Receiver, Sender};
use std::thread::{self, JoinHandle};
use tracing::*;

/// Create a stop signal sender/receiver pair for a thread worker.
fn new_stop_signal<T: ThreadWorkerWork>() -> (StopSignalSender, StopSignalReceiver) {
    let worker = T::worker_name();

    // Need a bound of 1 here.
    // A bound of 0 means the send will block (or fail for non-blocking sends) if the worker is not
    // actively at this very moment blocking on a stop signal.  In almost all cases the worker does
    // work besides waiting for a stop signal, so this isn't what we want.
    // A bound of > 1 doesn't make sense either, because a worker is expected to receive the stop
    // signal and stop immediately.  If the channel is already full with a stop signal, then we
    // simply log the fact that a stop is already signaled.
    let (sender, receiver) = bounded(1);
    (
        StopSignalSender { sender, worker },
        StopSignalReceiver { receiver, worker },
    )
}

/// Send a stop signal to a spawned thread worker.
///
/// Note that this also works as a drop guard; if this sender (and all of its clones) are dropped,
/// then that is equivalent to sending the stop signal.
///
/// Stop signal senders can be cloned very cheaply as they are just a [`crossbeam::channel::Sender`]
/// internally.
#[derive(Clone, Debug)]
pub(crate) struct StopSignalSender {
    sender: Sender<()>,
    worker: &'static str,
}

impl StopSignalSender {
    /// Signal to whatever worker is on the other end of this sender that it should stop what it is
    /// doing.
    ///
    /// If the worker has already stopped, this will do nothing (but it does log the fact that this
    /// happened).
    /// If the worker was already told to stop and has not yet stopped, this will also do nothing
    /// (but, again, will log the fact that this happened).
    ///
    /// This method is never blocking regardless of the state of the worker.
    pub(crate) fn signal_stop(self) {
        match self.sender.try_send(()) {
            Ok(()) => trace!(worker = self.worker, "Stop signal sent to worker"),
            Err(crossbeam::channel::TrySendError::Disconnected(_)) => {
                debug!(worker = self.worker, "Stop signal was not sent because the worker has dropped the receiver (and therefore is probably already stopped)");
            }
            Err(crossbeam::channel::TrySendError::Full(_)) => {
                debug!(worker = self.worker, "Stop signal was not sent to worker because another stop signal was already sent");
            }
        }
    }
}

/// Receives a stop signal sent by some caller to a thread worker.
///
/// Note that this considers a stop signal to be *either* a [`StopSignalSender`] sending an
/// explicit signal by calling [`StopSignalSender::signal_stop`], *or* if all associated
/// [`StopSignalSender`] senders have been dropped, meaning no stop signal will ever be sent.
#[derive(Clone, Debug)]
pub(crate) struct StopSignalReceiver {
    receiver: Receiver<()>,
    worker: &'static str,
}

impl StopSignalReceiver {
    /// Check if a stop signal has been received, without blocking.
    ///
    /// Returns `true` either when the stop signal has been received, or when all senders have been
    /// dropped.
    pub(crate) fn is_stop_signaled(&self) -> bool {
        match self.receiver.try_recv() {
            Ok(()) => {
                debug!(worker = self.worker, "Stop signal received");
                true
            }
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                debug!(
                    worker = self.worker,
                    "All stop signal senders have been dropped; treating this as a stop signal"
                );
                true
            }
            Err(crossbeam::channel::TryRecvError::Empty) => {
                // No stop signal received, and at least one sender is still active.
                false
            }
        }
    }

    /// Block until stop is signaled.
    ///
    /// Note that this will also return in case all senders have been dropped, meaning no stop
    /// signal will ever be sent.  The return value of `true` indicates a stop signal was received,
    /// and `false` indicates that all senders are dropped.
    pub(crate) fn wait_for_stop(&self) -> bool {
        match self.receiver.recv() {
            Ok(()) => {
                debug!(worker = self.worker, "Stop signal received");
                true
            }
            Err(crossbeam::channel::RecvError) => {
                debug!(
                    worker = self.worker,
                    "All stop signal senders have been dropped; treating this as a stop signal"
                );
                false
            }
        }
    }
}

/// Create a running output sender/receiver pair for a thread worker.
///
/// The correct bound for this channel is very dependent upon the work being performed and
/// therefore is a property of the worker itself
fn new_running_output_channel<T: ThreadWorkerWork>(
    bound: usize,
) -> (
    RunningOutputSender<T::RunningOutput>,
    RunningOutputReceiver<T::RunningOutput>,
) {
    let worker = T::worker_name();

    let (sender, receiver) = bounded(bound);
    (
        RunningOutputSender { sender, worker },
        RunningOutputReceiver { receiver, worker },
    )
}

#[derive(Debug)]
pub(crate) struct RunningOutputSender<T> {
    sender: Sender<T>,
    worker: &'static str,
}

/// Stupidly, the `Clone` derive macro assumes that `T` must also be Clone, but crossbeam
/// `Sender<T>`s are always clonable whether or not `T` is.
impl<T> Clone for RunningOutputSender<T> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            worker: self.worker,
        }
    }
}

impl<T> RunningOutputSender<T>
where
    T: Send + 'static,
{
    /// Send some running output from the worker to the receiver
    ///
    /// This will block if the channel is full.
    ///
    /// If the receiver has been dropped, then it's not possible to send any more running output.
    /// How the worker responds to this is up to the worker.
    ///
    /// It's entirely legitimate to just do this:
    ///
    /// ```rust,no_run
    /// # let (sender, receiver) = crossbeam::channel::bounded::<()>(0);
    /// # let sender = RunningOutputSender { sender, worker: "test" };
    /// // Send some output, and pay no attention to whether it failed or not
    /// let _ = sender.send(());
    ///
    /// // Alternatively, you can check the result and do something if it fails
    /// if sender.send(()).is_err() {
    ///   // Do something if the send failed, for example perhaps fail the worker
    ///   anyhow::bail!("No one listens to me anymore!");
    /// }
    ///
    /// // If for some reason you need to do something else with the output if
    /// // it was not able to be sent, then you can do this
    /// if let (Err(output)) = sender.send(()) {
    ///   // Do something with that `()` that you tried to send but failed
    /// }
    /// ```
    pub(crate) fn send(&self, output: T) -> Result<(), T> {
        match self.sender.send(output) {
            Ok(()) => Ok(()),
            Err(crossbeam::channel::SendError(output)) => {
                debug!(worker = self.worker, "Running output was not sent because all receivers are dropped so the channel is disconnected");
                Err(output)
            }
        }
    }

    /// Same as [`Self::send`], but does not block if the channel is full.
    ///
    /// This means there is an additional failure mode where the output could not be sent because
    /// the channel is full.  For this reason, this particular function leaks the underlying
    /// `crossbeam` impl in the form of the error that Crosbeam reports.
    pub(crate) fn try_send(&self, output: T) -> Result<(), crossbeam::channel::TrySendError<T>> {
        self.sender.try_send(output).inspect_err(|e| {
                match e {
                    crossbeam::channel::TrySendError::Disconnected(_) => {
                        debug!(worker = self.worker, "Running output was not sent because all receivers are dropped so the channel is disconnected");
                    }
                    crossbeam::channel::TrySendError::Full(_) => {
                        debug!(worker = self.worker, "Running output was not sent because the channel is full");
                    }
                }
        })
    }
}

#[derive(Debug)]
pub(crate) struct RunningOutputReceiver<T> {
    receiver: Receiver<T>,
    worker: &'static str,
}

/// Stupidly, the `Clone` derive macro assumes that `T` must also be Clone, but crossbeam
/// `Receiver<T>`s are always clonable whether or not `T` is.
impl<T> Clone for RunningOutputReceiver<T> {
    fn clone(&self) -> Self {
        Self {
            receiver: self.receiver.clone(),
            worker: self.worker,
        }
    }
}

impl<T> RunningOutputReceiver<T> {
    /// Receive a running output from the worker.
    ///
    /// This will block until an output is available.
    ///
    /// If the sender has been dropped, then this will return `None` and any subsequent calls to
    /// `recv` will also return `None`.
    pub(crate) fn recv(&self) -> Option<T> {
        match self.receiver.recv() {
            Ok(output) => Some(output),
            Err(crossbeam::channel::RecvError) => {
                debug!(
                    worker = self.worker,
                    "Running output receiver has been dropped"
                );
                None
            }
        }
    }

    /// Receive a running output from the worker without blocking.
    ///
    /// If an output is available, it is returned.  If not, `None` is returned.
    pub(crate) fn try_recv(&self) -> Option<T> {
        match self.receiver.try_recv() {
            Ok(output) => Some(output),
            Err(crossbeam::channel::TryRecvError::Empty) => None,
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                debug!(
                    worker = self.worker,
                    "Running output receiver has been dropped"
                );
                None
            }
        }
    }
}

/// Create a final output sender/receiver pair for a thread worker.
fn new_final_output_channel<T: ThreadWorkerWork>() -> (
    FinalOutputSender<T::FinalOutput>,
    FinalOutputReceiver<T::FinalOutput>,
) {
    let worker = T::worker_name();

    // The worker should be able to send the final result and exit, even if the receiver is not yet
    // ready to receive it.  That's why the bound is 1.
    let (sender, receiver) = bounded(1);
    (
        FinalOutputSender { sender, worker },
        FinalOutputReceiver { receiver, worker },
    )
}

#[derive(Debug)]
pub(crate) struct FinalOutputSender<T> {
    sender: Sender<Result<T>>,
    worker: &'static str,
}

impl<T> FinalOutputSender<T>
where
    T: Send + 'static,
{
    /// Send the final output from the worker to the receiver
    ///
    /// This will not block because the channel bound is 1 and there can only ever be one output
    /// sender.
    ///
    /// If the receiver has been dropped, then it's not possible to send the final output.
    /// The output is returned in the error case so the caller can handle it if needed.
    ///
    /// As with [`RunningOutputSender::send`], it's entirely legitimate to ignore the result of this
    /// operation if the worker's logic doesn't depend on whether or not its return value was ever
    /// received by the caller.
    pub(crate) fn send(self, output: Result<T>) -> Result<(), Result<T>> {
        match self.sender.send(output) {
            Ok(()) => Ok(()),
            Err(crossbeam::channel::SendError(output)) => {
                debug!(
                    worker = self.worker,
                    "Final output was not sent because receiver was dropped"
                );
                Err(output)
            }
        }
    }
}

#[derive(Debug)]
pub(crate) struct FinalOutputReceiver<T> {
    receiver: Receiver<Result<T>>,
    worker: &'static str,
}

impl<T> FinalOutputReceiver<T> {
    /// Receive the final output from the worker.
    ///
    /// This will block until the output is available.
    ///
    /// If the sender has been dropped without sending output, this will return None.
    pub(crate) fn recv(self) -> Option<Result<T>> {
        match self.receiver.recv() {
            Ok(output) => Some(output),
            Err(crossbeam::channel::RecvError) => {
                debug!(
                    worker = self.worker,
                    "Final output sender was dropped without sending output"
                );
                None
            }
        }
    }

    /// Try to receive the final output from the worker without blocking.
    ///
    /// Returns None if no output is available or if the sender was dropped.
    pub(crate) fn try_recv(self) -> Option<Result<T>> {
        match self.receiver.try_recv() {
            Ok(output) => Some(output),
            Err(crossbeam::channel::TryRecvError::Empty) => None,
            Err(crossbeam::channel::TryRecvError::Disconnected) => {
                debug!(
                    worker = self.worker,
                    "Final output sender was dropped without sending output"
                );
                None
            }
        }
    }
}

/// Trait implemented by a type that will actually perform some work in a thread.
///
/// By definition thist must be `Send` obviously.
pub(crate) trait ThreadWorkerInit: Send + 'static {
    /// The [`ThreadWorkerWork`] implementation that this initializer is initializing
    type Work: ThreadWorkerWork<RunningOutput = Self::RunningOutput>;

    /// The type that is produced as the work is being performed.
    /// In the actual progress channel this is *not* wrapped in a `Result`, so if your particualr
    /// implementation needs an ability to report a failure *and keep working*, this type will need
    /// to be explicitly `Result<T>` instead of `T`.
    type RunningOutput: Send + 'static;

    /// The bound to use for the channel containing the running output for this worker.
    fn running_output_channel_bound(&self) -> usize;

    fn init(
        self,
        running_output_sender: RunningOutputSender<Self::RunningOutput>,
    ) -> Result<Self::Work>;
}

/// Trait implemented by a type that will actually perform some work in a thread.
///
/// By definition thist must be `Send` obviously.
pub(crate) trait ThreadWorkerWork: Send + 'static {
    /// The type that is produced as the work is being performed.
    /// In the actual progress channel this is *not* wrapped in a `Result`, so if your particualr
    /// implementation needs an ability to report a failure *and keep working*, this type will need
    /// to be explicitly `Result<T>` instead of `T`.
    type RunningOutput: Send + 'static;

    /// The type that is produced when the work is done.
    ///
    /// Note that all thread workers are assumed to be falliable, so whatever type is specified
    /// here *is* wrapped in `Result`, unlike `RunningOutput` which is not.
    type FinalOutput: Send + 'static;

    /// The name of the worker for logging purposes.  This doesn't have any semantic meaning other
    /// than as a thing to use in the logs to indicate what worker goes with what log events.
    ///
    /// The default impl just uses the type name of the worker
    fn worker_name() -> &'static str {
        std::any::type_name::<Self>()
    }

    /// Run whatever the task is.
    ///
    /// The implementation can do anything, but it should frequently poll the stop signal to see if
    /// it should stop, and if so immediately return whatever final output makes sense for the
    /// specific task.
    fn run(
        self,
        stop_signal: StopSignalReceiver,
        running_output_sender: RunningOutputSender<Self::RunningOutput>,
    ) -> Result<Self::FinalOutput>;
}

/// Launch a new thread and run a given thread worker in that thread.
///
/// The returned tuple contains all that the caller needs to interact with the thread worker:
/// - `stop_signal_sender` to signal to the thread worker that it should stop what it's doing and
///   exit
/// - `running_output_receiver` to receive the running output from the worker as it runs.
/// - `final_output_receiver` to receive the final output (or error) from the worker when it's done.
pub(crate) fn start_thread_worker<T: ThreadWorkerInit>(
    init: T,
) -> (
    StopSignalSender,
    RunningOutputReceiver<T::RunningOutput>,
    FinalOutputReceiver<<T::Work as ThreadWorkerWork>::FinalOutput>,
) {
    let (stop_sender, stop_receiver) = new_stop_signal::<T::Work>();
    let (running_output_sender, running_output_receiver) =
        new_running_output_channel::<T::Work>(init.running_output_channel_bound());
    let (final_output_sender, final_output_receiver) = new_final_output_channel::<T::Work>();

    let current_span = Span::current();
    let _handle = thread::spawn(move || {
        // Propagate the span from the caller into this thread too
        let span =
            debug_span!(parent: &current_span, "thread_worker", worker = T::Work::worker_name());
        let _guard = span.enter();
        debug!("Starting worker thread");

        match init.init(running_output_sender.clone()) {
            Err(e) => {
                error!(error = %e, "Failed to initialize worker");
                let _ = final_output_sender.send(Err(e));
            }
            Ok(work) => match work.run(stop_receiver, running_output_sender) {
                Err(e) => {
                    error!(error = %e, "Worker failed");
                    let _ = final_output_sender.send(Err(e));
                }
                Ok(output) => {
                    debug!("Worker completed successfully");
                    let _ = final_output_sender.send(Ok(output));
                }
            },
        }

        debug!("Worker thread exiting");
    });

    (stop_sender, running_output_receiver, final_output_receiver)
}

#[cfg(test)]
mod test {}
