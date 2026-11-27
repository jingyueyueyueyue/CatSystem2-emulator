//! Final portable graph consumer. Original opaque kind43 state-tree nodes are
//! deliberately not decoded as this format. A host may opt a real task into
//! the portable receiver; no task is fabricated and no original null factory
//! profile changes. Preparation constructs an independent replacement graph.
use std::collections::{BTreeMap, BTreeSet, VecDeque};
use anyhow::Context;
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use cs2_renderer::{Canvas, RgbaFrame};
use cs2_runtime::object_graph::{self as wire, ExternalBindingSnapshot as Binding,
    ImageFrameSnapshot, ImageResourceSnapshot, ObjectGraphSnapshot, SceneNodeSnapshot,
    TaskRecordSnapshot, VmOwner, SUPPORTED_TASK_KINDS};
use cs2_script::{NativeReturn, Result, SharedExternalStorage, Vm, VmSnapshotGroup};
use crate::{DesktopImageResource, HeadlessPlayer, LoggingHost, PendingFesInput,
    PlayerFramePhase, RandomStream, SceneNode, ScriptRandom, ScriptTextEncoding,
    native_state_error, read_u32};
use super::{MessageTraversal, NativeTaskStore, Record, RootTaskDispatch, Traversal};

#[derive(Default)]
pub(super) struct GraphRequests {
    receivers: BTreeSet<u32>,
    pending: Option<Request>,
    receipt: Option<Request>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Request { caller: u32, index: u32, container: u32, record: u32, target: u32 }

pub(crate) struct ObjectGraphReceiver;
impl ObjectGraphReceiver {
    pub(crate) fn supports(index: u32) -> bool { matches!(index, 688 | 689) }
}

impl NativeTaskStore {
    pub(crate) fn object_graph_request_pending(&self) -> bool { self.graph.pending.is_some() }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct HostContinuation {
    version: u32, phase: u32, particle_cursor: Option<i32>,
    traversal: Option<Traversal>, direct_release: Option<Traversal>,
    root_dispatch: Option<RootTaskDispatch>, messages: BTreeMap<u32, MessageTraversal>,
    released: VecDeque<u32>, receivers: BTreeSet<u32>, receipt: Option<Request>,
    loaded: Option<(String, String, usize, String)>, sequence_mode: u32,
}

struct PreparedGraph {
    saved: ObjectGraphSnapshot, host: HostContinuation,
    tasks: NativeTaskStore, root_vm: Option<Vm>, attached_vms: BTreeMap<u32, Vm>,
    values: Vec<u32>, nodes: BTreeMap<u32, SceneNode>,
    images: BTreeMap<u32, RgbaFrame>, resources: BTreeMap<u32, DesktopImageResource>,
    resource_layers: crate::image_manager::PreparedResourceLayers,
    image_lists: crate::list_native::PreparedImageLists,
    canvas: Canvas, graphics: Canvas,
    data: crate::cst_state::PreparedScriptDataObjects,
    backlogs: crate::cst_state::PreparedBacklogs,
    input: super::PreparedInputSamplingState,
    allocator: cs2_runtime::object_state::PreparedObjectAllocator,
    export: super::PreparedImageExportState,
    fes_tasks: crate::fes_tasks::state::PreparedFesTaskState,
}

impl HeadlessPlayer {
    /// This is an explicit portable-format adapter on an existing VM task,
    /// not an assertion that the original kind43 receiver is recovered.
    pub fn install_portable_object_graph_receiver(&mut self, target: u32) -> anyhow::Result<()> {
        let record = self.data_services.tasks.records.get(&target)
            .ok_or_else(|| anyhow::anyhow!("portable graph receiver task is absent"))?;
        if !matches!(record.kind, 0 | 12) || record.deletion != 0 {
            anyhow::bail!("portable graph receiver requires a live ordinary VM task");
        }
        self.data_services.tasks.graph.receivers.insert(target);
        Ok(())
    }

    /// Actual unsupported task families, counted from live records. Callers
    /// can present this before save; an error never collects or drops them.
    pub fn object_graph_unsupported_task_families(&self) -> BTreeMap<u32, usize> {
        let mut kinds = BTreeMap::new();
        for (&id, record) in &self.data_services.tasks.records {
            if !SUPPORTED_TASK_KINDS.contains(&record.kind)
                || matches!(record.kind, 41 | 42)
                    && self.data_services.fes_controls.checkpoint_lower_owner(id).is_none() {
                *kinds.entry(record.kind).or_default() += 1;
            }
        }
        kinds
    }

