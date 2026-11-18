//! Concrete instance containers/groups; no renderer or VM callback adapter.
//! Node preorder, parent-owned groups and two-phase pool release follow the
//! pinned format reference. A newly created child enables stepping at age0.

use super::{Effect, Error, force::ForceState, transform::Matrix};
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub(super) struct Instance {
    pub age: f32,
    pub world: Matrix,
    pub render_age: f32,
    parent_matrix: Matrix,
    generation: Matrix,
    force: ForceState,
    parent: Option<usize>,
    stepped: bool,
    state: u8, // 0 active, 1 removing, 2 removed; released at next group update.
    groups: Vec<u64>,
    generated: Vec<usize>,
    next: Vec<f32>,
}
#[derive(Debug, Clone)]
struct Group {
    referenced: bool,
    instances: Vec<usize>,
}
#[derive(Debug, Clone)]
pub(super) struct Tree {
    containers: Vec<BTreeMap<u64, Group>>,
    instances: BTreeMap<usize, Instance>,
    next_instance: usize,
    next_group: u64,
}
impl Tree {
    pub fn new(effect: &Effect, position: [f32; 3]) -> Self {
        let mut tree = Self {
            containers: effect.nodes.iter().map(|_| BTreeMap::new()).collect(),
            instances: BTreeMap::new(),
            next_instance: 1,
            next_group: 1,
        };
        let root = tree.initialize(effect, 0, None, Matrix::translation(position), 0);
        tree.instances.insert(0, root);
        tree.containers[0].insert(
            0,
            Group {
                referenced: true,
                instances: vec![0],
            },
        );
        tree
    }
    fn initialize(
        &mut self,
        effect: &Effect,
        node: usize,
        parent: Option<usize>,
        matrix: Matrix,
        number: usize,
    ) -> Instance {
        let n = &effect.nodes[node];
        let mut groups = Vec::new();
        for &child in &n.children {
            let id = self.next_group;
            self.next_group += 1;
            self.containers[child].insert(
                id,
                Group {
                    referenced: true,
                    instances: Vec::new(),
                },
            );
            groups.push(id);
        }
        Instance {
            age: 0.0,
            render_age: 0.0,
            world: if parent.is_none() {
                matrix
            } else {
                Matrix::IDENTITY
            },
            parent_matrix: matrix,
            generation: n.spawn.matrix(number),
            // Initialization captures the full parent's position regardless
            // of the selected T/R/S binding (PE def0a..def48).
            force: ForceState::new(parent.map_or(matrix.0[3], |id| self.instances[&id].world.0[3])),
            parent,
            stepped: false,
            state: 0,
            groups,
            generated: vec![0; n.children.len()],
            next: n.children.iter().map(|&c| effect.nodes[c].offset).collect(),
        }
    }
    pub fn pooled(&self) -> usize {
        self.instances.len().saturating_sub(1)
    }
    pub fn finished(&self, effect: &Effect) -> bool {
        self.instances.len() == 1
            && effect.nodes[0].children.iter().all(|&c| {
                let n = &effect.nodes[c];
                self.instances[&0].age >= n.interval * (n.generations as f32 - 1.0) + n.offset + 1.0
            })
    }
    pub fn advance(
        &mut self,
        effect: &Effect,
        root: Matrix,
        pooled: &mut usize,
    ) -> Result<(), Error> {
        self.instances.get_mut(&0).unwrap().world = root;
        // Containers are parsed in preorder, not flattened by individual
        // parent. All groups of a node update before any child container.
        for node in 0..effect.nodes.len() {
            let group_ids: Vec<_> = self.containers[node].keys().copied().collect();
            for group_id in group_ids {
                let ids = self.containers[node][&group_id].instances.clone();
                for id in &ids {
                    let state = self.instances[id].state;
                    if state == 2 {
                        self.instances.remove(id);
                        *pooled = pooled.saturating_sub(1);
                    } else if state == 1 {
                        self.instances.get_mut(id).unwrap().state = 2;
                    }
                }
                self.containers[node]
                    .get_mut(&group_id)
                    .unwrap()
                    .instances
                    .retain(|i| self.instances.contains_key(i));
                for id in ids {
                    if self.instances.get(&id).is_some_and(|i| i.state == 0) {
                        self.update_instance(effect, node, id, pooled)?;
                    }
                }
            }
            self.containers[node].retain(|_, g| g.referenced || !g.instances.is_empty());
        }
        Ok(())
    }
    fn update_instance(
        &mut self,
        effect: &Effect,
        node: usize,
        id: usize,
        pooled: &mut usize,
    ) -> Result<(), Error> {
        let n = &effect.nodes[node];
        let mut p = self.instances[&id].clone();
        let original_age = p.age;
        p.render_age = p.age;
        if node != 0 {
            if let Some(parent_id) = p.parent {
                let parent = self
                    .instances
                    .get(&parent_id)
                    .ok_or(Error::Host("lost SKFE parent before detach"))?;
                p.parent_matrix =
                    p.parent_matrix
                        .bind(parent.world, self.instances[&0].world, n.bind)?;
                if parent.state != 0 {
                    p.parent = None;
                }
            }
            let local = n.translation.value(p.age, n.life)?;
            let t = std::array::from_fn(|i| {
                local[i]
                    + if n.effects_rotation {
                        0.0
                    } else {
                        p.generation.0[3][i]
                    }
            });
            p.world = Matrix::scale(n.scaling.value(p.age, n.life)?)
                .then(n.rotation.matrix(p.age, n.life)?)
                .then(Matrix::translation(t));
            if n.effects_rotation {
                p.world = p.world.then(p.generation);
            }
            p.world = p.world.then(p.parent_matrix);
            // Play d9fdd..d9feb initializes InstanceGlobal target XYZ to0;
            // type61 exposes no target setter. Absolute controllers operate
            // after parent binding, with the same1-frame manager update.
            let correction = p
                .force
                .sample(&n.absolute, p.world.0[3], [0.0; 3], p.age, 1.0)?;
            for i in 0..3 {
                p.world.0[3][i] += correction[i];
            }
            if p.world.0.iter().flatten().any(|v| !v.is_finite()) {
                return Err(Error::Host("nonfinite SKFE instance transform"));
            }
        }
        if p.stepped {
            p.age += 1.0;
        }
        // Children initialized below must see the current matrix/time, even
        // if this parent is killed later in the same update.
        self.instances.insert(id, p.clone());
        if p.stepped && (original_age <= n.life || !n.remove_life) {
            for (slot, &child) in n.children.iter().enumerate() {
                let child_node = &effect.nodes[child];
                while p.generated[slot] < child_node.generations
                    && original_age + 1.0 > p.next[slot]
                {
                    // Native pool exhaustion retries this generation later;
                    // it does not advance the sequence or invent a particle.
                    if *pooled >= 2000 {
                        break;
                    }
                    let parent_matrix = Matrix::capture(p.world, child_node.bind)?;
                    let instance =
                        self.initialize(effect, child, Some(id), parent_matrix, p.generated[slot]);
                    let new_id = self.next_instance;
                    self.next_instance =
                        self.next_instance.checked_add(1).ok_or(Error::Capacity)?;
                    self.instances.insert(new_id, instance);
                    self.containers[child]
                        .get_mut(&p.groups[slot])
                        .unwrap()
                        .instances
                        .push(new_id);
                    p.generated[slot] += 1;
                    p.next[slot] += child_node.interval.max(0.0);
                    *pooled += 1;
                }
            }
        }
        let children_extinct = n.remove_children
            && n.children.iter().enumerate().all(|(slot, &child)| {
                let c = &effect.nodes[child];
                p.age >= c.interval * (c.generations as f32 - 1.0) + c.offset + 1.0
                    && self.containers[child][&p.groups[slot]]
                        .instances
                        .iter()
                        .all(|i| self.instances[i].state != 0)
            });
        if node != 0
            && ((n.remove_life && p.age > n.life)
                || (n.remove_parent && p.parent.is_none())
                || children_extinct)
        {
            p.state = 1;
            for (slot, &child) in n.children.iter().enumerate() {
                self.containers[child]
                    .get_mut(&p.groups[slot])
                    .unwrap()
                    .referenced = false;
            }
        } else {
            p.stepped = true;
        }
        self.instances.insert(id, p);
        Ok(())
    }
    pub fn visible_groups<'a>(&'a self, effect: &'a Effect) -> Vec<(usize, Vec<&'a Instance>)> {
        let mut out = Vec::new();
        for (node, n) in effect.nodes.iter().enumerate() {
            let Some(s) = &n.renderer else {
                continue;
            };
            for g in self.containers[node].values() {
                let mut visible: Vec<_> = g
                    .instances
                    .iter()
                    .filter_map(|i| {
                        let p = &self.instances[i];
                        (p.state == 0).then_some(p)
                    })
                    .collect();
                if s.order == 1 {
                    visible.reverse();
                }
                if !visible.is_empty() {
                    out.push((node, visible));
                }
            }
        }
        out
    }
}
