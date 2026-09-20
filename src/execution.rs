//! Bounded execution of compiler queries. Salsa owns query coordination; this
//! module owns scheduling. Disabling `parallel` removes the pool and Rayon.
use std::{cell::RefCell, num::NonZeroUsize};

use crate::cancellation;

#[cfg(feature = "parallel")]
use std::sync::{Arc, OnceLock};

thread_local! {
    static CURRENT: RefCell<Option<Execution>> = const { RefCell::new(None) };
}

/// A reusable execution policy, shared by any number of compiler sessions.
///
/// The default lazily uses one process-wide compiler pool with at most four
/// workers. This bounds simultaneous solver memory even when several editor or
/// debugger sessions compile at once. It does not initialize Rayon's global pool.
#[derive(Clone)]
pub struct Execution {
    backend: Backend,
}

#[derive(Clone)]
enum Backend {
    Sequential,
    #[cfg(feature = "parallel")]
    Shared,
    #[cfg(feature = "parallel")]
    Pool(Arc<rayon::ThreadPool>),
}

impl Default for Execution {
    fn default() -> Self {
        CURRENT.with(|current| {
            current.borrow().clone().unwrap_or(Self {
                #[cfg(feature = "parallel")]
                backend: Backend::Shared,
                #[cfg(not(feature = "parallel"))]
                backend: Backend::Sequential,
            })
        })
    }
}

impl Execution {
    /// Run directly on the caller, without creating any threads.
    pub fn sequential() -> Self {
        Self {
            backend: Backend::Sequential,
        }
    }

    /// Create a private, reusable pool. One worker selects direct sequential
    /// execution. Without `parallel`, larger requests return `Unsupported`.
    pub fn with_threads(threads: NonZeroUsize) -> std::io::Result<Self> {
        if threads.get() == 1 {
            return Ok(Self::sequential());
        }
        #[cfg(feature = "parallel")]
        {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads.get())
                .thread_name(|index| format!("ruddy-inference-{index}"))
                .build()
                .map(|pool| Self {
                    backend: Backend::Pool(Arc::new(pool)),
                })
                .map_err(std::io::Error::other)
        }
        #[cfg(not(feature = "parallel"))]
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "Ruddy was built without the parallel feature",
        ))
    }

    /// Select the policy for sessions constructed inside `work` on this thread.
    /// Existing sessions retain their policy. Nested calls and unwinding restore
    /// the previous policy. This also configures the ordinary compile entry points.
    pub fn run<T>(&self, work: impl FnOnce() -> T) -> T {
        struct Restore(Option<Execution>);
        impl Drop for Restore {
            fn drop(&mut self) {
                CURRENT.with(|current| *current.borrow_mut() = self.0.take());
            }
        }
        let _restore = Restore(CURRENT.with(|current| current.replace(Some(self.clone()))));
        work()
    }

    /// Evaluate independent jobs with a clone of worker-local state (such as a
    /// Salsa database). Results should be stored by the query engine, then read
    /// in stable order by the caller. No worker state survives this call.
    ///
    /// Workers inherit cooperative cancellation. Failure stops dispatch, cancels
    /// sibling work, and joins all jobs before resuming the original panic or
    /// cancellation on the caller. Ordinary panics take precedence over cancellation.
    pub fn for_each<C, T, F>(&self, mut context: C, items: &[T], work: F)
    where
        C: Clone + Send,
        T: Sync,
        F: Fn(&mut C, &T) + Sync,
    {
        match &self.backend {
            #[cfg(feature = "parallel")]
            Backend::Shared | Backend::Pool(_) if items.len() > 1 => {
                if let Some(pool) = self.pool() {
                    parallel(&pool, context, items, &work);
                    return;
                }
            }
            _ => {}
        }
        for item in items {
            cancellation::checkpoint();
            work(&mut context, item);
        }
        cancellation::checkpoint();
    }

    #[cfg(feature = "parallel")]
    fn pool(&self) -> Option<Arc<rayon::ThreadPool>> {
        match &self.backend {
            Backend::Sequential => None,
            Backend::Pool(pool) => Some(pool.clone()),
            Backend::Shared => {
                static POOL: OnceLock<Option<Arc<rayon::ThreadPool>>> = OnceLock::new();
                POOL.get_or_init(|| {
                    let threads = std::thread::available_parallelism()
                        .unwrap_or(NonZeroUsize::MIN)
                        .min(NonZeroUsize::new(4).unwrap());
                    // The default remains usable when threads cannot be created.
                    match Self::with_threads(threads).ok()?.backend {
                        Backend::Pool(pool) => Some(pool),
                        _ => None,
                    }
                })
                .clone()
            }
        }
    }
}

#[cfg(feature = "parallel")]
fn parallel<C, T, F>(pool: &rayon::ThreadPool, context: C, items: &[T], work: &F)
where
    C: Clone + Send,
    T: Sync,
    F: Fn(&mut C, &T) + Sync,
{
    use std::{
        any::Any,
        panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
    };
    let token = cancellation::Cancellation::current();
    let next = AtomicUsize::new(0);
    let failure: Mutex<Option<Box<dyn Any + Send>>> = Mutex::new(None);
    // Clone on the caller: Salsa databases are Send, but not Sync. Move each
    // clone into a worker rather than capturing a shared reference to one.
    let contexts: Vec<_> = (0..items.len().min(pool.current_num_threads()))
        .map(|_| context.clone())
        .collect();
    drop(context);
    pool.scope(|scope| {
        for mut context in contexts {
            let token = &token;
            let next = &next;
            let failure = &failure;
            scope.spawn(move |_| {
                let result = catch_unwind(AssertUnwindSafe(|| {
                    token.run(|| {
                        loop {
                            cancellation::checkpoint();
                            let index = next.fetch_add(1, Ordering::Relaxed);
                            let Some(item) = items.get(index) else { break };
                            work(&mut context, item);
                        }
                    })
                }));
                let error: Box<dyn Any + Send> = match result {
                    Ok(Ok(())) => return,
                    Ok(Err(cancelled)) => Box::new(cancelled),
                    Err(error) => error,
                };
                token.cancel();
                let mut failure = failure.lock().unwrap();
                if failure
                    .as_ref()
                    .is_none_or(|old| old.is::<salsa::Cancelled>())
                {
                    *failure = Some(error);
                }
            });
        }
    });
    if let Some(error) = failure.into_inner().unwrap() {
        resume_unwind(error);
    }
}
