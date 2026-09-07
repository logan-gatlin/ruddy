//! Cooperative cancellation across syntax, lowering, inference, and SAT work.
use std::{cell::RefCell, panic::AssertUnwindSafe};

#[derive(Debug, Clone, Default)]
pub struct Cancellation(salsa::CancellationToken);

thread_local! {
    static ACTIVE: RefCell<Option<Cancellation>> = const { RefCell::new(None) };
}

impl Cancellation {
    pub fn cancel(&self) {
        self.0.cancel();
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
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