    fn check_object_graph_boundary(&self) -> anyhow::Result<()> {
        self.check_input_callbacks_for_graph()?;
        if self.data_services.emote.has_pending_loads() {
            anyhow::bail!("host graph is waiting for asynchronous E-mote material preparation");
        }
        if self.data_services.image_manager.has_pending_image_loads() {
            anyhow::bail!("cannot capture an object graph while image material preparation is pending");
        }
        let unsupported = self.object_graph_unsupported_task_families();
        if !unsupported.is_empty() {
            anyhow::bail!("host graph has {} unsupported task families: {:?}", unsupported.len(), unsupported);
        }
        let tasks = &self.data_services.tasks;
        if tasks.executing_external.is_some()
            || tasks.records.values().any(|r| r.active || r.loading.is_some())
        {
            anyhow::bail!("host graph cannot freeze a borrowed task callback or pending object load");
        }
        if matches!(self.frame_phase, PlayerFramePhase::Input | PlayerFramePhase::Configuration)
            || self.data_services.language.graph_has_continuation()
        { anyhow::bail!("host graph has an unsupported input/configuration continuation; live cursors retained"); }
        let data = &self.data_services;
        // Validate actual driver/notification/owned-buffer state, not only
        // the absence of an opaque guard. Commit restores these same owners.
        self.capture_fes_task_state()?;
        self.check_plane_visuals_graph_boundary()?;
        // The FES transaction above has validated the complete bank+physical
        // particle component; only that exact opaque count is now accounted.
        let opaque = data.graph_opaque_data_count()
            .checked_sub(data.fes_particles.graph_object_count())
            .ok_or_else(|| anyhow::anyhow!("particle checkpoint opaque accounting differs"))?
            + data.ordinary.graph_unsupported_object_count()
            + data.files.graph_open_count()
            + tasks.model_state_trees.len() + data.css_models.utate_models.len();
        if opaque != 0 {
            anyhow::bail!("host graph has {opaque} unsupported standalone objects (file/preload/Effekseer/model/model-state-tree)");
        }
        if self.fes.is_some() || self.cst.is_some() || !self.cst_images.is_empty()
            || self.image_frame.is_some()
            || !self.motion_controllers.is_empty() || !self.motion_snapshots.is_empty()
            || !self.movie_playbacks.is_empty() || !self.movie_resources.is_empty()
            || self.state.movie.current.is_some()
            || !data.messages.objects().is_empty()
        { anyhow::bail!("host graph has an unsupported media/text/tile runtime; resources and live continuations retained"); }
        if !tasks.records.contains_key(&self.current_task_id) {
            anyhow::bail!("host graph needs a registered root task; run its normal initialization first");
        }
        Ok(())
    }

    pub fn capture_object_graph(&self) -> anyhow::Result<ObjectGraphSnapshot> {
        self.capture_object_graph_inner(None)
    }

    fn capture_object_graph_inner(&self, save_receipt: Option<Request>) -> anyhow::Result<ObjectGraphSnapshot> {
        self.check_object_graph_boundary()?;
        self.data_services.image_manager.validate_resource_layer_owners(&self.image_resources)?;
        let tasks = &self.data_services.tasks;
        let mut owners = BTreeMap::new();
        let mut programs = BTreeMap::new();
        if let Some(vm) = &self.vm {
            owners.insert(self.current_task_id, VmOwner::Root);
            programs.insert(self.current_task_id, vm);
        }
        for (&id, vm) in &self.task_vms {
            if owners.insert(id, VmOwner::Attached).is_some() {
                anyhow::bail!("task {id} has multiple actual VM owners");
            }
            programs.insert(id, vm);
        }
        for (&id, record) in &tasks.records {
            if let Some(vm) = record.ordinary.as_ref().and_then(|task| task.vm.as_ref()) {
                if owners.insert(id, VmOwner::Ordinary).is_some() {
                    anyhow::bail!("task {id} has multiple actual VM owners");
                }
                programs.insert(id, vm);
            }
        }
        let group = VmSnapshotGroup::capture(programs.iter().map(|(&id, &vm)| (id, vm)))?;
        let mut detached = Vec::new();
        let mut bindings = Vec::<SharedExternalStorage>::new();
        let mut reference = |storage: &SharedExternalStorage| -> Result<Binding> {
            for (&id, vm) in &programs {
                if storage.same_storage(&vm.external_binding()) { return Ok(Binding::Vm(id)); }
            }
            if let Some(index) = bindings.iter().position(|other| storage.same_storage(other)) {
                return Ok(Binding::Detached(index));
            }
            let index = detached.len();
            detached.push(storage.snapshot()?); bindings.push(storage.clone());
            Ok(Binding::Detached(index))
        };
        let factory_external = tasks.factory_external.as_ref().map(&mut reference).transpose()?;
        let records = tasks.records.iter().map(|(&id, r)| Ok((id, TaskRecordSnapshot {
            kind: r.kind, clock: r.clock.clone(), deletion: r.deletion,
            ordinary: r.ordinary.as_ref().map(|task| task.execution_snapshot()).transpose()?,
            queued: r.queued.iter().copied().collect(), context: r.context.clone(), had_vm: r.had_vm,
            property: r.property, external: r.external.as_ref().map(&mut reference).transpose()?,
            language: r.language_notifications.clone(), shared_area: r.shared_area, registered: r.registered,
        }))).collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let mut pixels = BTreeMap::new();
        self.image_export_state_snapshot()?;
        for (&wrapper, &pixel) in &self.data_services.image_wrapper_pixels {
            pixels.entry(pixel).or_insert_with(|| capture_frame(&self.image_handles[&wrapper]));
        }
        let host = HostContinuation {
            version: 1, phase: phase_number(&self.frame_phase), particle_cursor: self.particle_frame_cursor,
            traversal: tasks.traversal.clone(), direct_release: tasks.direct_release.clone(),
            root_dispatch: tasks.root_dispatch, messages: tasks.message_traversals.clone(),
            released: tasks.released.clone(), receivers: tasks.graph.receivers.clone(),
            receipt: save_receipt.or_else(|| tasks.graph.receipt.clone()),
            loaded: self.loaded.as_ref().map(|entry| (entry.name.clone(), entry.kind.clone(), entry.byte_length, entry.detail.clone())),
            sequence_mode: self.data_services.ordinary.sequence_name_mode,
        };
        let saved = ObjectGraphSnapshot {
            version: 4, pe_sha256: self.graph_pe_sha().to_owned(), root: self.current_task_id,
            next_task_id: self.next_task_id, host_continuation: self.encode_fes_graph_host(wire::encode_component(&host)?)
                .context("encode host graph continuation")?,
            external_size: self.external_size, configured_shared_slots: self.configured_shared_slots,
            records, reserved: tasks.reserved.clone(), external_tasks: tasks.external_tasks.clone(),
            // History proxies are drawable object identities, not task-tree
            // nodes. The plane component captures their complete live state.
            parents: self.task_parents.clone(), nodes: self.scene_nodes.iter()
                .filter(|(id, _)| self.data_services.planes.history_owner(**id).is_none())
                .map(|(&id, n)| (id, capture_node(n))).collect(),
            programs: group, vm_owners: owners, detached_external: detached, factory_external,
            numeric: self.shared_numeric_state_snapshot()?, scene_memory: self.scene_message_memory_snapshot()?,
            input: self.capture_input_sampling_for_graph()?, allocator: self.object_allocator_snapshot(),
            top_clock: [self.data_services.top_clock.requested, self.data_services.top_clock.sampled],
            state: self.state.clone(), image_pixels: pixels,
            wrapper_pixels: self.data_services.image_wrapper_pixels.clone(),
            wrapper_owners: self.data_services.image_wrapper_owners.clone(),
            image_control: self.data_services.image_control_values.clone(),
            image_export: self.image_export_state_snapshot()?,
            image_resources: self.image_resources.iter().map(|(&id, r)| (id, ImageResourceSnapshot {
                image: capture_frame(&r.image), owner: r.owner, references: r.references,
                has_source_layer: r.has_source_layer, updated: r.updated, uses_alpha: r.uses_alpha,
                auxiliary: r.auxiliary, attributes: r.attributes.clone(),
                source_layers: self.data_services.image_manager.resource_layers_snapshot(id),
            })).collect(),
            message_defaults: self.data_services.messages.defaults.clone(),
            image_lists: self.image_list_state_snapshot()?,
            back_buffer: capture_canvas(&self.canvas)?, graphics_canvas: capture_canvas(&self.graphics_canvas)?,
            data: self.script_data_objects_snapshot()?, backlogs: self.backlog_snapshots()?,
            emocon: self.emocon_state_snapshot(),
            pending_messages: self.pending_messages.iter().copied().collect(),
            pending_fes_events: self.pending_fes_events.iter().map(|event| (event.code, event.physical.clone())).collect(),
            message_wait_address: self.message_wait_address, current_parameter: self.current_parameter,
            menu_items: self.menu_item_requests.clone(), debug_mode: self.debug_mode,
            sound_config_enabled: self.sound_config_enabled, auto_select: self.auto_select,
            system_config: self.system_config_blocks.clone(), encoding: match self.script_encoding {
                ScriptTextEncoding::Cp932 => 0, ScriptTextEncoding::Gbk => 1, ScriptTextEncoding::Utf8 => 2 },
            random_integer: self.script_random.integer.values.to_vec(), random_float: self.script_random.floating.values.to_vec(),
            random_indices: [self.script_random.integer.index, self.script_random.floating.index],
            languages: self.data_services.language.languages.clone(), language_dirty: self.data_services.language.system_settings_dirty,
        };
        // Capture validates the same full replacement which restore accepts,
        // including pointer-domain collisions and saved scheduler cursors.
        let prepared = self.prepare_object_graph(saved).context("validate captured host graph")?;
        Ok(prepared.saved)
    }

