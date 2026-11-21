use std::{
    any::Any,
    cell::{Cell, RefCell},
    iter::Enumerate,
    panic::{AssertUnwindSafe, catch_unwind, resume_unwind},
    ptr::NonNull,
    slice::ChunksMut,
    sync::{Arc, Condvar, Mutex, MutexGuard},
    thread::{self, JoinHandle},
};

const MAX_EXTRA_WORKERS: usize = 3;

thread_local! {
    static ACTIVE: Cell<bool> = const { Cell::new(false) };
    static POOL: RefCell<RowPool> = RefCell::new(RowPool::new());
}

/// Draw disjoint RGBA rows. The callback receives the absolute row index.
pub(super) fn draw_pixel_rows(
    target: &mut [u8],
    stride: usize,
    top: u32,
    workers: usize,
    draw_row: impl Fn(&mut [u8], u32) + Sync,
) {
    assert!(stride > 0 && target.len().is_multiple_of(stride));
    if target.is_empty() {
        return;
    }

    let rows = target.len() / stride;
    let workers = workers.clamp(1, 4).min(rows);
    if workers == 1 {
        process_rows(top, target, stride, &draw_row);
        return;
    }

    // A nested call runs serially. In particular, this avoids borrowing POOL
    // recursively and avoids making a worker wait for work from its own pool.
    let Some(_active) = ActiveGuard::try_enter() else {
        process_rows(top, target, stride, &draw_row);
        return;
    };
    let executed = POOL
        .try_with(|pool| {
            let Ok(mut pool) = pool.try_borrow_mut() else {
                return false;
            };
            pool.execute(target, stride, top, workers, &draw_row)
        })
        .unwrap_or(false);
    if !executed {
        // This includes failure to create the first worker and unavailable
        // thread-local storage during thread destruction.
        process_rows(top, target, stride, &draw_row);
    }
}

fn process_rows(
    first: u32,
    band: &mut [u8],
    stride: usize,
    draw_row: &(dyn Fn(&mut [u8], u32) + Sync),
) {
    for (row, pixels) in band.chunks_exact_mut(stride).enumerate() {
        draw_row(pixels, first + row as u32);
    }
}

struct ActiveGuard;

impl ActiveGuard {
    fn try_enter() -> Option<Self> {
        ACTIVE
            .try_with(|active| {
                if active.get() {
                    None
                } else {
                    active.set(true);
                    Some(Self)
                }
            })
            .ok()
            .flatten()
    }
}

impl Drop for ActiveGuard {
    fn drop(&mut self) {
        let _ = ACTIVE.try_with(|active| active.set(false));
    }
}

struct RowPool {
    shared: Arc<Shared>,
    workers: Vec<JoinHandle<()>>,
}

impl RowPool {
    fn new() -> Self {
        Self {
            shared: Arc::new(Shared::new()),
            workers: Vec::new(),
        }
    }

    fn execute(
        &mut self,
        target: &mut [u8],
        stride: usize,
        top: u32,
        workers: usize,
        draw_row: &(dyn Fn(&mut [u8], u32) + Sync),
    ) -> bool {
        debug_assert!((2..=4).contains(&workers));
        let requested = (workers - 1).min(MAX_EXTRA_WORKERS);
        self.ensure_workers(requested);
        let participants = self.workers.len().min(requested);
        if participants == 0 {
            return false;
        }

        let rows = target.len() / stride;
        let band_rows = rows.div_ceil(workers);
        let job = Job {
            bands: Mutex::new(target.chunks_mut(band_rows * stride).enumerate()),
            draw_row,
            top,
            stride,
            band_rows,
        };

        {
            let mut state = lock(&self.shared.state);
            debug_assert!(state.job.is_none());
            state.panic = None;
            state.generation = state.generation.wrapping_add(1);
            state.job = Some(JobRef::new(&job));
            state.participants = participants;
            state.finished = 0;
        }
        self.shared.start.notify_all();

        // Do not unwind past `job` while workers can still access it. If the
        // caller panics, the remaining workers drain the queue before we join
        // their logical completion, just as with scoped threads.
        let caller_result = catch_unwind(AssertUnwindSafe(|| job.work()));
        let mut state = lock(&self.shared.state);
        while state.finished < participants {
            state = wait(&self.shared.finished, state);
        }
        state.job = None;
        state.participants = 0;
        state.finished = 0;
        let worker_panic = state.panic.take();
        drop(state);

        if let Err(payload) = caller_result {
            resume_unwind(payload);
        }
        if let Some(payload) = worker_panic {
            resume_unwind(payload);
        }
        true
    }

    fn ensure_workers(&mut self, count: usize) {
        while self.workers.len() < count {
            let index = self.workers.len();
            let shared = Arc::clone(&self.shared);
            match thread::Builder::new().spawn(move || worker_loop(shared, index)) {
                Ok(handle) => self.workers.push(handle),
                Err(_) => break,
            }
        }
    }
}

impl Drop for RowPool {
    fn drop(&mut self) {
        {
            let mut state = lock(&self.shared.state);
            state.stopping = true;
        }
        self.shared.start.notify_all();
        // Each draw already waits for its workers. During Windows process
        // shutdown, TLS destructors can run after other threads were killed
        // by the OS, so the handles must be dropped without joining them.
    }
}

struct Shared {
    state: Mutex<State>,
    start: Condvar,
    finished: Condvar,
}

