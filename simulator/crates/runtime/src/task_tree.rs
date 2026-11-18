//! Signed-id task hierarchy and resumable lifecycle traversal. Object-specific
//! notifications and resource ownership are supplied by TaskLifecycle.
use std::collections::BTreeMap;
use std::ops::Bound::{Excluded, Unbounded};

use crate::{RuntimeError, RuntimeResult, TaskClock};

pub type EngineTaskId = i32;

#[derive(Clone, Debug)]
pub struct TaskNode {
    id: EngineTaskId,
    parent: Option<EngineTaskId>,
    pub object_type: u32,
    pub clock: TaskClock,
    pub deletion: u32,
    pub property: u32,
    registered: bool,
    destroying: bool,
    // Every descendant maps to the first direct child along its path.
    // None is a tombstone, retained until this layer's update sweeps it.
    index: BTreeMap<EngineTaskId, Option<EngineTaskId>>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct Lifecycle {
        events: Vec<(u8, i32)>,
        allocations: Vec<(i32, i32)>,
    }

    impl TaskLifecycle for Lifecycle {
        fn begin_update(&mut self, _: &mut TaskTree, id: i32) -> RuntimeResult<()> {
            self.events.push((0, id));
            Ok(())
        }
        fn update_body(&mut self, tree: &mut TaskTree, id: i32, _: u32, _: usize) -> RuntimeResult<bool> {
            self.events.push((1, id));
            self.allocations.push((id, tree.next_id()?));
            assert_eq!(tree.node(id).unwrap().clock.current, 50);
            Ok(true)
        }
        fn request_delete(&mut self, _: &mut TaskTree, command: TaskCommand, _: usize) -> RuntimeResult<bool> {
            assert_eq!(command.code, 3);
            assert_eq!(command.data, [0; 3]);
            self.events.push((2, command.target));
            Ok(true)
        }
        fn prepare_release(&mut self, _: &mut TaskTree, id: i32) -> RuntimeResult<()> {
            self.events.push((3, id));
            Ok(())
        }
        fn release(&mut self, _: &mut TaskTree, id: i32) -> RuntimeResult<()> {
            self.events.push((4, id));
            Ok(())
        }
    }

    #[test]
    fn unregister_hides_parent_but_preserves_descendant_alias_and_internal_parent_route() {
        let mut tree = TaskTree::new(0, 0);
        tree.insert(0, 1, 10).unwrap();
        tree.insert(1, 2, 10).unwrap();
        tree.insert(0, 3, 10).unwrap();
        tree.unregister(1).unwrap();
        assert!(tree.find_in_subtree(0, 1).is_none());
        assert_eq!(tree.find_in_subtree(0, 2).unwrap().id(), 2);
        assert_eq!(tree.route(3, 1), None);
        assert_eq!(tree.route(2, 1), Some(1));
        assert_eq!(tree.children(0), [3]);
        assert_eq!(tree.descendants(0), [2, 3]);
    }

    #[test]
    fn marked_parent_updates_children_then_destroys_and_reclaims_after_root_sweep() {
        let mut tree = TaskTree::new(0, 0);
        tree.insert(0, 1, 10).unwrap();
        tree.insert(1, -2, 10).unwrap();
        tree.insert(0, 4, 10).unwrap();
        tree.mark_delete(1, 9).unwrap();
        let mut lifecycle = Lifecycle::default();
        // Resume every traversal stage with a different host timestamp; the
        // whole update must retain its first timestamp and its original cursor.
        assert!(!tree.update(50, 1, &mut lifecycle, 1).unwrap());
        for _ in 0..100 {
            if tree.update(999, 20, &mut lifecycle, 1).unwrap() { break; }
        }
        assert!(!tree.has_pending_update());
        assert_eq!(lifecycle.events, [
            (0, 0), (0, 1), (0, -2), (1, -2), (2, -2),
            (3, -2), (4, -2), (3, 1), (4, 1), (0, 4), (1, 4), (1, 0),
        ]);
        assert_eq!(lifecycle.allocations, [(-2, 2), (4, 2), (0, 1)]);
        assert!(tree.node(1).is_none());
        assert!(tree.node(-2).is_none());
        assert_eq!(tree.descendants(0), [4]);
    }