    fn graph_pe_sha(&self) -> &str {
        self.scan.engine.primary_executable.as_ref().map_or("", |binary| binary.sha256.as_str())
    }

    /// Real final CSS consumer: encode fully before touching the container.
    pub fn save_object_graph_to_container(&mut self, handle: u32, record: u32) -> anyhow::Result<()> {
        let bytes = self.capture_object_graph()?.encode()?;
        self.append_graph_record(handle, record, &bytes)
    }

    fn append_graph_record(&mut self, handle: u32, record: u32, bytes: &[u8]) -> anyhow::Result<()> {
        let file = self.css_handles.get_mut(&handle).ok_or_else(|| anyhow::anyhow!("graph destination CSS handle is absent"))?;
        file.append_host_graph(record, bytes)?;
        Ok(())
    }

    pub fn restore_object_graph_from_container(&mut self, handle: u32, record: u32) -> anyhow::Result<()> {
        self.check_object_graph_boundary()?;
        let bytes = self.css_handles.get(&handle).ok_or_else(|| anyhow::anyhow!("graph source CSS handle is absent"))?
            .try_record(record)?.ok_or_else(|| anyhow::anyhow!("graph source CSS record is absent"))?;
        let prepared = self.prepare_object_graph(ObjectGraphSnapshot::decode(bytes)?)?;
        self.commit_object_graph(prepared);
        Ok(())
    }

    pub fn restore_object_graph_bytes(&mut self, bytes: &[u8]) -> anyhow::Result<()> {
        self.check_object_graph_boundary()?;
        let saved = ObjectGraphSnapshot::decode(bytes)?;
        let prepared = self.prepare_object_graph(saved)?;
        self.commit_object_graph(prepared);
        Ok(())
    }

