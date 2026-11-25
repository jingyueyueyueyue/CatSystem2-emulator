//! Model descendants of an existing FES bank's concrete lower tasks.
//! The task parent graph is the ownership evidence; names and PL are not.
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Context, Result, bail};
use super::LoggingHost;

#[derive(Clone, Copy, PartialEq, Eq, cs2_runtime::graph_serde::Serialize,
    cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
enum Backend { Mdl, Dae, Utate }

#[derive(Clone, cs2_runtime::graph_serde::Serialize,
    cs2_runtime::graph_serde::Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(crate) struct ModelTaskRelease {
    // Actual lower-task root through model task, independent of graph nodes
    // inside an MDL/DAE resource. No extra model/pixel store is introduced.
    path: Vec<u32>,
    backend: Backend,
}

fn backend(host: &LoggingHost<'_>, id: u32) -> Result<Option<Backend>> {
    let mdl = host.data_services.particle_external.canvases.contains_key(&id);
    let dae = host.data_services.css_models.owners.contains_key(&id);
    let utate = host.data_services.css_models.utate_models.contains_key(&id);
    if !mdl && !dae && !utate { return Ok(None); }
    if usize::from(mdl)+usize::from(dae)+usize::from(utate)>1 {
        bail!("model task has conflicting concrete consumers");
    }
    let (kind, _, _) = host.data_services.tasks.message_task_state(id)
        .context("model release consumer has no real task record")?;
    if kind != 42 || host.scene_nodes.get(&id).is_none_or(|node| node.kind != kind) {
        bail!("model release consumer differs from its actual task kind");
    }
    // Ordinary Release already removes these concrete stores by task ID.
    // The factory's wrapper profile is unnecessary once ownership is known.
    Ok(Some(if utate { Backend::Utate } else if mdl { Backend::Mdl } else { Backend::Dae }))
}

/// Capture only models actually below this bank's already-resolved task ids.
/// Empty roots mean no task ownership, never an inferred named model bank.
pub(crate) fn plan_model_descendants(
    host: &LoggingHost<'_>, roots: &[u32],
) -> Result<Vec<ModelTaskRelease>> {
    if roots.is_empty() { return Ok(Vec::new()); }
    let mut children: BTreeMap<u32, Vec<u32>> = BTreeMap::new();
    for (&child, &parent) in host.task_parents.iter() {
        if child != parent { children.entry(parent).or_default().push(child); }
    }
    for values in children.values_mut() { values.sort_by_key(|id| *id as i32); }
    let mut plan = Vec::new();
    let mut visited = BTreeSet::new();
    for &root in roots {
        if root == 0 { bail!("model bank release cannot own the ordinary root"); }
        visit(host, &children, root, &mut Vec::new(), &mut visited, &mut plan)?;
    }
    Ok(plan)
}

fn visit(
    host: &LoggingHost<'_>, children: &BTreeMap<u32, Vec<u32>>, id: u32,
    path: &mut Vec<u32>, visited: &mut BTreeSet<u32>, plan: &mut Vec<ModelTaskRelease>,
) -> Result<()> {
    if path.contains(&id) { bail!("model release task ancestry contains a cycle"); }
    if !visited.insert(id) { return Ok(()); }
    if visited.len() > 65_536 || path.len() >= 256 {
        bail!("model release task subtree exceeds the ordinary ownership limits");
    }
    path.push(id);
    if let Some(children_ids) = children.get(&id) {
        for &child in children_ids { visit(host, children, child, path, visited, plan)?; }
    }
    if let Some(backend) = backend(host, id)? {
        plan.push(ModelTaskRelease { path: path.clone(), backend });
    }
    path.pop();
    Ok(())
}

/// Accept real deletion requests in captured child-first order. Destruction
/// remains in the ordinary Unregister/DestroyChildren/Release continuation:
/// release_canvas/release, model_state_trees, bindings and image wrappers.
/// In particular, do not clear a live manager before child delete callbacks.
pub(crate) fn request_model_descendants(
    host: &mut LoggingHost<'_>, plan: &[ModelTaskRelease],
) -> Result<()> {
    // Validate the entire retained group before the first state change.
    for item in plan {
        let &id = item.path.last().context("model release identity has an empty task path")?;
        let current = backend(host, id)?;
        if current.is_none() {
            if host.data_services.tasks.message_task_state(id).is_some()
                || host.scene_nodes.contains_key(&id) || host.task_parents.contains_key(&id) {
                bail!("planned model lost its consumer while its task is still alive");
            }
            continue; // The real task destructor already completed this id.
        }
        if current != Some(item.backend) || item.path.windows(2).any(|pair|
            host.task_parents.get(&pair[1]) != Some(&pair[0])) {
            bail!("planned model task changed its consumer or bank ancestry");
        }
    }
    for item in plan {
        let id = *item.path.last().context("model release identity has an empty task path")?;
        if backend(host, id)?.is_none() { continue; }
        host.request_ordinary_task_delete(id)?;
        // Main and offscreen consumers both read these actual scene flags.
        // Retain meshes/textures until the existing postorder destructor.
        if let Some(node) = host.scene_nodes.get_mut(&id) {
            node.visible = false;
            node.initialized = false;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HeadlessPlayer, tests::TempDir};

    #[test]
    fn model_bank_release_uses_live_kind42_owners_without_an_executable_profile() -> Result<()> {
        let root = TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(&root.0, root.0.join("profile"))?;
        player.with_host(|host| {
            assert!(host.css_task_sha().is_none());
            assert!(!host.data_services.particle_external.legacy_kc_view());
            let bank = host.create_task_object(0, 0, b"bank", &[])?;
            let mdl = host.create_task_object(42, bank, b"model", &[])?;
            let dae = host.create_task_object(42, mdl, b"model", &[])?;
            let kxs = host.create_task_object(42, bank, b"model", &[])?;
            let sibling = host.create_task_object(42, 0, b"unrelated model", &[])?;
            host.data_services.particle_external.canvases.entry(mdl).or_default();
            host.data_services.css_models.register(dae);
            host.data_services.css_models.register_utate(kxs);
            host.data_services.css_models.register(sibling);

            let plan = plan_model_descendants(host, &[bank])?;
            assert_eq!(plan.iter().map(|item| item.path.as_slice()).collect::<Vec<_>>(),
                [&[bank, mdl, dae][..], &[bank, mdl][..], &[bank, kxs][..]]);

            host.data_services.css_models.register(kxs);
            assert!(request_model_descendants(host, &plan).unwrap_err().to_string().contains("conflicting"));
            for id in [mdl, dae, kxs] {
                assert_eq!(host.data_services.tasks.message_task_state(id).unwrap().1, 0);
            }
            host.data_services.css_models.owners.remove(&kxs);

            request_model_descendants(host, &plan)?;
            for id in [mdl, dae, kxs] {
                assert_eq!(host.data_services.tasks.message_task_state(id).unwrap().1, 1);
                assert!(!host.scene_nodes[&id].visible);
                assert!(backend(host, id)?.is_some());
            }
            assert!(host.update_ordinary_children(0, 1, 1, 4096)?);
            for id in [mdl, dae, kxs] {
                assert!(!host.scene_nodes.contains_key(&id));
                assert!(backend(host, id)?.is_none());
            }
            assert!(host.scene_nodes.contains_key(&bank));
            assert!(matches!(backend(host, sibling)?, Some(Backend::Dae)));
            Ok(())
        })
    }
}
