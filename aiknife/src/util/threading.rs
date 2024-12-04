//! Some utility code for cases when some work needs to be done in a separate thread, which can be
//! aborted, can report an error, and produces a stream of work product that can be consumed by the
//! caller of this work in another thread.
//!
//! The basic pattern here is to use crossbeam channels in a few ways:
//! - A bounded(1) channel of type `()` is used to signal to the thread that it should stop its
//!   work.
//! - A bounded(1) channel of type `Result<T>` is used indicate the success or failure of the
//!   the work that's done in another thread, the successful completion of which produces some type
//!   `T`.
//! - A higher-bounded channel of some type `T` is used to produce ongoing results of the work
//! (this can be progress updates, or it can be packets read a bit at a time from a media file).
//!
//! # Thread Workers
//!
//! The bit of work that needs to happen in another thread is generalized into the concept of a
//! "thread worker".  Not much is assumed about this work, other than that it's blocking (no async
//! runtime is provided), it's fallible, and it produces a stream of work product while it runs.
//!
//! To keep things reasonably simple, a trait is used to describe the work that needs to be done in
//! the running step.  Unfortunately that means that the implementation of the thread worker isn't
//! just a simple function but needs a type to hang the trait impl on.  This isn't ideal but this
//! entire module is a yak-shaving exercise to indulge my need for clean abstractions, and I'm not
//! about to waste days getting all of the short and curly hairs off of this yak.  Actually maybe I
//! am, but I'm trying very hard not to.

use anyhow::Result;
use crossbeam::channel::{bounded, Receiver, Sender};
use std::thread;
use tracing::*;