    #[test]
    fn base_control_checks_deletion_before_specific_handler_only() {
        let mut tree = TaskTree::new(0, 7);
        let node = tree.node_mut(0).unwrap();
        let command = TaskCommand { target: 0, sender: 0, code: 24, data: [1, 0xffff_ffff, 0] };
        let mut output = 99;
        assert!(!node.dispatch_control(command, &mut output, &mut |node, _, _| {
            node.deletion = 1;
            Ok(true)
        }).unwrap());
        assert_eq!(node.property, u32::MAX);
        let query = TaskCommand { code: 13, ..command };
        assert!(!node.dispatch_control(query, &mut output, &mut |_, _, _| {
            panic!("deleted node must not enter specific handler")
        }).unwrap());
        assert_eq!(output, 99);
    }

    #[test]
    fn typed_broadcast_visits_descendants_after_consumption_and_root_forwarding() {
        let mut tree = TaskTree::new(0, 0);
        tree.insert(0, 1, 10).unwrap();
        tree.insert(1, 2, 20).unwrap();
        tree.insert(0, 4, 10).unwrap();
        let command = TaskCommand { target: 2, sender: 4, code: 13, data: [7, 8, 1] };
        let mut seen = Vec::new();
        let mut output = 99;
        let mut handler = |_: &mut TaskTree, id, packet: &mut TaskCommand, output: &mut u32| {
            assert_eq!(packet.target, id);
            assert_eq!(packet.sender, 4);
            assert_eq!(packet.data, [7, 8, 0]);
            seen.push(id);
            *output = id as u32;
            Ok(false)
        };
        assert!(!tree.broadcast(4, command, Some(10), &mut output, &mut handler).unwrap());
        assert_eq!(seen, [1, 4]);
        let mut seen = Vec::new();
        assert!(tree.broadcast(0, command, Some(10), &mut output, &mut |_, id, _, _| {
            seen.push(id);
            Ok(false)
        }).unwrap());
        assert_eq!(seen, [1]);
    }
}

impl TaskNode {
    pub fn id(&self) -> EngineTaskId { self.id }
    pub fn parent(&self) -> Option<EngineTaskId> { self.parent }
    pub fn is_registered(&self) -> bool { self.registered }
    pub fn is_destroying(&self) -> bool { self.destroying }

    /// Call after the object's own handler has allowed base-class handling.
    pub fn dispatch_base_control(&mut self, command: TaskCommand, output: &mut u32) -> bool {
        crate::task::dispatch_base_control(
            self.object_type, &mut self.deletion, &mut self.property, &mut self.clock, command, output,
        )
    }

    /// The deletion check belongs at entry. A specific handler may mark the
    /// node and still allow this same command to reach the base handler.
    pub fn dispatch_control(
        &mut self,
        command: TaskCommand,
        output: &mut u32,
        handler: &mut impl FnMut(&mut Self, TaskCommand, &mut u32) -> RuntimeResult<bool>,
    ) -> RuntimeResult<bool> {
        if self.deletion != 0 || !handler(self, command, output)? {
            return Ok(false);
        }
        Ok(self.dispatch_base_control(command, output))
    }

