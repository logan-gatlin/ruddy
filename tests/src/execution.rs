use std::{num::NonZeroUsize, panic::catch_unwind, sync::Mutex};

#[cfg(feature = "parallel")]
use std::{panic::AssertUnwindSafe, sync::Arc};

use ruddy::{cancellation::Cancellation, execution::Execution};

#[test]
fn sequential_runs_on_the_caller_and_restores_scoped_defaults() {
    let caller = std::thread::current().id();
    let execution = Execution::with_threads(NonZeroUsize::MIN).unwrap();
    execution.run(|| {
        Execution::default().for_each((), &[0, 1, 2], |_, _| {
            assert_eq!(std::thread::current().id(), caller);
        });
        let result = catch_unwind(|| {
            Execution::sequential().run(|| std::panic::panic_any(42u32));
        });
        assert_eq!(*result.unwrap_err().downcast::<u32>().unwrap(), 42);
        Execution::default().for_each((), &[0, 1], |_, _| {
            assert_eq!(std::thread::current().id(), caller);
        });
    });
}

#[test]
fn empty_and_cancelled_jobs_do_not_execute() {
    let execution = Execution::default();
    execution.for_each((), &[] as &[usize], |_, _| panic!("empty"));
    let token = Cancellation::default();
    assert!(
        token
            .run(|| {
                token.cancel();
                execution.for_each((), &[0, 1, 2], |_, _| panic!("cancelled"));
            })
            .is_err()
    );
    execution.for_each((), &[0], |_, _| {});
}

#[cfg(not(feature = "parallel"))]
#[test]
fn threadless_build_rejects_explicit_parallelism_and_defaults_to_caller() {
    assert_eq!(
        Execution::with_threads(NonZeroUsize::new(2).unwrap())
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::Unsupported
    );
    let caller = std::thread::current().id();
    Execution::default().for_each((), &[0, 1, 2], |_, _| {
        assert_eq!(std::thread::current().id(), caller);
    });
}

#[cfg(feature = "parallel")]
#[test]
fn workers_overlap_and_release_their_contexts_before_returning() {
    use std::sync::{
        Barrier,
        atomic::{AtomicUsize, Ordering},
    };
    struct Context(Arc<AtomicUsize>);
    impl Clone for Context {
        fn clone(&self) -> Self {
            self.0.fetch_add(1, Ordering::SeqCst);
            Self(self.0.clone())
        }
    }
    impl Drop for Context {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }
    let execution = Execution::with_threads(NonZeroUsize::new(2).unwrap()).unwrap();
    let alive = Arc::new(AtomicUsize::new(1));
    let barrier = Barrier::new(2);
    let workers = Mutex::new(std::collections::HashSet::new());
    execution.for_each(Context(alive.clone()), &[0, 1], |_, _| {
        workers.lock().unwrap().insert(std::thread::current().id());
        barrier.wait();
    });
    assert_eq!(workers.into_inner().unwrap().len(), 2);
    assert_eq!(alive.load(Ordering::SeqCst), 0);
    // The same pool remains usable after the request's contexts are gone.
    execution.for_each((), &[0, 1], |_, _| {});
}

#[cfg(feature = "parallel")]
#[test]
fn worker_failure_cancels_siblings_and_preserves_the_original_payload() {
    use std::sync::Barrier;
    let execution = Execution::with_threads(NonZeroUsize::new(2).unwrap()).unwrap();
    let barrier = Barrier::new(2);
    let token = Cancellation::default();
    let failure = catch_unwind(AssertUnwindSafe(|| {
        token.run(|| {
            execution.for_each((), &[0, 1], |_, job| {
                barrier.wait();
                if *job == 0 {
                    std::panic::panic_any(73u32);
                }
                loop {
                    ruddy::cancellation::checkpoint();
                    std::thread::yield_now();
                }
            });
        })
    }));
    assert_eq!(*failure.unwrap_err().downcast::<u32>().unwrap(), 73);
    assert!(token.is_cancelled());
    // Worker thread-local cancellation is restored for the next request.
    execution.for_each((), &[0, 1], |_, _| {});
}

#[cfg(feature = "parallel")]
#[test]
fn cancellation_reaches_running_workers_and_pool_can_be_reused() {
    use std::sync::Barrier;
    let execution = Execution::with_threads(NonZeroUsize::new(2).unwrap()).unwrap();
    let barrier = Arc::new(Barrier::new(3));
    let token = Cancellation::default();
    std::thread::scope(|scope| {
        let barrier = &barrier;
        let token = &token;
        scope.spawn(move || {
            barrier.wait();
            token.cancel();
        });
        assert!(
            token
                .run(|| execution.for_each((), &[0, 1], |_, _| {
                    barrier.wait();
                    loop {
                        ruddy::cancellation::checkpoint();
                        std::thread::yield_now();
                    }
                }))
                .is_err()
        );
    });
    execution.for_each((), &[0, 1], |_, _| {});
}

#[test]
fn sequential_jobs_reuse_non_sync_worker_state() {
    // A non-Sync worker context is supported (Salsa databases have this property).
    let values = Mutex::new(Vec::new());
    Execution::sequential().run(|| {
        Execution::default().for_each(std::cell::Cell::new(0), &[1, 2], |state, value| {
            state.set(state.get() + value);
            values.lock().unwrap().push(state.get());
        });
    });
    assert_eq!(*values.lock().unwrap(), [1, 3]);
}