    fn prepare_object_graph(&self, saved: ObjectGraphSnapshot) -> anyhow::Result<PreparedGraph> {
        saved.validate_structure()?;
        saved.state.validate()?;
        if saved.pe_sha256.is_empty() || saved.pe_sha256 != self.graph_pe_sha() {
            anyhow::bail!("host graph belongs to another or an unidentified PE profile");
        }
        if saved.external_size > 64 * 1024 * 1024 || saved.configured_shared_slots > 1_000_000
            || saved.system_config.iter().map(Vec::len).collect::<Vec<_>>() != [5628, 72, 32, 12]
        { anyhow::bail!("host graph has invalid external/configuration sizes"); }
        let (host_bytes, fes_bytes) = Self::decode_fes_graph_host(&saved.host_continuation)
            .context("decode host graph envelope")?;
        let host: HostContinuation = wire::decode_component(&host_bytes)
            .context("decode host graph continuation")?;
        validate_host(&host, &saved)?;
        if saved.state.movie.current.is_some() {
            anyhow::bail!("saved graph contains an unsupported active movie object");
        }
        if let Some(defaults) = &saved.message_defaults {
            defaults.configuration(cs2_runtime::message_object::MessageEncoding::Cp932)?;
            defaults.configuration(cs2_runtime::message_object::MessageEncoding::Unicode)?;
        }
        let mut programs = saved.programs.restore()?;
        let detached = saved.detached_external.iter().map(|bytes| SharedExternalStorage::new(bytes.clone()))
            .collect::<Result<Vec<_>>>()?;
        let resolve = |binding: &Binding| -> anyhow::Result<SharedExternalStorage> {
            match binding {
                Binding::Vm(id) => programs.get(id).map(Vm::external_binding),
                Binding::Detached(index) => detached.get(*index).cloned(),
            }.ok_or_else(|| anyhow::anyhow!("graph external binding has no rebuilt storage"))
        };
        let external = saved.records.iter().map(|(&id, r)| Ok((id, r.external.as_ref().map(&resolve).transpose()?)))
            .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let factory = saved.factory_external.as_ref().map(&resolve).transpose()?;
        let owners = saved.records.keys().copied().collect();
        let queue_lengths = saved.records.iter().map(|(&id, r)| (id, r.queued.len())).collect();
        let scene = NativeTaskStore::prepare_scene_message_memory_restore(&saved.scene_memory, &programs, &owners, &queue_lengths)?;
        let mut tasks = NativeTaskStore::default();
        tasks.preserve_graph_buffer_adapters(&self.data_services.tasks);
        for (&id, r) in &saved.records {
            let ordinary_vm = if saved.vm_owners.get(&id) == Some(&VmOwner::Ordinary) { programs.remove(&id) } else { None };
            tasks.records.insert(id, Record {
                kind: r.kind, clock: r.clock.clone(), deletion: r.deletion,
                ordinary: r.ordinary.as_ref().map(|metadata| metadata.restore(ordinary_vm)).transpose()?,
                active: false, queued: r.queued.clone().into(), context: r.context.clone(), had_vm: r.had_vm,
                property: r.property, external: external[&id].clone(), language_notifications: r.language.clone(),
                loading: None, shared_area: r.shared_area, message_owner: None, registered: r.registered,
            });
        }
        let root_vm = if saved.vm_owners.get(&saved.root) == Some(&VmOwner::Root) { programs.remove(&saved.root) } else { None };
        if let Some(address) = saved.message_wait_address {
            root_vm.as_ref().ok_or_else(|| anyhow::anyhow!("saved message-wait receiver has no root VM"))?.read(address, 16)?;
        }
        if programs.keys().any(|id| saved.vm_owners.get(id) != Some(&VmOwner::Attached)) {
            anyhow::bail!("host graph contains an unassigned VM");
        }
        tasks.reserved = saved.reserved.clone(); tasks.external_tasks = saved.external_tasks.clone();
        tasks.factory_external = factory; tasks.traversal = host.traversal.clone();
        tasks.direct_release = host.direct_release.clone(); tasks.root_dispatch = host.root_dispatch;
        tasks.message_traversals = host.messages.clone(); tasks.released = host.released.clone();
        tasks.graph = GraphRequests { receivers: host.receivers.clone(), pending: None, receipt: host.receipt.clone() };
        let mut values = Vec::new();
        tasks.restore_shared_numeric_state(&mut values, saved.numeric.clone())?;
        tasks.commit_scene_message_memory_restore(scene);
        let mut nodes = saved.nodes.iter().map(|(&id, node)| Ok((id, restore_node(node)?))).collect::<anyhow::Result<_>>()?;
        let pixels = saved.image_pixels.iter().map(|(&id, frame)| {
            let image = crate::image_manager::restore_list_frame(frame, self.data_services.image_list_memory)?;
            Ok((id, image))
        }).collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        let images = saved.wrapper_pixels.iter().map(|(&wrapper, &pixel)| {
            let image = pixels.get(&pixel).ok_or_else(|| anyhow::anyhow!("wrapper alias refers to absent image pixels"))?;
            Ok((wrapper, image.clone()))
        }).collect::<anyhow::Result<BTreeMap<_, _>>>()?;
        if saved.image_pixels.keys().any(|id| !saved.wrapper_pixels.values().any(|pixel| pixel == id))
            || saved.wrapper_owners.keys().any(|id| !images.contains_key(id))
            || saved.wrapper_owners.values().any(|id| !saved.records.contains_key(id))
            || saved.image_control.keys().any(|id| !images.contains_key(id))
        { anyhow::bail!("image alias/owner/control state differs from the rebuilt graph"); }
        let mut resources = saved.image_resources.iter().map(|(&id, r)| {
            if id == 0 || r.owner.is_some_and(|owner| !saved.records.contains_key(&owner))
                || r.references == 0 || r.references > 1_000_000 {
                anyhow::bail!("retained image resource has invalid owner/reference count");
            }
            let image = restore_frame(&r.image)?;
            Ok((id, DesktopImageResource { geometry: crate::image_manager::ResourceGeometry::from_frame(&image),
                image, owner: r.owner,
                references: r.references, has_source_layer: r.has_source_layer, updated: r.updated,
                uses_alpha: r.uses_alpha, auxiliary: r.auxiliary, attributes: r.attributes.clone() }))
        }).collect::<anyhow::Result<_>>()?;
        let resource_layers = crate::image_manager::ImageManager::prepare_resource_layers_restore(&saved.image_resources)?;
        let mut image_lists = self.prepare_image_list_state_restore(&saved, &images, &mut resources)?;
        // Reinsert validated proxy identities before allocator preparation or
        // the first live write; never rebuild their history during commit.
        image_lists.prepare_scene_nodes(&mut nodes)?;
        let export = Self::prepare_image_export_state_restore(&saved.image_export, &saved.wrapper_pixels, &images)?;
        let identities = saved.wrapper_pixels.keys().chain(saved.image_pixels.keys()).chain(saved.image_resources.keys())
            .chain(saved.data.contexts.contexts.iter().map(|context| &context.handle))
            .chain(saved.data.containers.iter().map(|file| &file.handle))
            .chain(saved.data.databases.iter().map(|object| &object.handle))
            .chain(saved.data.cst_windows.iter().map(|object| &object.handle))
            .chain(saved.data.ordered_maps.keys()).chain(saved.data.shared_strings.keys())
            .chain(saved.backlogs.keys()).copied()
            .chain(saved.data.standalone_handles())
            .chain(image_lists.object_handles());
        let allocator = Self::prepare_object_allocator_restore(saved.allocator, &saved.programs, identities, &saved.data.raw_blocks)?;
        let data = Self::prepare_script_data_objects_restore(&saved.data)?;
        let backlogs = Self::prepare_backlog_restore(&saved.backlogs)?;
        let input = Self::prepare_input_sampling_state_restore(&saved.input, saved.state.clock.frame,
            &saved.records.iter().map(|(&id, record)| (id, record.kind)).collect())?;
        let canvas = restore_canvas(&saved.back_buffer)?;
        let graphics = restore_canvas(&saved.graphics_canvas)?;
        if [canvas.width(), canvas.height()] != [graphics.width(), graphics.height()]
            || [canvas.width(), canvas.height()] != self.logical_size()
        { anyhow::bail!("graph canvases differ from this session's logical screen size"); }
        // Physical SOUND preparation holds its PCM registry transaction.
        // Prepare it last, validate the paired consumers, then commit/drop
        // synchronously without calling another audio/capture service.
        let fes_tasks = self.prepare_fes_task_state(&fes_bytes, &saved, &tasks, &images)
            .context("prepare host graph FES tasks")?;
        fes_tasks.validate_projection_links(image_lists.checkpoint_lists(), &tasks.checkpoint_task_kinds())?;
        fes_tasks.validate_image_source_links(&image_lists)?;
        Ok(PreparedGraph { saved, host, tasks, root_vm, attached_vms: programs, values, nodes,
            images, resources, resource_layers, image_lists, canvas, graphics, data, backlogs, input, allocator, export, fes_tasks })
    }

