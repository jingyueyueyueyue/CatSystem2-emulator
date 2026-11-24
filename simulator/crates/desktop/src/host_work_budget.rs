//! Cooperative native/instruction boundaries; never interrupts a native halfway through.
use std::{cell::Cell, time::{Duration, Instant}};

pub(super) const INTERACTIVE_INSTRUCTION_SLICE: usize = 2048;

thread_local! {
    static DEADLINE: Cell<Option<Deadline>> = const { Cell::new(None) };
}

#[derive(Clone, Copy)]
struct Deadline {
    at: Instant,
    yielded: bool,
}

pub(super) struct Scope(Option<Deadline>);

impl Scope {
    pub(super) fn enter(limit: Duration) -> Self {
        let now = Instant::now();
        let deadline = now.checked_add(limit).unwrap_or(now);
        let previous = DEADLINE.with(|slot| {
            let previous = slot.get();
            let at = previous.map_or(deadline, |outer| outer.at.min(deadline));
            slot.set(Some(Deadline { at, yielded: false }));
            previous
        });
        Self(previous)
    }

    pub(super) fn yielded(&self) -> bool {
        DEADLINE.with(|slot| slot.get().is_some_and(|deadline| deadline.yielded))
    }
}

impl Drop for Scope {
    fn drop(&mut self) { DEADLINE.with(|slot| slot.set(self.0)); }
}

pub(super) fn exhausted() -> bool {
    DEADLINE.with(|slot| slot.get().is_some_and(|deadline| Instant::now() >= deadline.at))
}

fn check_deadline(slot: &Cell<Option<Deadline>>, mut deadline: Deadline) -> bool {
    if Instant::now() < deadline.at {
        return false;
    }
    deadline.yielded = true;
    slot.set(Some(deadline));
    true
}

pub(super) fn should_yield() -> bool {
    DEADLINE.with(|slot| slot.get().is_some_and(|deadline| check_deadline(slot, deadline)))
}

pub(super) fn renew_instruction_budget(budget: usize) -> bool {
    budget == INTERACTIVE_INSTRUCTION_SLICE
        && DEADLINE.with(|slot| slot.get().is_some_and(|deadline| !check_deadline(slot, deadline)))
}

#[test]
fn frame_work_budget_nested_scope_cannot_extend_outer_deadline() {
    assert!(!exhausted());
    {
        let outer = Scope::enter(Duration::ZERO);
        assert!(exhausted());
        assert!(!outer.yielded());
        {
            let inner = Scope::enter(Duration::from_secs(30));
            assert!(exhausted());
            assert!(should_yield());
            assert!(inner.yielded());
        }
        assert!(exhausted());
        assert!(!outer.yielded());
        assert!(!renew_instruction_budget(INTERACTIVE_INSTRUCTION_SLICE));
        assert!(outer.yielded());
    }
    assert!(!exhausted());
}