    fn new(id: EngineTaskId, parent: Option<EngineTaskId>, object_type: u32) -> Self {
        Self {
            id,
            parent,
            object_type,
            clock: TaskClock::default(),
            deletion: 0,
            property: u32::MAX,
            registered: true,
            destroying: false,
            index: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct TaskCommand {
    pub target: EngineTaskId,
    pub sender: EngineTaskId,
    pub code: u32,
    pub data: [u32; 3],
}

/// All callbacks are required: a host must explicitly supply object updates,
/// synchronous delete notifications and its two resource-release stages.
pub trait TaskLifecycle {
    fn begin_update(&mut self, tree: &mut TaskTree, id: EngineTaskId) -> RuntimeResult<()>;
    fn update_body(
        &mut self,
        tree: &mut TaskTree,
        id: EngineTaskId,
        increment: u32,
        budget: usize,
    ) -> RuntimeResult<bool>;
    fn request_delete(
        &mut self,
        tree: &mut TaskTree,
        command: TaskCommand,
        budget: usize,
    ) -> RuntimeResult<bool>;
    fn prepare_release(&mut self, tree: &mut TaskTree, id: EngineTaskId) -> RuntimeResult<()>;
    fn release(&mut self, tree: &mut TaskTree, id: EngineTaskId) -> RuntimeResult<()>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Begin,
    Children(Option<EngineTaskId>),
    Body,
    Unregister,
    NotifyChildren(Option<EngineTaskId>),
    NotifySelf,
    DestroyChildren(Option<EngineTaskId>),
    PrepareRelease,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Visit {
    id: EngineTaskId,
    phase: Phase,
}

#[derive(Clone, Debug)]
struct Traversal {
    now: u32,
    increment: u32,
    destroying: bool,
    target: EngineTaskId,
    stack: Vec<Visit>,
}

pub struct TaskTree {
    root: EngineTaskId,
    nodes: BTreeMap<EngineTaskId, TaskNode>,
    pending: Option<Traversal>,
    in_callback: bool,
    closed: bool,
}

impl TaskTree {
    pub fn new(root: EngineTaskId, object_type: u32) -> Self {
        Self {
            root,
            nodes: BTreeMap::from([(root, TaskNode::new(root, None, object_type))]),
            pending: None,
            in_callback: false,
            closed: false,
        }
    }

    pub fn root(&self) -> EngineTaskId { self.root }
    pub fn is_closed(&self) -> bool { self.closed }
    pub fn node(&self, id: EngineTaskId) -> Option<&TaskNode> { self.nodes.get(&id) }
    pub fn node_mut(&mut self, id: EngineTaskId) -> Option<&mut TaskNode> { self.nodes.get_mut(&id) }
    pub fn has_pending_update(&self) -> bool { self.pending.is_some() }

    pub fn next_id(&self) -> RuntimeResult<EngineTaskId> {
        let index = self.nodes.get(&self.root).map(|root| &root.index);
        let mut candidate = 0i32;
        loop {
            if !self.nodes.contains_key(&candidate)
                && !index.is_some_and(|index| index.contains_key(&candidate))
            {
                return Ok(candidate);
            }
            candidate = candidate.checked_add(1)
                .ok_or_else(|| RuntimeError::Host("task number space exhausted".into()))?;
        }
    }

    pub fn insert(
        &mut self,
        parent: EngineTaskId,
        id: EngineTaskId,
        object_type: u32,
    ) -> RuntimeResult<()> {
        if self.closed {
            return Err(RuntimeError::Host("task tree is closed".into()));
        }
        let parent_node = self.nodes.get(&parent)
            .ok_or_else(|| RuntimeError::Host(format!("missing parent task {parent}")))?;
        if parent_node.destroying {
            return Err(RuntimeError::Host("cannot attach a child during parent destruction".into()));
        }
        if self.nodes.contains_key(&id)
            || self.nodes.get(&self.root).is_some_and(|root| root.index.contains_key(&id))
        {
            return Err(RuntimeError::Host(format!("task number {id} is still reserved")));
        }
        // Bound recursion in update/routing and avoid pathological cyclic
        // metadata even when a host has restored its own object state.
        let mut depth = 0usize;
        let mut ancestor = Some(parent);
        while let Some(current) = ancestor {
            depth += 1;
            if depth > 256 {
                return Err(RuntimeError::Host("task hierarchy exceeds 256 levels".into()));
            }
            ancestor = self.nodes.get(&current).and_then(|node| node.parent);
        }
        self.nodes.insert(id, TaskNode::new(id, Some(parent), object_type));
        let mut branch = id;
        let mut ancestor = Some(parent);
        while let Some(current) = ancestor {
            let node = self.nodes.get_mut(&current).expect("validated ancestor");
            node.index.insert(id, Some(branch));
            ancestor = node.parent;
            branch = current;
        }
        Ok(())
    }

    pub fn allocate(&mut self, parent: EngineTaskId, object_type: u32) -> RuntimeResult<EngineTaskId> {
        let id = self.next_id()?;
        self.insert(parent, id, object_type)?;
        Ok(id)
    }

    pub fn find_in_subtree(&self, from: EngineTaskId, target: EngineTaskId) -> Option<&TaskNode> {
        let mut node = self.nodes.get(&from)?;
        for _ in 0..=256 {
            if node.id == target { return Some(node); }
            let branch = node.index.get(&target).copied().flatten()?;
            node = self.nodes.get(&branch)?;
        }
        None
    }

    /// Search the sender's index, then follow actual parent pointers. An
    /// unregistered ancestor is reachable from within its own descendants.
    pub fn route(&self, from: EngineTaskId, target: EngineTaskId) -> Option<EngineTaskId> {
        let mut from = Some(from);
        while let Some(id) = from {
            if let Some(node) = self.find_in_subtree(id, target) { return Some(node.id); }
            from = self.nodes.get(&id)?.parent;
        }
        None
    }

    pub fn descendants(&self, id: EngineTaskId) -> Vec<EngineTaskId> {
        self.nodes.get(&id).map_or_else(Vec::new, |node| {
            node.index.iter().filter_map(|(id, branch)| branch.map(|_| *id)).collect()
        })
    }

    pub fn children(&self, id: EngineTaskId) -> Vec<EngineTaskId> {
        self.nodes.values()
            .filter(|node| node.parent == Some(id) && node.registered)
            .map(|node| node.id).collect()
    }

    pub fn unregister(&mut self, id: EngineTaskId) -> RuntimeResult<()> {
        let node = self.nodes.get_mut(&id)
            .ok_or_else(|| RuntimeError::Host(format!("unknown task {id}")))?;
        node.registered = false;
        let mut ancestor = node.parent;
        while let Some(current) = ancestor {
            let node = self.nodes.get_mut(&current).expect("live task has live ancestors");
            node.index.insert(id, None);
            ancestor = node.parent;
        }
        Ok(())
    }

    pub fn sweep(&mut self, id: EngineTaskId) {
        if let Some(node) = self.nodes.get_mut(&id) {
            node.index.retain(|_, branch| branch.is_some());
        }
    }

    pub fn mark_delete(&mut self, id: EngineTaskId, value: u32) -> RuntimeResult<()> {
        let node = self.nodes.get_mut(&id)
            .ok_or_else(|| RuntimeError::Host(format!("unknown task {id}")))?;
        node.deletion = value;
        Ok(())
    }

    fn next_child(&self, parent: EngineTaskId, cursor: Option<EngineTaskId>, registered: bool)
        -> Option<EngineTaskId>
    {
        let lower = cursor.map_or(Unbounded, Excluded);
        self.nodes.range((lower, Unbounded))
            .find(|(_, node)| node.parent == Some(parent) && (!registered || node.registered))
            .map(|(id, _)| *id)
    }

    pub fn update(
        &mut self,
        now: u32,
        increment: u32,
        lifecycle: &mut impl TaskLifecycle,
        budget: usize,
    ) -> RuntimeResult<bool> {
        if self.closed {
            return Err(RuntimeError::Host("cannot update a closed task tree".into()));
        }
        if self.in_callback {
            return Err(RuntimeError::Host("task tree update is reentrant".into()));
        }
        let traversal = match self.pending.take() {
            Some(pending) if !pending.destroying => pending,
            Some(pending) => {
                self.pending = Some(pending);
                return Err(RuntimeError::Host("finish task destruction before updating".into()));
            }
            None => Traversal {
                now, increment, destroying: false, target: self.root,
                stack: vec![Visit { id: self.root, phase: Phase::Begin }],
            },
        };
        self.drive(traversal, lifecycle, budget)
    }

    pub fn destroy(
        &mut self,
        id: EngineTaskId,
        lifecycle: &mut impl TaskLifecycle,
        budget: usize,
    ) -> RuntimeResult<bool> {
        if self.in_callback {
            return Err(RuntimeError::Host("use mark_delete from lifecycle callbacks".into()));
        }
        let traversal = match self.pending.take() {
            Some(pending) if pending.destroying && pending.target == id => pending,
            Some(pending) => {
                self.pending = Some(pending);
                return Err(RuntimeError::Host("finish the active task traversal first".into()));
            }
            None => {
                if !self.nodes.contains_key(&id) {
                    return Err(RuntimeError::Host(format!("unknown task {id}")));
                }
                Traversal {
                    now: 0, increment: 0, destroying: true, target: id,
                    stack: vec![Visit { id, phase: Phase::Unregister }],
                }
            }
        };
        self.drive(traversal, lifecycle, budget)
    }

    fn drive(
        &mut self,
        mut traversal: Traversal,
        lifecycle: &mut impl TaskLifecycle,
        budget: usize,
    ) -> RuntimeResult<bool> {
        let result = self.drive_inner(&mut traversal, lifecycle, budget);
        self.in_callback = false;
        match result {
            Ok(true) => Ok(true),
            other => {
                // Preserve the exact callback phase on yield or failure;
                // successful earlier notifications/releases never repeat.
                self.pending = Some(traversal);
                other
            }
        }
    }

    fn drive_inner(
        &mut self,
        traversal: &mut Traversal,
        lifecycle: &mut impl TaskLifecycle,
        budget: usize,
    ) -> RuntimeResult<bool> {
        for _ in 0..budget {
            let Some(visit) = traversal.stack.last().copied() else { return Ok(true) };
            let id = visit.id;
            if !self.nodes.contains_key(&id) {
                traversal.stack.pop();
                continue;
            }
            self.in_callback = true;
            let next = match visit.phase {
                Phase::Begin => {
                    lifecycle.begin_update(self, id)?;
                    if let Some(node) = self.nodes.get_mut(&id) {
                        node.clock.update(traversal.now);
                    }
                    Phase::Children(None)
                }
                Phase::Children(cursor) => {
                    if let Some(child) = self.next_child(id, cursor, true) {
                        traversal.stack.last_mut().unwrap().phase = Phase::Children(Some(child));
                        traversal.stack.push(Visit { id: child, phase: Phase::Begin });
                        continue;
                    }
                    Phase::Body
                }
                Phase::Body => {
                    self.sweep(id);
                    if self.nodes[&id].deletion != 0 {
                        Phase::Unregister
                    } else {
                        if !lifecycle.update_body(self, id, traversal.increment, budget)? {
                            return Ok(false);
                        }
                        traversal.stack.pop();
                        continue;
                    }
                }
                Phase::Unregister => {
                    self.unregister(id)?;
                    self.nodes.get_mut(&id).unwrap().destroying = true;
                    Phase::NotifyChildren(None)
                }
                Phase::NotifyChildren(cursor) => {
                    if let Some(child) = self.next_child(id, cursor, false) {
                        if self.nodes[&child].deletion == 0 {
                            let command = TaskCommand {
                                target: child, sender: id, code: 3, data: [0; 3],
                            };
                            if !lifecycle.request_delete(self, command, budget)? {
                                return Ok(false);
                            }
                            if let Some(node) = self.nodes.get_mut(&child) {
                                if node.deletion == 0 { node.deletion = 1; }
                            }
                        }
                        Phase::NotifyChildren(Some(child))
                    } else {
                        Phase::NotifySelf
                    }
                }
                Phase::NotifySelf => {
                    if self.nodes[&id].deletion == 0 {
                        let command = TaskCommand { target: id, sender: id, code: 3, data: [0; 3] };
                        if !lifecycle.request_delete(self, command, budget)? {
                            return Ok(false);
                        }
                        if let Some(node) = self.nodes.get_mut(&id) {
                            if node.deletion == 0 { node.deletion = 1; }
                        }
                    }
                    Phase::DestroyChildren(None)
                }
                Phase::DestroyChildren(cursor) => {
                    if let Some(child) = self.next_child(id, cursor, false) {
                        traversal.stack.last_mut().unwrap().phase = Phase::DestroyChildren(Some(child));
                        traversal.stack.push(Visit { id: child, phase: Phase::Unregister });
                        continue;
                    }
                    Phase::PrepareRelease
                }
                Phase::PrepareRelease => {
                    lifecycle.prepare_release(self, id)?;
                    Phase::Release
                }
                Phase::Release => {
                    lifecycle.release(self, id)?;
                    self.nodes.remove(&id);
                    if id == self.root { self.closed = true; }
                    traversal.stack.pop();
                    continue;
                }
            };
            traversal.stack.last_mut().unwrap().phase = next;
        }
        Ok(traversal.stack.is_empty())
    }

    /// Direct commands share one mutable packet and output slot across all
    /// receivers. Consuming a command stops only that receiver's child branch.
    pub fn dispatch(
        &mut self,
        from: EngineTaskId,
        command: &mut TaskCommand,
        output: &mut u32,
        handler: &mut impl FnMut(&mut TaskTree, EngineTaskId, &mut TaskCommand, &mut u32) -> RuntimeResult<bool>,
    ) -> RuntimeResult<bool> {
        let Some(target) = self.route(from, command.target) else { return Ok(false) };
        self.dispatch_branch(target, command, output, handler)?;
        Ok(true)
    }

    fn dispatch_branch(
        &mut self,
        id: EngineTaskId,
        command: &mut TaskCommand,
        output: &mut u32,
        handler: &mut impl FnMut(&mut TaskTree, EngineTaskId, &mut TaskCommand, &mut u32) -> RuntimeResult<bool>,
    ) -> RuntimeResult<()> {
        if !handler(self, id, command, output)? || command.data[2] == 0 {
            return Ok(());
        }
        let mut cursor = None;
        while let Some(child) = self.next_child(id, cursor, true) {
            cursor = Some(child);
            self.dispatch_branch(child, command, output, handler)?;
        }
        Ok(())
    }

    /// Dispatch the inner packet of commands 11/12. Broadcast traversal is
    /// independent of each object's propagation result and its type match.
    pub fn broadcast(
        &mut self,
        from: EngineTaskId,
        command: TaskCommand,
        object_type: Option<u32>,
        output: &mut u32,
        handler: &mut impl FnMut(&mut TaskTree, EngineTaskId, &mut TaskCommand, &mut u32) -> RuntimeResult<bool>,
    ) -> RuntimeResult<bool> {
        if !self.nodes.contains_key(&from) {
            return Err(RuntimeError::Host(format!("unknown broadcasting task {from}")));
        }
        let mut current = from;
        let start = loop {
            let node = &self.nodes[&current];
            if node.id == command.target {
                break current;
            }
            if let Some(branch) = node.index.get(&command.target).copied().flatten() {
                break branch;
            }
            let Some(parent) = node.parent else { break self.root };
            // Forwarding into the root uses its whole-tree broadcast entry.
            // A broadcast originating at the root searched its index above.
            if parent == self.root {
                break self.root;
            }
            current = parent;
        };
        self.broadcast_branch(start, command, object_type, output, handler)
    }

    fn broadcast_branch(
        &mut self,
        id: EngineTaskId,
        command: TaskCommand,
        object_type: Option<u32>,
        output: &mut u32,
        handler: &mut impl FnMut(&mut TaskTree, EngineTaskId, &mut TaskCommand, &mut u32) -> RuntimeResult<bool>,
    ) -> RuntimeResult<bool> {
        let Some(node) = self.nodes.get(&id) else { return Ok(true) };
        let matches = object_type.is_none_or(|object_type| node.object_type == object_type);
        let mut result = true;
        if matches {
            let mut packet = command;
            packet.target = id;
            packet.data[2] = 0;
            result = handler(self, id, &mut packet, output)?;
        }
        let mut cursor = None;
        while let Some(child) = self.next_child(id, cursor, true) {
            cursor = Some(child);
            result = self.broadcast_branch(child, command, object_type, output, handler)?;
        }
        Ok(result)
    }
}
