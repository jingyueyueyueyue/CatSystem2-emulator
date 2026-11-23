//! Cooperative native/instruction boundaries; never interrupts a native halfway through.
use std::{cell::Cell, time::{Duration, Instant}};

thread_local! {
    static DEADLINE: Cell<Option<Instant>> = const { Cell::new(None) };
}

pub(super) struct Scope(Option<Instant>);

impl Scope {
    pub(super) fn enter(limit: Duration) -> Self {
        let now = Instant::now();
        let deadline = now.checked_add(limit).unwrap_or(now);
        let previous = DEADLINE.with(|slot| {
            let previous = slot.get();
            slot.set(Some(previous.map_or(deadline, |outer| outer.min(deadline))));
            previous
        });
        Self(previous)
    }
}

impl Drop for Scope {
    fn drop(&mut self) { DEADLINE.with(|slot| slot.set(self.0)); }
}

pub(super) fn exhausted() -> bool {
    DEADLINE.with(|slot| slot.get().is_some_and(|deadline| Instant::now() >= deadline))
}

#[test]
fn frame_work_budget_nested_scope_cannot_extend_outer_deadline() {
    assert!(!exhausted());
    {
        let _outer = Scope::enter(Duration::ZERO);
        assert!(exhausted());
        {
            let _inner = Scope::enter(Duration::from_secs(30));
            assert!(exhausted());
        }
        assert!(exhausted());
    }
    assert!(!exhausted());
}