/// Create a stop signal sender/receiver pair for a thread worker.
fn new_stop_signal<T: ThreadWorker>(
    worker: &'static str,
) -> (StopSignalSender, StopSignalReceiver) {
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
fn new_running_output_channel<T: ThreadWorker>(
    bound: usize,
    worker: &'static str,
) -> (
    RunningOutputSender<T::RunningOutput>,
    RunningOutputReceiver<T::RunningOutput>,
) {
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
fn new_final_output_channel<T: ThreadWorker>(
    worker: &'static str,
) -> (
    FinalOutputSender<T::FinalOutput>,
    FinalOutputReceiver<T::FinalOutput>,
) {
    // The worker should be able to send the final result and exit, even if the receiver is not yet
    // ready to receive it.  That's why the bound is 1.
    let (sender, receiver) = bounded(1);
    (
        FinalOutputSender { sender, worker },
        FinalOutputReceiver { receiver, worker },
    )
}

#[derive(Debug)]
struct FinalOutputSender<T> {
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
struct FinalOutputReceiver<T> {
    receiver: Receiver<Result<T>>,
    worker: &'static str,
}

impl<T> FinalOutputReceiver<T> {
    /// Receive the final output from the worker.
    ///
    /// This will block until the output is available.
    ///
    /// If the sender has been dropped without sending output, this will return None.
    fn recv(self) -> Option<Result<T>> {
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
}

/// A handle to a thread worker, by which it is possible to get the final output from the worker
/// and also to join the worker thread and thereby detect potential panics
#[derive(Debug)]
pub(crate) struct ThreadWorkerHandle<T> {
    final_output: FinalOutputReceiver<T>,
    join_handle: thread::JoinHandle<()>,
}

impl<T> ThreadWorkerHandle<T> {
    /// Create a new thread worker handle from the final output receiver and the join handle.
    pub(crate) fn new(
        final_output: FinalOutputReceiver<T>,
        join_handle: thread::JoinHandle<()>,
    ) -> Self {
        Self {
            final_output,
            join_handle,
        }
    }

    /// Check if the thread worker is finished.
    ///
    /// If this is `true`, then `join` is guaranteed to return immediately without blocking.
    pub(crate) fn is_finished(&self) -> bool {
        self.join_handle.is_finished()
    }

    /// Wait for the worker to complete and return the final output.
    ///
    /// # Notes
    ///
    /// If the thread panics, there is a special case here.  Regardless of whether or not a final
    /// output was written to the final output channel, the panic will be propagated to the caller.
    /// This reflects the seriousness of a panic in a worker thread and ensures that bugs which
    /// cause threads to panic are not silently ignored.
    pub(crate) fn join(self) -> Result<T> {
        // Make sure that the thread has completed before we try to get the final output
        if let Err(e) = self.join_handle.join() {
            error!(worker = self.final_output.worker, "Worker thread panicked");
            std::panic::resume_unwind(e);
        }

        // Now that the thread has completed, we can get the final output
        // Note that at this point, we know that the thread did not panic.  The body of the thread
        // is in this module in `start_thread_worker`, and we can clearly see that there is no way
        // that this thread can return without writing *something* to the final output channel.
        // Thus, we are certain that there will be output here, and if there isn't it's due to some
        // very fundamental bug somewhere.
        self.final_output
            .recv()
            .expect("BUG: Worker thread did not produce final output")
    }
}

/// A receiver for the case when a thread worker produces some fallible running output (meaning
/// wrappedin `Result`) and the final output is `Result<()>`.
///
/// This struct presents a convenience whereby the running output channel is joined with the thread
/// handle, such that once the running output channel is empty (or the sender dropped), the final
/// channel is queried.  If the final output is `Ok`, that response is translated into a `None`
/// value signalling that there will be no more output, while if it is an `Err` then that error is
/// returned.
#[derive(Debug)]
pub(crate) struct CombinedOutputReceiver<T> {
    running_output: RunningOutputReceiver<Result<T>>,
    thread_worker_handle: Option<ThreadWorkerHandle<()>>,
}

impl<T> CombinedOutputReceiver<T> {
    /// Join together the running output and final output channels for a thread worker into a
    /// single unified channel.
    pub(crate) fn join(
        running_output: RunningOutputReceiver<Result<T>>,
        thread_worker_handle: ThreadWorkerHandle<()>,
    ) -> Self {
        Self {
            running_output,
            thread_worker_handle: Some(thread_worker_handle),
        }
    }

    pub(crate) fn recv(&mut self) -> Option<Result<T>> {
        if let Some(output) = self.running_output.recv() {
            return Some(output);
        }

        if let Some(thread_worker_handle) = self.thread_worker_handle.take() {
            match thread_worker_handle.join() {
                Ok(()) => return None,
                Err(e) => return Some(Err(e)),
            }
        }

        // If both channels are empty, then we're done
        None
    }
}

impl<T> Iterator for CombinedOutputReceiver<T> {
    type Item = Result<T>;

    fn next(&mut self) -> Option<Self::Item> {
        self.recv()
    }
}

/// Trait implemented by a type that will actually perform some work in a thread.
///
/// By definition thist must be `Send` obviously.
pub(crate) trait ThreadWorker: Send + 'static {
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

    /// The bound to use for the channel containing the running output for this worker.
    fn running_output_channel_bound(&self) -> usize;

    /// The name of the worker for logging purposes.  This doesn't have any semantic meaning other
    /// than as a thing to use in the logs to indicate what worker goes with what log events.
    ///
    /// The default impl just uses the type name of the worker
    fn name(&self) -> &'static str {
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
pub(crate) fn start_thread_worker<T: ThreadWorker>(
    worker: T,
) -> (
    StopSignalSender,
    RunningOutputReceiver<T::RunningOutput>,
    ThreadWorkerHandle<T::FinalOutput>,
) {
    let worker_name = worker.name();
    let (stop_sender, stop_receiver) = new_stop_signal::<T>(worker.name());
    let (running_output_sender, running_output_receiver) =
        new_running_output_channel::<T>(worker.running_output_channel_bound(), worker.name());
    let (final_output_sender, final_output_receiver) = new_final_output_channel::<T>(worker.name());

    let current_span = Span::current();
    let thread_func = move || {
        // Propagate the span from the caller into this thread too
        let span = debug_span!(parent: &current_span, "thread_worker", worker = worker.name());
        let _guard = span.enter();
        debug!("Starting worker thread");

        match worker.run(stop_receiver, running_output_sender) {
            Err(e) => {
                error!(error = %e, "Worker failed");
                let _ = final_output_sender.send(Err(e));
            }
            Ok(output) => {
                debug!("Worker completed successfully");
                let _ = final_output_sender.send(Ok(output));
            }
        }

        debug!("Worker thread exiting");
    };

    // Spawn a new thread for this worker
    // NOTE: the `expect` means we will panic if `spawn` fails.  In fact this is the behavior of
    // `std::thread::spawn` as well, you just may not have been aware of it.  If thread spawning
    // is failling the system is in a very bad state anyway, so getting upset over a panic is
    // likely to be the least of your worries.
    let join_handle = thread::Builder::new()
        .name(format!("worker-{worker_name}"))
        .spawn(thread_func)
        .expect("Failed to spawn worker thread");

    (
        stop_sender,
        running_output_receiver,
        ThreadWorkerHandle::new(final_output_receiver, join_handle),
    )
}

/// A variation on [`start_thread_worker`] that takes a closure as the worker instead of an
/// explicit [`ThreadWorker`] implementation.
///
/// See [`start_thread_worker`] for more details.
pub(crate) fn start_func_as_thread_worker<RunningOutput, FinalOutput, Func>(
    worker_name: &'static str,
    running_output_channel_bound: usize,
    func: Func,
) -> (
    StopSignalSender,
    RunningOutputReceiver<RunningOutput>,
    ThreadWorkerHandle<FinalOutput>,
)
where
    RunningOutput: Send + 'static,
    FinalOutput: Send + 'static,
    Func: FnOnce(StopSignalReceiver, RunningOutputSender<RunningOutput>) -> Result<FinalOutput>
        + Send
        + 'static,
{
    let worker = FuncThreadWorker {
        worker_name,
        running_output_channel_bound,
        func,
        _phantom: std::marker::PhantomData,
    };

    start_thread_worker(worker)
}

/// A [`ThreadWorker`] implementation that runs an arbitrary closure as the work to do.
struct FuncThreadWorker<RunningOutput, FinalOutput, Func> {
    worker_name: &'static str,
    running_output_channel_bound: usize,
    func: Func,
    _phantom: std::marker::PhantomData<(RunningOutput, FinalOutput)>,
}

impl<RunningOutput, FinalOutput, Func> ThreadWorker
    for FuncThreadWorker<RunningOutput, FinalOutput, Func>
where
    RunningOutput: Send + 'static,
    FinalOutput: Send + 'static,
    Func: FnOnce(StopSignalReceiver, RunningOutputSender<RunningOutput>) -> Result<FinalOutput>
        + Send
        + 'static,
{
    type RunningOutput = RunningOutput;
    type FinalOutput = FinalOutput;

    fn running_output_channel_bound(&self) -> usize {
        self.running_output_channel_bound
    }

    fn name(&self) -> &'static str {
        self.worker_name
    }

    fn run(
        self,
        stop_signal: StopSignalReceiver,
        running_output: RunningOutputSender<Self::RunningOutput>,
    ) -> Result<Self::FinalOutput> {
        (self.func)(stop_signal, running_output)
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::panic::AssertUnwindSafe;
    use std::time::Duration;

    /// A simple test worker that counts up to a number
    struct TestWorker {
        count_to: u32,
        delay_ms: u64,
    }

    impl ThreadWorker for TestWorker {
        type RunningOutput = u32;
        type FinalOutput = u32;

        fn name(&self) -> &'static str {
            "test_worker"
        }

        fn running_output_channel_bound(&self) -> usize {
            5
        }

        fn run(
            self,
            stop_signal: StopSignalReceiver,
            running_output: RunningOutputSender<Self::RunningOutput>,
        ) -> Result<Self::FinalOutput> {
            let mut current = 0;
            while current < self.count_to {
                if stop_signal.is_stop_signaled() {
                    return Ok(current);
                }
                current += 1;
                let _ = running_output.send(current);
                if self.delay_ms > 0 {
                    thread::sleep(Duration::from_millis(self.delay_ms));
                }
            }
            Ok(current + 1)
        }
    }

    #[test]
    fn test_stop_signal() {
        crate::test_helpers::init_test_logging();

        let (sender, receiver) = new_stop_signal::<TestWorker>("test");
        assert!(!receiver.is_stop_signaled());

        sender.signal_stop();
        assert!(receiver.is_stop_signaled());
    }

    #[test]
    fn test_stop_signal_drop() {
        crate::test_helpers::init_test_logging();

        let (sender, receiver) = new_stop_signal::<TestWorker>("test");
        assert!(!receiver.is_stop_signaled());

        drop(sender);
        assert!(receiver.is_stop_signaled());
    }

    #[test]
    fn test_running_output_channel() {
        crate::test_helpers::init_test_logging();

        let (sender, receiver) = new_running_output_channel::<TestWorker>(2, "test");

        assert!(sender.send(1).is_ok());
        assert!(sender.send(2).is_ok());

        assert_eq!(receiver.recv(), Some(1));
        assert_eq!(receiver.recv(), Some(2));
    }

    #[test]
    fn test_running_output_channel_full() {
        crate::test_helpers::init_test_logging();

        let (sender, receiver) = new_running_output_channel::<TestWorker>(1, "test");

        assert!(sender.send(1).is_ok());
        assert!(sender.try_send(2).is_err());

        assert_eq!(receiver.recv(), Some(1));
    }

    #[test]
    fn test_final_output_channel() {
        crate::test_helpers::init_test_logging();

        let (sender, receiver) = new_final_output_channel::<TestWorker>("test");

        sender.send(Ok(42)).unwrap();
        match receiver.recv().unwrap() {
            Ok(val) => assert_eq!(val, 42),
            Err(e) => panic!("Expected Ok(42), got Err: {}", e),
        }
    }

    #[test]
    fn test_complete_worker_lifecycle() {
        crate::test_helpers::init_test_logging();

        let worker = TestWorker {
            count_to: 5,
            delay_ms: 100,
        };

        let (_stop_sender, running_receiver, thread_worker_handle) = start_thread_worker(worker);

        let mut received_outputs = Vec::new();
        let mut done = false;
        while let Some(output) = running_receiver.recv() {
            assert!(!done, "received more outputs than expected!: {output}");
            received_outputs.push(output);
            if received_outputs.len() == 5 {
                done = true;
            }
        }

        let final_result = thread_worker_handle.join().unwrap();
        assert_eq!(final_result, 6);
        assert_eq!(received_outputs, vec![1, 2, 3, 4, 5]);
    }

    #[test]
    fn test_worker_early_stop() {
        crate::test_helpers::init_test_logging();

        let worker = TestWorker {
            count_to: 100,
            delay_ms: 10,
        };

        let (stop_sender, _running_receiver, thread_worker_handle) = start_thread_worker(worker);

        // Let it run for a bit
        thread::sleep(Duration::from_millis(50));

        // Stop it
        stop_sender.signal_stop();

        let final_result = thread_worker_handle.join().unwrap();
        assert!(final_result > 0 && final_result < 100);
    }

    // Test worker that fails during initialization
    struct FailingInitWorker;

    impl ThreadWorker for FailingInitWorker {
        type RunningOutput = u32;
        type FinalOutput = u32;

        fn name(&self) -> &'static str {
            "fragile_worker"
        }

        fn running_output_channel_bound(&self) -> usize {
            1
        }

        fn run(
            self,
            _stop_signal: StopSignalReceiver,
            _running_output: RunningOutputSender<Self::RunningOutput>,
        ) -> Result<Self::FinalOutput> {
            anyhow::bail!("Initialization failed")
        }
    }

    #[test]
    fn test_worker_failure() {
        crate::test_helpers::init_test_logging();

        let init = FailingInitWorker;
        let (_stop_sender, _running_receiver, thread_worker_handle) = start_thread_worker(init);

        let result = thread_worker_handle.join();
        assert!(result.is_err());
    }

    #[test]
    fn test_func_worker_basic() {
        crate::test_helpers::init_test_logging();

        let (_stop_sender, running_receiver, thread_worker_handle) =
            start_func_as_thread_worker("counter", 5, |stop_signal, running_output| {
                let mut count = 0;
                while count < 5 {
                    if stop_signal.is_stop_signaled() {
                        return Ok(count);
                    }
                    count += 1;
                    let _ = running_output.send(count);
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(count)
            });

        let mut received = Vec::new();
        while let Some(output) = running_receiver.recv() {
            received.push(output);
        }

        assert_eq!(received, vec![1, 2, 3, 4, 5]);
        assert_eq!(thread_worker_handle.join().unwrap(), 5);
    }

    #[test]
    fn test_func_worker_early_stop() {
        crate::test_helpers::init_test_logging();

        let (stop_sender, _running_receiver, thread_worker_handle) =
            start_func_as_thread_worker("infinite_counter", 5, |stop_signal, running_output| {
                let mut count = 0;
                loop {
                    if stop_signal.is_stop_signaled() {
                        return Ok(count);
                    }
                    count += 1;
                    let _ = running_output.send(count);
                    thread::sleep(Duration::from_millis(10));
                }
            });

        // Let it run for a bit
        thread::sleep(Duration::from_millis(50));
        stop_sender.signal_stop();

        let final_count = thread_worker_handle.join().unwrap();
        assert!(
            final_count > 0,
            "Worker should have counted up before stopping"
        );
        assert!(
            final_count < 100,
            "Worker should have stopped before counting too high"
        );
    }

    #[test]
    fn test_func_worker_error() {
        crate::test_helpers::init_test_logging();

        let (_stop_sender, running_receiver, thread_worker_handle) =
            start_func_as_thread_worker("failing_counter", 5, |stop_signal, running_output| {
                for i in 1..=3 {
                    if stop_signal.is_stop_signaled() {
                        return Ok(i);
                    }
                    let _ = running_output.send(i);
                    thread::sleep(Duration::from_millis(10));
                }
                anyhow::bail!("Simulated failure")
            });

        let mut received = Vec::new();
        while let Some(output) = running_receiver.recv() {
            received.push(output);
        }

        assert_eq!(received, vec![1, 2, 3]);
        assert!(thread_worker_handle.join().is_err());
    }

    #[test]
    fn test_func_worker_channel_bounds() {
        crate::test_helpers::init_test_logging();

        let (_stop_sender, running_receiver, thread_worker_handle) = start_func_as_thread_worker(
            "fast_counter",
            2, // Small channel bound
            |stop_signal, running_output| {
                for i in 1..=5 {
                    if stop_signal.is_stop_signaled() {
                        return Ok(i);
                    }
                    // Use try_send to test channel bounds
                    match running_output.try_send(i) {
                        Ok(_) => (),
                        Err(crossbeam::channel::TrySendError::Full(_)) => {
                            // Expected when channel is full
                            thread::sleep(Duration::from_millis(10));
                        }
                        Err(e) => anyhow::bail!("Unexpected error: {}", e),
                    }
                }
                Ok(5)
            },
        );

        // Sleep briefly to let the worker fill the channel
        thread::sleep(Duration::from_millis(50));

        let mut received = Vec::new();
        while let Some(output) = running_receiver.try_recv() {
            received.push(output);
        }

        assert!(!received.is_empty(), "Should have received some values");
        assert!(received.len() <= 5, "Should not receive more than 5 values");
        assert_eq!(thread_worker_handle.join().unwrap(), 5);
    }

    #[test]
    fn test_combined_output_receiver_success() {
        crate::test_helpers::init_test_logging();

        let (_stop_sender, running_receiver, thread_worker_handle) =
            start_func_as_thread_worker("success_worker", 5, |stop_signal, running_output| {
                for i in 1..=3 {
                    if stop_signal.is_stop_signaled() {
                        return Ok(());
                    }
                    let _ = running_output.send(Ok(i));
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            });

        let mut combined = CombinedOutputReceiver::join(running_receiver, thread_worker_handle);

        // Test iterator implementation
        let results: Vec<_> = combined.collect();
        assert_eq!(results.len(), 3);
        for (i, result) in results.iter().enumerate() {
            match result {
                Ok(val) => assert_eq!(*val, i as i32 + 1),
                Err(_) => panic!("Expected Ok value at index {}", i),
            }
        }
    }

    #[test]
    fn test_combined_output_receiver_error_in_running() {
        crate::test_helpers::init_test_logging();

        let (_stop_sender, running_receiver, thread_worker_handle) = start_func_as_thread_worker(
            "error_in_running_worker",
            5,
            |stop_signal, running_output| {
                for i in 1..=3 {
                    if stop_signal.is_stop_signaled() {
                        return Ok(());
                    }
                    let result = if i == 2 {
                        Err(anyhow::anyhow!("Error at {}", i))
                    } else {
                        Ok(i)
                    };
                    let _ = running_output.send(result);
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            },
        );

        let mut combined = CombinedOutputReceiver::join(running_receiver, thread_worker_handle);

        // First value should be Ok(1)
        match combined.recv() {
            Some(Ok(1)) => (),
            other => panic!("Expected Some(Ok(1)), got {:?}", other),
        }

        // Second value should be an error
        match combined.recv() {
            Some(Err(_)) => (),
            other => panic!("Expected Some(Err(_)), got {:?}", other),
        }

        // Third value should be Ok(3)
        match combined.recv() {
            Some(Ok(3)) => (),
            other => panic!("Expected Some(Ok(3)), got {:?}", other),
        }

        // Should end with None after successful completion
        assert!(combined.recv().is_none());
    }

    #[test]
    fn test_combined_output_receiver_error_in_final() {
        crate::test_helpers::init_test_logging();

        let (_stop_sender, running_receiver, thread_worker_handle) = start_func_as_thread_worker(
            "error_in_final_worker",
            5,
            |stop_signal, running_output| {
                for i in 1..=2 {
                    if stop_signal.is_stop_signaled() {
                        return Ok(());
                    }
                    let _ = running_output.send(Ok(i));
                    thread::sleep(Duration::from_millis(10));
                }
                Err(anyhow::anyhow!("Final error"))
            },
        );

        let mut combined = CombinedOutputReceiver::join(running_receiver, thread_worker_handle);

        // First value should be Ok(1)
        match combined.recv() {
            Some(Ok(1)) => (),
            other => panic!("Expected Some(Ok(1)), got {:?}", other),
        }

        // Second value should be Ok(2)
        match combined.recv() {
            Some(Ok(2)) => (),
            other => panic!("Expected Some(Ok(2)), got {:?}", other),
        }

        // Should end with the final error
        match combined.recv() {
            Some(Err(_)) => (),
            other => panic!("Expected Some(Err(_)), got {:?}", other),
        }
    }

    #[test]
    fn test_combined_output_receiver_early_stop() {
        crate::test_helpers::init_test_logging();

        let (stop_sender, running_receiver, thread_worker_handle) =
            start_func_as_thread_worker("stopped_worker", 5, |stop_signal, running_output| {
                for i in 1..=5 {
                    if stop_signal.is_stop_signaled() {
                        return Ok(());
                    }
                    let _ = running_output.send(Ok(i));
                    thread::sleep(Duration::from_millis(50));
                }
                Ok(())
            });

        let mut combined = CombinedOutputReceiver::join(running_receiver, thread_worker_handle);

        // First value should be Ok(1)
        match combined.recv() {
            Some(Ok(1)) => (),
            other => panic!("Expected Some(Ok(1)), got {:?}", other),
        }

        // Stop the worker after first output
        stop_sender.signal_stop();

        // Should receive None after the worker stops
        assert!(combined.recv().is_none());
    }

    // A worker that panics during execution
    struct PanickingWorker {
        panic_after: u32,
    }

    impl ThreadWorker for PanickingWorker {
        type RunningOutput = u32;
        type FinalOutput = u32;

        fn name(&self) -> &'static str {
            "panicking_worker"
        }

        fn running_output_channel_bound(&self) -> usize {
            5
        }

        fn run(
            self,
            stop_signal: StopSignalReceiver,
            running_output: RunningOutputSender<Self::RunningOutput>,
        ) -> Result<Self::FinalOutput> {
            let mut count = 0;
            while count < self.panic_after {
                if stop_signal.is_stop_signaled() {
                    return Ok(count);
                }
                count += 1;
                let _ = running_output.send(count);
                thread::sleep(Duration::from_millis(10));
            }
            panic!("Worker panicked after {} iterations", count);
        }
    }

    #[test]
    #[should_panic(expected = "Worker panicked after 3 iterations")]
    fn test_worker_panic() {
        crate::test_helpers::init_test_logging();

        let worker = PanickingWorker { panic_after: 3 };
        let (_stop_sender, running_receiver, thread_worker_handle) = start_thread_worker(worker);

        // Collect outputs until panic
        let mut received = Vec::new();
        while let Some(output) = running_receiver.recv() {
            received.push(output);
        }

        // This should panic with our custom message
        thread_worker_handle.join().unwrap();
    }

    #[test]
    fn test_worker_panic_output_handling() {
        crate::test_helpers::init_test_logging();

        let worker = PanickingWorker { panic_after: 3 };
        let (_stop_sender, running_receiver, thread_worker_handle) = start_thread_worker(worker);

        // We should receive all outputs sent before the panic
        let mut received = Vec::new();
        while let Some(output) = running_receiver.recv() {
            received.push(output);
        }

        assert_eq!(received, vec![1, 2, 3]);

        // The join should result in a panic that we can catch
        let result =
            std::panic::catch_unwind(AssertUnwindSafe(|| thread_worker_handle.join().unwrap()));
        assert!(result.is_err());
    }

    #[test]
    fn test_combined_output_receiver_panic() {
        crate::test_helpers::init_test_logging();

        let (_stop_sender, running_receiver, thread_worker_handle) =
            start_func_as_thread_worker("panicking_worker", 5, |stop_signal, running_output| {
                for i in 1..=3 {
                    if stop_signal.is_stop_signaled() {
                        return Ok(());
                    }
                    let _ = running_output.send(Ok(i));
                    if i == 2 {
                        panic!("Planned panic after second output");
                    }
                    thread::sleep(Duration::from_millis(10));
                }
                Ok(())
            });

        let mut combined = CombinedOutputReceiver::join(running_receiver, thread_worker_handle);

        // First value should be Ok(1)
        match combined.recv() {
            Some(Ok(1)) => (),
            other => panic!("Expected Some(Ok(1)), got {:?}", other),
        }

        // Second value should be Ok(2)
        match combined.recv() {
            Some(Ok(2)) => (),
            other => panic!("Expected Some(Ok(2)), got {:?}", other),
        }

        // The next recv should result in a panic that we can catch
        let result = std::panic::catch_unwind(AssertUnwindSafe(|| combined.recv()));
        assert!(result.is_err());
    }
}