    /// No resource reads, VM execution, fallible validation, callbacks or new
    /// object allocations occur after the first live write in this method.
    fn commit_object_graph(&mut self, prepared: PreparedGraph) {
        let PreparedGraph { saved, host, tasks, root_vm, attached_vms, values, nodes,
            images, resources, resource_layers, image_lists, canvas, graphics, data, backlogs, input, allocator, export, fes_tasks } = prepared;
        self.commit_object_allocator_restore(allocator);
        self.data_services.tasks = tasks;
        self.vm = root_vm; self.task_vms = attached_vms;
        self.current_task_id = saved.root; self.next_task_id = saved.next_task_id;
        self.task_parents = saved.parents; self.scene_nodes = nodes; self.shared_values = values;
        self.external_size = saved.external_size; self.configured_shared_slots = saved.configured_shared_slots;
        self.state = saved.state;
        self.data_services.top_clock.requested = saved.top_clock[0];
        self.data_services.top_clock.sampled = saved.top_clock[1];
        self.frame_phase = restore_phase(host.phase); self.particle_frame_cursor = host.particle_cursor;
        self.loaded = host.loaded.map(|(name, kind, byte_length, detail)| crate::LoadedEntry { name, kind, byte_length, detail });
        self.data_services.ordinary.sequence_name_mode = host.sequence_mode;
        self.image_handles = images; self.image_resources = resources;
        self.data_services.image_manager.install_resource_layers_restore(resource_layers);
        self.install_image_list_state_restore(image_lists);
        let physical_fes = self.commit_fes_task_state(fes_tasks);
        self.data_services.image_wrapper_pixels = saved.wrapper_pixels;
        self.data_services.image_wrapper_owners = saved.wrapper_owners;
        self.data_services.image_control_values = saved.image_control;
        self.commit_image_export_state_restore(export);
        self.canvas = canvas; self.graphics_canvas = graphics;
        self.commit_script_data_objects_restore(data); self.commit_backlog_restore(backlogs);
        self.data_services.emocon.install_prepared_graph(saved.emocon);
        self.data_services.messages.defaults = saved.message_defaults;
        self.pending_messages = saved.pending_messages.into();
        self.pending_fes_events = saved.pending_fes_events.into_iter().map(|(code, physical)| PendingFesInput { code, physical }).collect();
        self.message_wait_address = saved.message_wait_address; self.current_parameter = saved.current_parameter;
        self.menu_item_requests = saved.menu_items; self.debug_mode = saved.debug_mode;
        self.sound_config_enabled = saved.sound_config_enabled; self.auto_select = saved.auto_select;
        self.system_config_blocks = saved.system_config;
        self.script_encoding = match saved.encoding { 0 => ScriptTextEncoding::Cp932, 1 => ScriptTextEncoding::Gbk, _ => ScriptTextEncoding::Utf8 };
        self.script_random = ScriptRandom {
            integer: RandomStream { values: std::array::from_fn(|i| saved.random_integer[i]), index: saved.random_indices[0] },
            floating: RandomStream { values: std::array::from_fn(|i| saved.random_float[i]), index: saved.random_indices[1] },
        };
        self.data_services.language.languages = saved.languages;
        self.data_services.language.system_settings_dirty = saved.language_dirty;
        self.install_prepared_input_for_graph(input);
        physical_fes.commit(self);
    }

