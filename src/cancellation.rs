//! Cooperative cancellation across syntax, lowering, inference, and SAT work.
use std::{
    cell::RefCell,
    panic::AssertUnwindSafe,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

#[derive(Debug, Clone, Default)]
pub struct Cancellation(Arc<AtomicBool>);

thread_local! {
    static ACTIVE: RefCell<Option<Cancellation>> = const { RefCell::new(None) };
}

impl Cancellation {
    /// Share the current work's cancellation with an acquisition thread.
    pub fn current() -> Self {
        ACTIVE.with(|active| active.borrow().clone().unwrap_or_default())
    }

    /// Cancellation signal for blocking libraries that accept an atomic flag.
    pub fn signal(&self) -> &AtomicBool {
        &self.0
    }

    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }

    /// Run one revision's work. Cancellation abandons incomplete query results;
    /// other panics retain their original payload and are not hidden as cancellation.
    pub fn run<T>(&self, work: impl FnOnce() -> T) -> Result<T, salsa::Cancelled> {
        struct Restore(Option<Cancellation>);
        impl Drop for Restore {
            fn drop(&mut self) {
                ACTIVE.with(|active| *active.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(ACTIVE.with(|active| active.replace(Some(self.clone()))));
        salsa::Cancelled::catch(AssertUnwindSafe(|| {
            checkpoint();
            let result = work();
            checkpoint();
            result
        }))
    }
}

#[inline]
pub fn checkpoint() {
    if ACTIVE.with(|active| {
        active
            .borrow()
            .as_ref()
            .is_some_and(Cancellation::is_cancelled)
    }) {
        std::panic::resume_unwind(Box::new(salsa::Cancelled::Local));
    }
}