impl Shared {
    fn new() -> Self {
        Self {
            state: Mutex::new(State {
                generation: 0,
                job: None,
                participants: 0,
                finished: 0,
                stopping: false,
                panic: None,
            }),
            start: Condvar::new(),
            finished: Condvar::new(),
        }
    }
}

struct State {
    generation: u64,
    job: Option<JobRef>,
    participants: usize,
    finished: usize,
    stopping: bool,
    panic: Option<Box<dyn Any + Send>>,
}

fn worker_loop(shared: Arc<Shared>, index: usize) {
    let mut observed_generation = 0;
    loop {
        let job = {
            let mut state = lock(&shared.state);
            loop {
                if state.stopping {
                    return;
                }
                if state.generation != observed_generation {
                    observed_generation = state.generation;
                    if index < state.participants {
                        if let Some(job) = state.job {
                            break job;
                        }
                    }
                }
                state = wait(&shared.start, state);
            }
        };

        let result = {
            let _active = ActiveGuard::try_enter();
            catch_unwind(AssertUnwindSafe(|| job.run()))
        };

        let mut state = lock(&shared.state);
        if let Err(payload) = result {
            if state.panic.is_none() {
                state.panic = Some(payload);
            }
        }
        state.finished += 1;
        if state.finished == state.participants {
            shared.finished.notify_one();
        }
    }
}

struct Job<'a> {
    bands: Mutex<Enumerate<ChunksMut<'a, u8>>>,
    draw_row: &'a (dyn Fn(&mut [u8], u32) + Sync),
    top: u32,
    stride: usize,
    band_rows: usize,
}

impl Job<'_> {
    fn work(&self) {
        loop {
            let next = lock(&self.bands).next();
            let Some((index, band)) = next else {
                break;
            };
            process_rows(
                self.top + (index * self.band_rows) as u32,
                band,
                self.stride,
                self.draw_row,
            );
        }
    }
}

#[derive(Clone, Copy)]
struct JobRef(NonNull<()>);

impl JobRef {
    fn new(job: &Job<'_>) -> Self {
        Self(NonNull::from(job).cast())
    }

    fn run(self) {
        // SAFETY: JobRef is created from the Job on RowPool::execute's stack.
        // execute waits for every participating worker to finish this job
        // before clearing it or unwinding, including when the caller panics.
        // Each worker calls run at most once for a generation.
        let job = unsafe { &*self.0.cast::<Job<'_>>().as_ptr() };
        job.work();
    }
}

// SAFETY: Job is shared between threads through a Mutex-protected band queue;
// its callback is Sync and each queue item owns a disjoint mutable slice.
// JobRef cannot outlive Job because execute waits for all participants before
// returning or resuming a panic. Moving the pointer to a worker is therefore
// equivalent to sharing &Job for that bounded interval.
unsafe impl Send for JobRef {}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn wait<'a, T>(condvar: &Condvar, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
    condvar
        .wait(guard)
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::{Arc, Mutex, RowPool, catch_unwind, draw_pixel_rows};
    use std::panic::AssertUnwindSafe;

    #[test]
    fn parallel_rows_match_serial_rows() {
        let mut expected = vec![0; 4 * 64];
        let mut actual = vec![0; 4 * 64];
        let draw = |row: &mut [u8], y: u32| row.fill(y as u8);
        draw_pixel_rows(&mut expected, 4, 7, 1, draw);
        for _ in 0..3 {
            actual.fill(0);
            draw_pixel_rows(&mut actual, 4, 7, 4, draw);
            assert_eq!(actual, expected);
        }
    }

    #[test]
    fn nested_draw_is_serial_and_completes() {
        let mut outer = vec![0; 4 * 16];
        let inner = Mutex::new(vec![0; 4 * 16]);
        draw_pixel_rows(&mut outer, 4, 0, 4, |row, y| {
            row.fill(y as u8);
            if y == 3 {
                let mut inner = inner.lock().unwrap();
                draw_pixel_rows(&mut inner[..], 4, 20, 4, |row, y| {
                    row.fill(y as u8);
                });
            }
        });
        let inner = inner.into_inner().unwrap();
        for (index, row) in inner.chunks_exact(4).enumerate() {
            assert!(row.iter().all(|&byte| byte == 20 + index as u8));
        }
    }

    #[test]
    fn panic_is_propagated_and_pool_can_be_reused() {
        let mut target = vec![0; 4 * 64];
        let result = catch_unwind(AssertUnwindSafe(|| {
            draw_pixel_rows(&mut target, 4, 10, 4, |row, y| {
                if y == 12 {
                    panic!("row failure");
                }
                row.fill(y as u8);
            });
        }));
        assert!(result.is_err());

        let mut expected = vec![0; 4 * 64];
        draw_pixel_rows(&mut expected, 4, 10, 1, |row, y| row.fill(y as u8));
        draw_pixel_rows(&mut target, 4, 10, 4, |row, y| row.fill(y as u8));
        assert_eq!(target, expected);
    }

    #[test]
    fn dropping_pool_signals_workers_to_stop() {
        let mut pool = RowPool::new();
        let shared = Arc::clone(&pool.shared);
        let mut target = vec![0; 4 * 16];
        let _ = pool.execute(&mut target, 4, 0, 4, &|row, y| row.fill(y as u8));
        let workers = std::mem::take(&mut pool.workers);
        drop(pool);
        for worker in workers {
            worker.join().unwrap();
        }
        assert_eq!(Arc::strong_count(&shared), 1);
    }
}