    /// Called only with all VM/LoggingHost borrows returned. A Retry retains
    /// the frame stage; successful restore replaces the whole continuation.
    pub(crate) fn service_object_graph_request(&mut self) -> anyhow::Result<()> {
        let Some(request) = self.data_services.tasks.graph.pending.clone() else { return Ok(()); };
        match request.index {
            688 => {
                let bytes = self.capture_object_graph_inner(Some(request.clone()))?.encode()
                    .context("encode portable host graph")?;
                self.append_graph_record(request.container, request.record, &bytes)?;
                self.data_services.tasks.graph.pending = None;
                self.data_services.tasks.graph.receipt = Some(request);
            }
            689 => self.restore_object_graph_from_container(request.container, request.record)?,
            _ => anyhow::bail!("invalid pending host graph operation"),
        }
        Ok(())
    }
}

impl LoggingHost<'_> {
    pub(crate) fn portable_object_graph_native(&mut self, index: u32, args: &[u8]) -> Result<NativeReturn> {
        // Positive original state trees must never be decoded as a portable
        // graph. Only an explicitly installed portable target takes that path.
        let portable = args.get(8..12).map(|bytes| u32::from_le_bytes(bytes.try_into().unwrap()))
            .is_some_and(|target| self.data_services.tasks.graph.receivers.contains(&target));
        if !portable { return self.original_state_tree_native(index, args); }
        if args.len() != 12 { return Err(native_state_error("688/689 require CSS, record, task DWORDs")); }
        // The registered wrappers prove the same three argument positions.
        // The explicitly installed adapter owns the portable graph format.
        if self.registered_state_tree_commands().is_none() {
            return Err(native_state_error("portable 688/689 need a registered state-tree wrapper"));
        }
        let container = read_u32(args, 0)?;
        if container == 0 { return Ok(NativeReturn::Value(0)); }
        let request = Request { caller: self.current_task_id, index, container,
            record: read_u32(args, 4)?, target: read_u32(args, 8)? };
        // The first native consumer freezes a root body between callbacks.
        // Nested/synchronous message callbacks need their complete producer
        // stack, and must fail before queuing any request in this version.
        if self.data_services.tasks.executing_external != Some(self.current_task_id)
            || self.message_wait_address.is_some()
            || self.data_services.tasks.records.get(&self.current_task_id)
                .is_some_and(|record| record.message_owner.is_some())
        { return Err(native_state_error("portable graph native needs the external root body, outside a message callback")); }
        if !self.data_services.tasks.graph.receivers.contains(&request.target)
            || self.data_services.tasks.records.get(&request.target).is_none_or(|r| r.deletion != 0 || !matches!(r.kind, 0 | 12))
        { return Err(native_state_error("688/689 target has no recovered original or installed portable graph receiver")); }
        let graph = &mut self.data_services.tasks.graph;
        if graph.receipt.as_ref() == Some(&request) { graph.receipt = None; return Ok(NativeReturn::Value(1)); }
        let file = self.css_handles.get(&container).ok_or_else(|| native_state_error("688/689 CSS handle is absent/wrong type"))?;
        if index == 689 && file.try_record(request.record).map_err(|_| native_state_error("malformed source CSS"))?.is_none() {
            return Ok(NativeReturn::Value(0));
        }
        if graph.pending.as_ref().is_some_and(|pending| pending != &request) || graph.receipt.is_some() {
            return Err(native_state_error("another graph request or receipt is still owned by its native continuation"));
        }
        graph.pending = Some(request);
        Ok(NativeReturn::Retry)
    }
}

fn validate_host(host: &HostContinuation, saved: &ObjectGraphSnapshot) -> anyhow::Result<()> {
    if host.version != 1 || host.phase > 8 || matches!(host.phase, 1 | 7)
        || host.receivers.iter().any(|id| saved.records.get(id).is_none_or(|r| !matches!(r.kind, 0 | 12)))
        || host.messages.len() > 1_000_000 || host.released.len() > 1_000_000
    { anyhow::bail!("invalid host graph phase or receiver registry"); }
    for traversal in host.traversal.iter().chain(host.direct_release.iter()) {
        if !saved.records.contains_key(&traversal.root) || traversal.stack.len() > 512
            || traversal.stack.iter().any(|(id, _)| !saved.records.contains_key(id))
        { anyhow::bail!("saved lifecycle cursor has no matching task graph"); }
    }
    for (&origin, pending) in &host.messages {
        if !saved.records.contains_key(&origin) || pending.stack.len() > 512
            || pending.stack.iter().any(|(id, _)| !saved.records.contains_key(id))
        { anyhow::bail!("saved task message traversal has no matching owner"); }
    }
    if host.root_dispatch.is_some_and(|pending| pending.target != saved.root)
        || host.receipt.as_ref().is_some_and(|r| !matches!(r.index, 688 | 689)
            || !saved.programs.programs.contains_key(&r.caller) || !host.receivers.contains(&r.target))
    { anyhow::bail!("saved root dispatch or graph receipt has invalid ownership"); }
    Ok(())
}

fn phase_number(phase: &PlayerFramePhase) -> u32 {
    match phase { PlayerFramePhase::Start => 0, PlayerFramePhase::Input => 1,
        PlayerFramePhase::Children => 2, PlayerFramePhase::Root => 3, PlayerFramePhase::Fes => 4,
        PlayerFramePhase::Particles => 5, PlayerFramePhase::Messages => 6,
        PlayerFramePhase::Configuration => 7, PlayerFramePhase::Render => 8 }
}
fn restore_phase(phase: u32) -> PlayerFramePhase {
    match phase { 0 => PlayerFramePhase::Start, 1 => PlayerFramePhase::Input,
        2 => PlayerFramePhase::Children, 3 => PlayerFramePhase::Root, 4 => PlayerFramePhase::Fes,
        5 => PlayerFramePhase::Particles, 6 => PlayerFramePhase::Messages,
        7 => PlayerFramePhase::Configuration, _ => PlayerFramePhase::Render }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::TempDir;
    use cs2_formats::CssFile;
    use cs2_script::KcsProgram;
    use std::path::Path;

    #[test]
    #[ignore = "requires the installed V401 engine and exercises a 288 MiB image checkpoint"]
    fn dense_canvas_and_large_list_survive_css_file_rollback() -> anyhow::Result<()> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let scratch = TempDir::new();
        std::fs::copy(workspace.join("games/cs2_full_v401/system/cs2_open.exe"), scratch.0.join("cs2_open.exe"))?;
        std::fs::write(scratch.0.join("startup.xml"), br#"<?xml version="1.0" encoding="UTF-8"?>
            <document><SCREEN><width>128</width><height>128</height></SCREEN>
            <TEXTURE><size><x>256</x><y>256</y></size><vt><x>24</x><y>24</y></vt></TEXTURE></document>"#)?;
        let save = scratch.0.join("profile");
        let mut player = HeadlessPlayer::open_with_android_memory(
            &scratch.0, &save, cs2_runtime::resource_router::GameLanguage::Automatic, 2 << 30,
        )?;
        player.vm = Some(Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 8], 0)?)?);
        let edge = 6144;
        let base_color = 0xff28_567c;
        let corner_color = 0xff80_4020;
        let (file, wrapper) = player.with_host(|host| {
            let root = host.current_task_id;
            host.ensure_task_record(root)?;
            let id = host.create_task_object(15, root, b"imglist", b"")?;
            host.list_contract_native(256, &[id, edge, edge].map(u32::to_le_bytes).concat())?;
            assert!(host.update_ordinary_children(root, 1, 1, 16_384)?);
            let wrapper = host.data_services.tasks.fes.platform.lists[&id].output;
            let mut vm = Vm::new();
            for [x, y, width, height, color] in [[0, 0, edge, edge, base_color], [edge - 1, 0, 1, 1, corner_color]] {
                let args = [wrapper, x, y, width, height, color].map(u32::to_le_bytes).concat();
                host.image_pixels_native(117, &args, &mut vm)?;
            }
            host.list_contract_native(262, &[id, 1].map(u32::to_le_bytes).concat())?;
            assert!(host.update_ordinary_children(root, 1, 1, 16_384)?);
            let file = host.allocate_handle();
            host.css_handles.insert(file, CssFile::new(1000));
            Ok((file, wrapper))
        })?;

        let assert_pixels = |p: &HeadlessPlayer| -> anyhow::Result<()> {
            let image = &p.image_handles[&wrapper];
            let atlas = p.data_services.planes.vram_overflow.as_ref().context("missing dense atlas")?;
            for row in 0..24 {
                for column in 0..24 {
                    let x = column * 256 + 192;
                    let y = row * 256 + 192;
                    assert_eq!(atlas.pixel(x, y), Some(crate::packed_color(base_color)));
                    assert_eq!(image.pixel(x as i32, y as i32)?, crate::packed_color(base_color));
                }
            }
            assert_eq!(atlas.pixel(edge - 1, edge - 1), Some(crate::packed_color(corner_color)));
            assert_eq!(image.pixel(edge as i32 - 1, edge as i32 - 1)?, crate::packed_color(corner_color));
            Ok(())
        };
        assert_pixels(&player)?;
        player.save_object_graph_to_container(file, 42)?;
        let css = player.css_handles.remove(&file).unwrap();
        assert!(css.try_record(42)?.unwrap().len() > 288 * 1024 * 1024);
        let disk = css.to_css()?;
        drop(css);
        assert_eq!(&disk[..4], b"CSG\0");
        crate::write_save_css(&save, "dense.dat", &disk)?;
        let loaded = CssFile::parse(&crate::read_save_css(&save, "dense.dat")?)?;
        player.css_handles.insert(file, loaded);
        player.data_services.planes.vram_overflow = None;
        player.current_parameter = 112;
        player.restore_object_graph_from_container(file, 42)?;
        assert_ne!(player.current_parameter, 112);
        assert_pixels(&player)
    }

    #[test]
    #[ignore = "requires the installed V401 engine"]
    fn native_state_tree_portable_adapter_uses_registered_wrapper() -> anyhow::Result<()> {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        let scratch = TempDir::new();
        let mut player = HeadlessPlayer::open_with_save_root(workspace.join("games/cs2_full_v401"), &scratch.0)?;
        player.data_services.native_profile = None;
        player.data_services.css_task_pe_sha256 = None;
        player.vm = Some(Vm::from_program(KcsProgram::from_code(&[0x3b], vec![0; 8], 0)?)?);
        player.with_host(|host| Ok(host.ensure_task_record(0)?))?;
        player.install_portable_object_graph_receiver(0)?;
        let arguments = [1, 42, 0].map(u32::to_le_bytes).concat();
        player.with_host(|host| {
            host.css_handles.insert(1, CssFile::new(1000));
            let error = host.portable_object_graph_native(688, &arguments).unwrap_err();
            assert!(error.to_string().contains("external root body"));
            host.data_services.tasks.executing_external = Some(0);
            let result = host.portable_object_graph_native(688, &arguments);
            host.data_services.tasks.executing_external = None;
            assert_eq!(result?, NativeReturn::Retry);
            Ok(())
        })?;
        player.service_object_graph_request().context("servicing registered portable capture")?;
        let saved = ObjectGraphSnapshot::decode(player.css_handles[&1].try_record(42)?.unwrap())
            .context("decoding the newly captured portable graph")?;
        assert_eq!(saved.pe_sha256, player.scan.engine.primary_executable.as_ref().unwrap().sha256);

        player.with_host(|host| {
            host.data_services.tasks.executing_external = Some(0);
            let result = host.portable_object_graph_native(688, &arguments);
            host.data_services.tasks.executing_external = None;
            assert_eq!(result?, NativeReturn::Value(1));
            Ok(())
        })?;
        let mut foreign = saved;
        foreign.pe_sha256 = "0".repeat(64);
        let error = player.restore_object_graph_bytes(&foreign.encode()?).unwrap_err();
        assert!(error.to_string().contains("another or an unidentified PE profile"));

        player.current_parameter = 112;
        player.with_host(|host| {
            host.data_services.tasks.executing_external = Some(0);
            let result = host.portable_object_graph_native(689, &arguments);
            host.data_services.tasks.executing_external = None;
            assert_eq!(result?, NativeReturn::Retry);
            Ok(())
        })?;
        player.service_object_graph_request().context("servicing registered portable restore")?;
        assert_ne!(player.current_parameter, 112);
        assert!(!player.data_services.tasks.object_graph_request_pending());

        player.data_services.ordinary_native_table = None;
        player.with_host(|host| {
            let error = host.portable_object_graph_native(689, &arguments).unwrap_err();
            assert!(error.to_string().contains("need a registered state-tree wrapper"));
            assert!(!host.data_services.tasks.object_graph_request_pending());
            Ok(())
        })
    }
}
fn capture_node(n: &SceneNode) -> SceneNodeSnapshot {
    SceneNodeSnapshot { id: n.id, kind: n.kind, parent: n.parent, name: n.name.clone(),
        size_bits: n.size.map(f32::to_bits), position_bits: n.position.map(f32::to_bits), scale_bits: n.scale.map(f32::to_bits),
        color: n.color, overlay_color: n.overlay_color, visible: n.visible, initialized: n.initialized,
        fes_block_name: n.fes_block_name.clone(), fes_object_prefix: n.fes_object_prefix.clone(),
        fes_bank: n.fes_bank, fes_parameters: n.fes_parameters.to_vec() }
}
fn restore_node(n: &SceneNodeSnapshot) -> anyhow::Result<SceneNode> {
    Ok(SceneNode { id: n.id, kind: n.kind, parent: n.parent, name: n.name.clone(),
        size: n.size_bits.map(f32::from_bits), position: n.position_bits.map(f32::from_bits), scale: n.scale_bits.map(f32::from_bits),
        color: n.color, overlay_color: n.overlay_color, visible: n.visible, initialized: n.initialized,
        fes_block_name: n.fes_block_name.clone(), fes_object_prefix: n.fes_object_prefix.clone(),
        fes_bank: n.fes_bank, fes_parameters: n.fes_parameters.as_slice().try_into()? })
}
fn capture_frame(frame: &RgbaFrame) -> ImageFrameSnapshot {
    crate::image_manager::capture_frame(frame)
}
fn capture_canvas(canvas: &Canvas) -> cs2_renderer::Result<ImageFrameSnapshot> {
    Ok(ImageFrameSnapshot { size: [canvas.width(), canvas.height()], rgba: canvas.try_pixels()?.to_vec(),
        container: 0, frame_id: None, bit_depth: 32, offset: [0; 2], canvas_size: [canvas.width(), canvas.height()],
        base: [0; 2], alpha_flag: 0, attributes: BTreeMap::new() })
}
fn restore_frame(frame: &ImageFrameSnapshot) -> anyhow::Result<RgbaFrame> {
    crate::image_manager::restore_frame(frame)
}
fn restore_canvas(frame: &ImageFrameSnapshot) -> anyhow::Result<Canvas> {
    Ok(Canvas::from_rgba8(frame.size[0], frame.size[1], frame.rgba.clone())?)
}
