mod core;
pub(crate) use self::core::Header;

mod error;
#[allow(unreachable_pub)] // https://github.com/rust-lang/rust/issues/57411
pub use self::error::JoinError;

mod harness;

mod join;
#[cfg(any(feature = "rt-current-thread", feature = "rt-full"))]
#[allow(unreachable_pub)] // https://github.com/rust-lang/rust/issues/57411
pub use self::join::JoinHandle;

mod list;
pub(crate) use self::list::OwnedList;

mod raw;

mod stack;
pub(crate) use self::stack::TransferStack;

mod state;
mod waker;

/// Unit tests
#[cfg(test)]
mod tests;

use self::raw::RawTask;

use std::future::Future;
use std::marker::PhantomData;
use std::ptr::NonNull;
use std::{fmt, mem};

/// An owned handle to the task, tracked by ref count
pub(crate) struct Task<S: 'static, M = SendMarker> {
    raw: RawTask,
    _p: PhantomData<(S, M)>,
}

/// An owned handle to a `!Send` task, tracked by ref count.
#[cfg(feature = "local")]
pub(crate) type UnsendTask<S> = Task<S, UnsendMarker>;

/// Marker type indicating that a `Task` was constructed from a future that
/// implements `Send`.
#[derive(Debug)]
pub(crate) struct SendMarker {}

/// Marker type indicating that a `Task` was constructed from a future that
/// does not implement `Send`, and may only be scheduled by a scheduler that is
/// capable of scheduling `!Send` tasks.
#[derive(Debug)]
#[cfg(feature = "local")]
pub(crate) struct UnsendMarker {}

unsafe impl<S: Send + Sync + 'static> Send for Task<S, SendMarker> {}

/// Task result sent back
pub(crate) type Result<T> = std::result::Result<T, JoinError>;

pub(crate) trait Schedule<M>: Send + Sync + Sized + 'static {
    /// Bind a task to the executor.
    ///
    /// Guaranteed to be called from the thread that called `poll` on the task.
    fn bind(&self, task: &Task<Self, M>);

    /// The task has completed work and is ready to be released. The scheduler
    /// is free to drop it whenever.
    fn release(&self, task: Task<Self, M>);

    /// The has been completed by the executor it was bound to.
    fn release_local(&self, task: &Task<Self, M>);

    /// Schedule the task
    fn schedule(&self, task: Task<Self, M>);
}

/// Create a new task without an associated join handle
pub(crate) fn background<T, S>(task: T) -> Task<S>
where
    T: Future + Send + 'static,
    S: Schedule<SendMarker>,
{
    Task {
        raw: RawTask::new_background::<_, S>(task),
        _p: PhantomData,
    }
}

/// Create a new task with an associated join handle
pub(crate) fn joinable<T, S>(task: T) -> (Task<S>, JoinHandle<T::Output>)
where
    T: Future + Send + 'static,
    S: Schedule<SendMarker>,
{
    let raw = RawTask::new_joinable::<_, S>(task);

    let task = Task {
        raw,
        _p: PhantomData,
    };

    let join = JoinHandle::new(raw);

    (task, join)
}

/// Create a new `!Send` task with an associated join handle
#[cfg(feature = "local")]
pub(crate) fn joinable_unsend<T, S>(task: T) -> (UnsendTask<S>, JoinHandle<T::Output>)
where
    T: Future + 'static,
    S: Schedule<UnsendMarker>,
{
    let raw = RawTask::new_joinable_unsend::<_, S>(task);

    let task = Task {
        raw,
        _p: PhantomData,
    };

    let join = JoinHandle::new(raw);

    (task, join)
}

impl<S: 'static, M> Task<S, M> {
    pub(crate) unsafe fn from_raw(ptr: NonNull<Header>) -> Task<S, M> {
        Task {
            raw: RawTask::from_raw(ptr),
            _p: PhantomData,
        }
    }

    pub(crate) fn header(&self) -> &Header {
        self.raw.header()
    }

    pub(crate) fn into_raw(self) -> NonNull<Header> {
        let raw = self.raw.into_raw();
        mem::forget(self);
        raw
    }
}

impl<S: Schedule<M>, M> Task<S, M> {
    /// Returns `self` when the task needs to be immediately re-scheduled
    pub(crate) fn run<F>(self, mut executor: F) -> Option<Self>
    where
        F: FnMut() -> Option<NonNull<S>>,
    {
        if unsafe {
            self.raw
                .poll(&mut || executor().map(|ptr| ptr.cast::<()>()))
        } {
            Some(self)
        } else {
            // Cleaning up the `Task` instance is done from within the poll
            // function.
            mem::forget(self);
            None
        }
    }

    /// Pre-emptively cancel the task as part of the shutdown process.
    pub(crate) fn shutdown(self) {
        self.raw.cancel_from_queue();
        mem::forget(self);
    }
}

impl<S: 'static, M> Drop for Task<S, M> {
    fn drop(&mut self) {
        self.raw.drop_task();
    }
}

impl<S, M> fmt::Debug for Task<S, M> {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt.debug_struct("Task")
            .field("send", &format_args!("{}", std::any::type_name::<M>()))
            .finish()
    }
}
