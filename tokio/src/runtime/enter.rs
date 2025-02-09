use std::cell::{Cell, RefCell};
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::marker::PhantomData;

thread_local!(static ENTERED: Cell<bool> = Cell::new(false));

/// Represents an executor context.
///
/// For more details, see [`enter` documentation](fn.enter.html)
pub(crate) struct Enter {
    _p: PhantomData<RefCell<()>>,
}

/// An error returned by `enter` if an execution scope has already been
/// entered.
pub(crate) struct EnterError {
    _a: (),
}

impl fmt::Debug for EnterError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("EnterError")
            .field("reason", &format!("{}", self))
            .finish()
    }
}

impl fmt::Display for EnterError {
    fn fmt(&self, fmt: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            fmt,
            "attempted to run an executor while another executor is already running"
        )
    }
}

impl Error for EnterError {}

/// Marks the current thread as being within the dynamic extent of an
/// executor.
///
/// Executor implementations should call this function before blocking the
/// thread. If `None` is returned, the executor should fail by panicking or
/// taking some other action without blocking the current thread. This prevents
/// deadlocks due to multiple executors competing for the same thread.
///
/// # Error
///
/// Returns an error if the current thread is already marked
pub(crate) fn enter() -> Result<Enter, EnterError> {
    ENTERED.with(|c| {
        if c.get() {
            Err(EnterError { _a: () })
        } else {
            c.set(true);

            Ok(Enter { _p: PhantomData })
        }
    })
}

// Forces the current "entered" state to be cleared while the closure
// is executed.
//
// # Warning
//
// This is hidden for a reason. Do not use without fully understanding
// executors. Misuing can easily cause your program to deadlock.
#[cfg(feature = "rt-full")]
pub(crate) fn exit<F: FnOnce() -> R, R>(f: F) -> R {
    // Reset in case the closure panics
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            ENTERED.with(|c| {
                c.set(true);
            });
        }
    }

    ENTERED.with(|c| {
        debug_assert!(c.get());
        c.set(false);
    });

    let reset = Reset;
    let ret = f();
    ::std::mem::forget(reset);

    ENTERED.with(|c| {
        assert!(!c.get(), "closure claimed permanent executor");
        c.set(true);
    });

    ret
}

impl Enter {
    /// Blocks the thread on the specified future, returning the value with
    /// which that future completes.
    pub(crate) fn block_on<F: Future>(&mut self, mut f: F) -> F::Output {
        use crate::runtime::park::{CachedParkThread, Park};
        use std::pin::Pin;
        use std::task::Context;
        use std::task::Poll::Ready;

        let mut park = CachedParkThread::new();
        let waker = park.unpark().into_waker();
        let mut cx = Context::from_waker(&waker);

        // `block_on` takes ownership of `f`. Once it is pinned here, the original `f` binding can
        // no longer be accessed, making the pinning safe.
        let mut f = unsafe { Pin::new_unchecked(&mut f) };

        loop {
            if let Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            park.park().unwrap();
        }
    }
}

impl fmt::Debug for Enter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Enter").finish()
    }
}

impl Drop for Enter {
    fn drop(&mut self) {
        ENTERED.with(|c| {
            assert!(c.get());
            c.set(false);
        });
    }
}
