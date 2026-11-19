use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

use crate::{RuntimeError, RuntimeResult};

pub type TaskId = u64;
pub type ObjectHandle = u64;

fn first_handle() -> ObjectHandle {
    1
}

fn first_task_id() -> TaskId {
    1
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ObjectHandleTable {
    #[serde(default = "first_handle")]
    next: ObjectHandle,
    #[serde(default)]
    live: BTreeSet<ObjectHandle>,
}

impl Default for ObjectHandleTable {
    fn default() -> Self {
        Self {
            next: 1,
            live: BTreeSet::new(),
        }
    }
}

impl ObjectHandleTable {
    pub fn allocate(&mut self) -> RuntimeResult<ObjectHandle> {
        let handle = self.next;
        self.next = self
            .next
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Host("object handle space exhausted".to_string()))?;
        self.live.insert(handle);
        Ok(handle)
    }

    pub fn reserve(&mut self, handle: ObjectHandle) -> RuntimeResult<()> {
        if handle == 0 {
            return Err(RuntimeError::Host(
                "object handle zero is reserved".to_string(),
            ));
        }
        let next =
            if handle >= self.next {
                Some(handle.checked_add(1).ok_or_else(|| {
                    RuntimeError::Host("object handle space exhausted".to_string())
                })?)
            } else {
                None
            };
        if !self.live.insert(handle) {
            return Err(RuntimeError::Host(format!(
                "object handle {handle} is already live"
            )));
        }
        if let Some(next) = next {
            self.next = next;
        }
        Ok(())
    }

    pub fn release(&mut self, handle: ObjectHandle) -> bool {
        self.live.remove(&handle)
    }

    pub fn contains(&self, handle: ObjectHandle) -> bool {
        self.live.contains(&handle)
    }

    pub fn handles(&self) -> impl Iterator<Item = ObjectHandle> + '_ {
        self.live.iter().copied()
    }

    pub fn len(&self) -> usize {
        self.live.len()
    }

    pub fn is_empty(&self) -> bool {
        self.live.is_empty()
    }

    pub fn clear(&mut self) {
        self.live.clear();
    }

    pub fn validate(&self) -> RuntimeResult<()> {
        if self.next == 0 || self.live.contains(&0) {
            return Err(RuntimeError::Host(
                "object handle table contains an invalid zero handle".to_string(),
            ));
        }
        if self.live.iter().any(|handle| *handle >= self.next) {
            return Err(RuntimeError::Host(
                "object handle table next value is behind a live handle".to_string(),
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TaskStatus {
    Ready,
    Running,
    Sleeping,
    Completed,
    Cancelled,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RuntimeTask {
    pub id: TaskId,
    pub name: String,
    pub wake_frame: u64,
    pub interval_frames: Option<u64>,
    pub status: TaskStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskScheduler {
    #[serde(default = "first_task_id")]
    next_id: TaskId,
    #[serde(default)]
    tasks: BTreeMap<TaskId, RuntimeTask>,
    #[serde(skip)]
    deadlines: Option<BTreeSet<(u64, TaskId)>>,
}

impl PartialEq for TaskScheduler {
    fn eq(&self, other: &Self) -> bool {
        self.next_id == other.next_id && self.tasks == other.tasks
    }
}

impl Eq for TaskScheduler {}

impl Default for TaskScheduler {
    fn default() -> Self {
        Self {
            next_id: 1,
            tasks: BTreeMap::new(),
            deadlines: Some(BTreeSet::new()),
        }
    }
}

impl TaskScheduler {
    pub fn schedule(
        &mut self,
        name: impl Into<String>,
        wake_frame: u64,
        interval_frames: Option<u64>,
    ) -> RuntimeResult<TaskId> {
        if interval_frames == Some(0) {
            return Err(RuntimeError::Host(
                "task interval must be greater than zero".to_string(),
            ));
        }
        if self.next_id == 0 {
            return Err(RuntimeError::Host("task id space is exhausted".to_string()));
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| RuntimeError::Host("task id space exhausted".to_string()))?;
        self.tasks.insert(
            id,
            RuntimeTask {
                id,
                name: name.into(),
                wake_frame,
                interval_frames,
                status: TaskStatus::Sleeping,
            },
        );
        if let Some(deadlines) = &mut self.deadlines {
            deadlines.insert((wake_frame, id));
        }
        Ok(id)
    }

    pub fn schedule_after(
        &mut self,
        name: impl Into<String>,
        current_frame: u64,
        delay_frames: u64,
        interval_frames: Option<u64>,
    ) -> RuntimeResult<TaskId> {
        self.schedule(
            name,
            current_frame.saturating_add(delay_frames),
            interval_frames,
        )
    }

    pub fn schedule_after_u32(
        &mut self,
        name: impl Into<String>,
        current_frame: u32,
        delay_frames: u32,
        interval_frames: Option<u32>,
    ) -> RuntimeResult<TaskId> {
        self.schedule_after(
            name,
            u64::from(current_frame),
            u64::from(delay_frames),
            interval_frames.map(u64::from),
        )
    }

    pub fn task(&self, id: TaskId) -> Option<&RuntimeTask> {
        self.tasks.get(&id)
    }

    pub fn task_mut(&mut self, id: TaskId) -> Option<&mut RuntimeTask> {
        self.deadlines = None;
        self.tasks.get_mut(&id)
    }

    pub fn tasks(&self) -> impl Iterator<Item = &RuntimeTask> {
        self.tasks.values()
    }

    pub fn ids(&self) -> impl Iterator<Item = TaskId> + '_ {
        self.tasks.keys().copied()
    }

    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }

    pub fn is_active(&self, id: TaskId) -> bool {
        self.task(id).is_some_and(|task| {
            !matches!(task.status, TaskStatus::Completed | TaskStatus::Cancelled)
        })
    }

    pub fn wake(&mut self, id: TaskId, frame: u64) -> RuntimeResult<()> {
        let task = self
            .tasks
            .get_mut(&id)
            .ok_or_else(|| RuntimeError::Host(format!("unknown task {id}")))?;
        if matches!(task.status, TaskStatus::Completed | TaskStatus::Cancelled) {
            return Err(RuntimeError::Host(format!("task {id} is already terminal")));
        }
        if let Some(deadlines) = &mut self.deadlines {
            deadlines.remove(&(task.wake_frame, id));
            deadlines.insert((frame, id));
        }
        task.wake_frame = frame;
        task.status = TaskStatus::Ready;
        Ok(())
    }

    pub fn suspend(&mut self, id: TaskId) -> RuntimeResult<()> {
        let task = self
            .tasks
            .get_mut(&id)
            .ok_or_else(|| RuntimeError::Host(format!("unknown task {id}")))?;
        if task.status == TaskStatus::Running {
            task.status = TaskStatus::Ready;
            if let Some(deadlines) = &mut self.deadlines {
                deadlines.insert((task.wake_frame, id));
            }
        }
        Ok(())
    }

    pub fn status(&self, id: TaskId) -> RuntimeResult<TaskStatus> {
        self.task(id)
            .map(|task| task.status.clone())
            .ok_or_else(|| RuntimeError::Host(format!("unknown task {id}")))
    }

    pub fn poll_ready(&mut self, frame: u64) -> Vec<TaskId> {
        let deadlines = self.deadlines.get_or_insert_with(|| {
            self.tasks.values().filter_map(|task| {
                matches!(task.status, TaskStatus::Sleeping | TaskStatus::Ready)
                    .then_some((task.wake_frame, task.id))
            }).collect()
        });
        let mut ready = Vec::new();
        while deadlines.first().is_some_and(|&(wake, _)| wake <= frame) {
            let (_, id) = deadlines.pop_first().expect("deadline was checked");
            self.tasks.get_mut(&id).expect("deadline has a live task").status = TaskStatus::Running;
            ready.push(id);
        }
        ready.sort_unstable();
        ready
    }

    pub fn poll_ready_u32(&mut self, frame: u32) -> Vec<TaskId> {
        self.poll_ready(u64::from(frame))
    }

    pub fn complete(&mut self, id: TaskId, frame: u64) -> RuntimeResult<()> {
        let task = self
            .tasks
            .get_mut(&id)
            .ok_or_else(|| RuntimeError::Host(format!("unknown task {id}")))?;
        if !matches!(task.status, TaskStatus::Running) {
            return Err(RuntimeError::Host(format!("task {id} is not running")));
        }
        if let Some(interval) = task.interval_frames {
            task.wake_frame = frame.saturating_add(interval);
            task.status = TaskStatus::Sleeping;
            if let Some(deadlines) = &mut self.deadlines {
                deadlines.insert((task.wake_frame, id));
            }
        } else {
            task.status = TaskStatus::Completed;
        }
        Ok(())
    }

    pub fn cancel(&mut self, id: TaskId) -> RuntimeResult<()> {
        let task = self
            .tasks
            .get_mut(&id)
            .ok_or_else(|| RuntimeError::Host(format!("unknown task {id}")))?;
        if matches!(task.status, TaskStatus::Completed | TaskStatus::Cancelled) {
            return Ok(());
        }
        if let Some(deadlines) = &mut self.deadlines {
            deadlines.remove(&(task.wake_frame, id));
        }
        task.status = TaskStatus::Cancelled;
        Ok(())
    }

    pub fn remove(&mut self, id: TaskId) -> RuntimeResult<RuntimeTask> {
        let task = self
            .tasks
            .get(&id)
            .ok_or_else(|| RuntimeError::Host(format!("unknown task {id}")))?;
        if !matches!(task.status, TaskStatus::Completed | TaskStatus::Cancelled) {
            return Err(RuntimeError::Host(format!("task {id} is still active")));
        }
        Ok(self.tasks.remove(&id).expect("task was checked above"))
    }

    pub fn recover_after_restart(&mut self, frame: u64) -> RuntimeResult<()> {
        self.validate()?;
        self.deadlines = None;
        for task in self.tasks.values_mut() {
            if task.status == TaskStatus::Running {
                task.status = if task.wake_frame <= frame {
                    TaskStatus::Ready
                } else {
                    TaskStatus::Sleeping
                };
            }
        }
        Ok(())
    }

    pub fn validate(&self) -> RuntimeResult<()> {
        if self.next_id == 0 {
            return Err(RuntimeError::Host(
                "task scheduler next id is zero".to_string(),
            ));
        }
        let mut max_id = 0;
        for (id, task) in &self.tasks {
            if *id == 0 || task.id != *id {
                return Err(RuntimeError::Host(
                    "task scheduler contains an invalid task id".to_string(),
                ));
            }
            if task.interval_frames == Some(0) {
                return Err(RuntimeError::Host(format!("task {id} has a zero interval")));
            }
            max_id = max_id.max(*id);
        }
        if max_id >= self.next_id {
            return Err(RuntimeError::Host(
                "task scheduler next id is behind an existing task".to_string(),
            ));
        }
        Ok(())
    }

    pub fn retain_active(&mut self) {
        self.tasks.retain(|_, task| {
            !matches!(task.status, TaskStatus::Completed | TaskStatus::Cancelled)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recurring_tasks_sleep_until_the_next_frame() {
        let mut scheduler = TaskScheduler::default();
        let id = scheduler.schedule("clock", 3, Some(2)).unwrap();
        assert!(scheduler.poll_ready(2).is_empty());
        assert_eq!(scheduler.poll_ready(3), vec![id]);
        scheduler.complete(id, 3).unwrap();
        assert!(scheduler.poll_ready(4).is_empty());
        assert_eq!(scheduler.poll_ready(5), vec![id]);
    }

    #[test]
    fn task_completion_requires_a_polled_running_task() {
        let mut scheduler = TaskScheduler::default();
        let id = scheduler.schedule("once", 4, None).unwrap();
        assert!(scheduler.complete(id, 4).is_err());
        assert_eq!(scheduler.poll_ready(4), vec![id]);
        scheduler.complete(id, 4).unwrap();
        assert_eq!(scheduler.task(id).unwrap().status, TaskStatus::Completed);
        scheduler.remove(id).unwrap();
        assert!(scheduler.task(id).is_none());
    }

    #[test]
    fn running_tasks_are_rearmed_after_restart() {
        let mut scheduler = TaskScheduler::default();
        let id = scheduler.schedule("restart", 2, None).unwrap();
        assert_eq!(scheduler.poll_ready(2), vec![id]);
        scheduler.recover_after_restart(2).unwrap();
        assert_eq!(scheduler.task(id).unwrap().status, TaskStatus::Ready);
        assert_eq!(scheduler.poll_ready(2), vec![id]);
    }

    #[test]
    fn indexed_deadlines_keep_id_order_and_rebuild_after_mutation() {
        let mut scheduler = TaskScheduler::default();
        let first = scheduler.schedule("late", 5, None).unwrap();
        let second = scheduler.schedule("early", 2, None).unwrap();
        let cancelled = scheduler.schedule("cancelled", 2, None).unwrap();
        let future = scheduler.schedule("future", 1000, None).unwrap();
        scheduler.cancel(cancelled).unwrap();
        scheduler.task_mut(future).unwrap().wake_frame = 4;
        assert!(scheduler.poll_ready(1).is_empty());
        assert_eq!(scheduler.poll_ready(5), vec![first, second, future]);
        scheduler.suspend(first).unwrap();
        assert_eq!(scheduler.poll_ready(5), vec![first]);
        scheduler.wake(second, 10).unwrap();
        assert!(scheduler.poll_ready(9).is_empty());
        assert_eq!(scheduler.poll_ready(10), vec![second]);
    }

    #[test]
    fn object_handles_are_monotonic_and_validate_restored_state() {
        let mut handles = ObjectHandleTable::default();
        let first = handles.allocate().unwrap();
        let second = handles.allocate().unwrap();
        assert_eq!((first, second), (1, 2));
        assert!(handles.release(first));
        assert!(!handles.contains(first));
        handles.reserve(9).unwrap();
        assert_eq!(handles.allocate().unwrap(), 10);
        assert!(handles.validate().is_ok());
    }
}
