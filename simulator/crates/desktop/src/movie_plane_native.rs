//! PLANE movie commands and ordinary-native adapters. No reference implementation is linked.
use crate::movie_native::record_host_error;
use crate::movie_plane::{PlaneCamera, PlaneMovieKind};
use crate::{LoggingHost, expand_kcs_format, native_state_error, read_f32_matrix, read_u32};
use cs2_renderer::{BlendMode, Rect};
use cs2_script::{NativeReturn, Vm};

#[path = "movie_plane_commands.rs"]
mod commands;
#[path = "legacy_plane_input.rs"]
mod legacy_input;

impl LoggingHost<'_> {
    pub(super) fn movie_plane_native(
        &mut self,
        index: u32,
        args: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<Option<NativeReturn>> {
        if let Some(result) = self.legacy_plane_input_native(index, args)? {
            return Ok(Some(result));
        }
        // UTATE456b60 predates the movie-plane wrapper: four integer
        // coordinates address the context's exclusive player, not a task.
        // The native forms an x86 wrapping RECT;46c050 subtracts it back to
        // width/height, so preserve the original bits for the surface host.
        if index == 701 && self.bind_utate_plane_input_profile()? {
            if args.len() != 16 {
                return Err(native_state_error("legacy movie rectangle requires four integer arguments"));
            }
            let rectangle = [read_u32(args, 0)? as i32, read_u32(args, 4)? as i32,
                read_u32(args, 8)? as i32, read_u32(args, 12)? as i32];
            if let Some(host) = self.movie_host.as_deref_mut() {
                host.set_display_rectangle(rectangle)
                    .map_err(|cause| record_host_error(self.events, cause))?;
            }
            return Ok(Some(NativeReturn::Continue));
        }
        if matches!(index, 701 | 988) {
            let target = read_u32(args, 0)?;
            if target == 0 {
                return Ok(Some(NativeReturn::Continue));
            }
            if args.len() != 8 {
                return Err(native_state_error(
                    "native 701/988 requires target and a name-format address",
                ));
            }
            let command = cs2_runtime::TaskCommand { target: target as i32,
                sender: self.current_task_id as i32, code: if index == 701 { 43 } else { 44 }, data: [0; 3] };
            let mut buffers = match self.pending_owned_task_buffers(command) {
                Some(buffers) => buffers,
                None => {
                    let bytes = expand_kcs_format(vm, &vm.read_string(read_u32(args, 4)?)?, 0)?;
                    let mut record = vec![0; 12];
                    record.extend(bytes.into_iter().take_while(|byte| *byte != 0));
                    record.push(0);
                    crate::native_tasks::OwnedTaskBuffers::new(None, Some(record))?
                }
            };
            let outcome = self.route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?;
            return Ok(Some(if outcome.is_some_and(|outcome| !outcome.done) { NativeReturn::Retry }
                else { NativeReturn::Continue }));
        }
        if matches!(index, 702 | 989) {
            if args.len() != 4 {
                return Err(native_state_error(
                    "movie plane query requires exactly one task id",
                ));
            }
            let target = read_u32(args, 0)?;
            let command = cs2_runtime::TaskCommand { target: target as i32,
                sender: self.current_task_id as i32, code: if index == 702 { 87 } else { 88 }, data: [0; 3] };
            let mut buffers = self.pending_owned_task_buffers(command)
                .unwrap_or(crate::native_tasks::OwnedTaskBuffers::new(None, Some(vec![0; 4]))?);
            let outcome = self.route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?;
            if outcome.is_some_and(|outcome| !outcome.done) {
                return Ok(Some(NativeReturn::Retry));
            }
            let bytes = buffers.data1().and_then(|bytes| bytes.get(..4))
                .ok_or_else(|| native_state_error("movie handle receiver omitted its output storage"))?;
            let handle = u32::from_le_bytes(bytes.try_into().unwrap());
            self.record_movie_plane_command(target, command.code);
            return Ok(Some(NativeReturn::Value(handle)));
        }
        if !matches!(index, 67 | 69 | 70 | 78..=106 | 343..=345 | 402 | 466 | 653 | 1065..=1069) {
            return Ok(None);
        }
        let target = read_u32(args, 0)?;
        if index == 67 {
            // Native67 marks a subtree. Task-tree release owns actual device
            // teardown after notifications; don't destroy children early.
            return Ok(None);
        }
        let Some(node_kind) = self.scene_nodes.get(&target).map(|node| node.kind) else {
            return Ok(None);
        };
        let is_camera = node_kind == 65;
        let is_movie = self
            .movie_host
            .as_ref()
            .is_some_and(|host| host.plane_target_handle(target).is_some());
        if matches!(index, 69 | 70) && matches!(node_kind, 2 | 65) {
            let packet = [self.current_task_id, read_u32(args, 4)?, read_u32(args, 8)?, read_u32(args, 12)?];
            let mut output = 0;
            if let Some(outcome) = self.dispatch_movie_plane_message(target, packet, index == 70, &mut output, Some(&mut *vm))? {
                return Ok(Some(if outcome.done { NativeReturn::Value(output) } else { NativeReturn::Retry }));
            }
        }
        // A deleted object cannot consume an unrelated service's native or
        //turn a value-returning query into status2. Geometry wrappers below
        //still dispatch their initialized scratch and write the real outputs.
        let deleted = self.data_services.tasks.message_task_state(target)
            .is_some_and(|(_, deleted, _)| deleted != 0);
        if deleted && (is_movie || is_camera) && matches!(index, 69 | 70) {
            return Ok(Some(NativeReturn::Value(0)));
        }
        if deleted && (
            is_camera && matches!(index, 84 | 101 | 344 | 1065)
            || is_movie && matches!(index,
                78 | 79 | 80 | 82 | 83 | 84 | 85 | 88 | 90..=94 | 100 | 101 | 104 | 344 | 402 | 466 | 653 | 1066..=1069)
        ) {
            return Ok(Some(NativeReturn::Continue));
        }
        if index == 653 && node_kind == 2 {
            let flag = read_u32(args, 4)?;
            self.movie_host
                .as_deref_mut()
                .ok_or_else(|| native_state_error("movie-plane creation flag has no movie host"))?
                .planes
                .properties
                .entry(target)
                .or_default()
                .create = flag;
            self.data_services
                .planes
                .observe_existing(index, args, self.scene_nodes)?;
            return Ok(Some(NativeReturn::Continue));
        }
        if matches!(index, 69 | 70) && node_kind == 2 {
            let command = read_u32(args, 4)?;
            if index == 70 && (is_movie || matches!(command, 43 | 44 | 84 | 87 | 88)) {
                return Err(native_state_error(
                    "queued movie-plane output or drawable replacement has no recovered continuation",
                ));
            }
            if is_movie && command == 3 {
                self.request_ordinary_task_delete(target)?;
                return Ok(Some(NativeReturn::Value(0)));
            }
            if is_movie && matches!(command, 41 | 42 | 45 | 68 | 70 | 83) {
                self.release_movie_plane_target(target)?;
                if command == 68 {
                    if let Some(node) = self.scene_nodes.get_mut(&target) {
                        node.initialized = false;
                        node.size = [0.; 2];
                    }
                    return Ok(Some(NativeReturn::Value(0)));
                }
                return Err(native_state_error(
                    "movie plane replacement requires the corresponding real geometry command service",
                ));
            }
            if is_movie && command == 89 {
                let address = read_u32(args, 12)?;
                let clip = if address == 0 {
                    None
                } else {
                    let data = vm.read(address, 16)?;
                    let values: [i32; 4] = std::array::from_fn(|axis| {
                        i32::from_le_bytes(data[axis * 4..axis * 4 + 4].try_into().unwrap())
                    });
                    Some(Rect::new(
                        values[0],
                        values[1],
                        values[2].saturating_sub(values[0]).max(0) as u32,
                        values[3].saturating_sub(values[1]).max(0) as u32,
                    ))
                };
                self.movie_host
                    .as_deref_mut()
                    .unwrap()
                    .planes
                    .properties
                    .entry(target)
                    .or_default()
                    .clip = clip;
                return Ok(Some(NativeReturn::Value(0)));
            }
            if is_movie {
                if index == 70 {
                    return Err(native_state_error(
                        "queued movie-plane command requires a recovered output or replacement continuation",
                    ));
                }
                if self.apply_movie_plane_command(
                    target,
                    command,
                    read_u32(args, 8)?,
                    read_u32(args, 12)?,
                    vm,
                )? {
                    return Ok(Some(NativeReturn::Value(0)));
                }
            }
            return Ok(None);
        }
        if is_movie && matches!(index, 78 | 79 | 80 | 82 | 83 | 104) {
            self.release_movie_plane_target(target)?;
            return Ok(None);
        }
        if !is_movie && !is_camera {
            return Ok(None);
        }
        if is_movie && index == 90 {
            if args.len() != 16 {
                return Err(native_state_error("movie plane parent offset requires target and three floats"));
            }
            let command = cs2_runtime::TaskCommand { target: target as i32,
                sender: self.current_task_id as i32, code: 53, data: [0; 3] };
            let mut buffers = self.pending_owned_task_buffers(command)
                .unwrap_or(crate::native_tasks::OwnedTaskBuffers::new(None, Some(args[4..16].to_vec()))?);
            let outcome = self.route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?;
            return Ok(Some(if outcome.is_some_and(|outcome| !outcome.done) {
                NativeReturn::Retry
            } else { NativeReturn::Continue }));
        }
        if is_movie && matches!(index, 86 | 87 | 89 | 343 | 345) {
            let (code, expected) = match index {
                86 => (61, 16), 87 => (62, 20), 89 => (61, 4),
                343 => (75, 12), 345 => (76, 20), _ => unreachable!(),
            };
            if args.len() != expected {
                return Err(native_state_error("movie geometry query has the wrong argument length"));
            }
            let selector = match index {
                86 => read_u32(args, 12)?, 87 => read_u32(args, 16)?, _ => 0,
            };
            let command = cs2_runtime::TaskCommand { target: target as i32,
                sender: self.current_task_id as i32, code, data: [selector, 0, 0] };
            let mut buffers = self.pending_owned_task_buffers(command)
                .unwrap_or(commands::movie_query_buffers(code)?);
            let outcome = self.route_task_command_buffers(command, &mut buffers, &mut 0, 4096)?
                .ok_or_else(|| native_state_error("movie geometry query has no live receiver"))?;
            if !outcome.done {
                return Ok(Some(NativeReturn::Retry));
            }
            let first = buffers.data0();
            let second = buffers.data1().ok_or_else(|| native_state_error("movie query omitted its vector output"))?;
            if index == 89 {
                return Ok(Some(NativeReturn::Value(u32::from_le_bytes(second[8..12].try_into().unwrap()))));
            }
            let count = match index { 86 | 343 => 2, 87 => 3, 345 => 4, _ => unreachable!() };
            // Preserve the wrappers' write order for overlapping destinations.
            for axis in 0..count {
                let bytes = match index {
                    343 if axis == 0 => &first.ok_or_else(|| native_state_error("movie query omitted width"))?[..4],
                    343 => &second[..4],
                    345 if axis < 2 => &first.ok_or_else(|| native_state_error("movie query omitted low bounds"))?[axis * 4..axis * 4 + 4],
                    345 => &second[(axis - 2) * 4..(axis - 2) * 4 + 4],
                    _ => &second[axis * 4..axis * 4 + 4],
                };
                vm.write(read_u32(args, 4 + axis * 4)?, bytes)?;
            }
            return Ok(Some(NativeReturn::Continue));
        }
        let host = self
            .movie_host
            .as_deref_mut()
            .ok_or_else(|| native_state_error("plane properties have no movie host"))?;
        if is_camera {
            let camera = host.planes.cameras.entry(target).or_default();
            match index {
                84 => camera.offset = [float(args, 4)?, float(args, 8)?],
                101 => camera.scale = [float(args, 4)?, float(args, 8)?],
                344 => camera.matrix = read_f32_matrix(vm, read_u32(args, 4)?)?,
                1065 => *camera = PlaneCamera::default(),
                _ => return Ok(None),
            }
            return Ok(Some(NativeReturn::Continue));
        }
        let movie_size = host
            .plane_target_handle(target)
            .and_then(|handle| host.planes.movies.get(&handle).map(|movie| movie.size));
        let properties = host.planes.properties.entry(target).or_default();
        match index {
            84 if read_u32(args, 16)? != 0 => {}
            85 => {
                if read_u32(args, 12)? == 0 {
                    properties.base[0] = float(args, 4)?;
                    properties.base[1] = float(args, 8)?;
                }
            }
            86 => {
                let base = if read_u32(args, 12)? == 0 {
                    properties.base
                } else {
                    [0.; 3]
                };
                vm.write(read_u32(args, 4)?, &base[0].to_le_bytes())?;
                vm.write(read_u32(args, 8)?, &base[1].to_le_bytes())?;
            }
            87 => {
                let position = if read_u32(args, 16)? == 0 {
                    self.scene_nodes[&target].position
                } else {
                    [0.; 3]
                };
                for (axis, value) in position.into_iter().enumerate() {
                    vm.write(read_u32(args, 4 + axis * 4)?, &value.to_le_bytes())?;
                }
            }
            88 => properties.base[2] = float(args, 4)?,
            89 => return Ok(Some(NativeReturn::Value(properties.base[2].to_bits()))),
            91 => properties.parent_offset = None,
            93 => {
                if read_u32(args, 16)? == 0 {
                    properties.rotation = [float(args, 4)?, float(args, 8)?, float(args, 12)?];
                    properties.matrix = None;
                }
            }
            92 => {
                properties.display = read_u32(args, 4)?;
                self.scene_nodes.get_mut(&target).unwrap().visible = properties.display != 0;
            }
            94 => {
                let alpha = (read_u32(args, 4)? as i32).clamp(0, 255) as u32;
                properties.color =
                    Some((properties.color.unwrap_or(u32::MAX) & 0x00ff_ffff) | alpha << 24);
            }
            100 => properties.color = (read_u32(args, 8)? != 0).then_some(read_u32(args, 4)?),
            343 => {
                let size = self.scene_nodes[&target].size;
                vm.write(read_u32(args, 4)?, &size[0].to_le_bytes())?;
                vm.write(read_u32(args, 8)?, &size[1].to_le_bytes())?;
            }
            344 => {
                if read_u32(args, 8)? == 0 {
                    properties.matrix = Some(read_f32_matrix(vm, read_u32(args, 4)?)?);
                }
            }
            345 => {
                let size = movie_size
                    .ok_or_else(|| native_state_error("movie plane bounds are unavailable"))?;
                let bounds = [0., 0., size[0], size[1]];
                for (axis, value) in bounds.into_iter().enumerate() {
                    vm.write(read_u32(args, 4 + axis * 4)?, &value.to_le_bytes())?;
                }
            }
            402 => properties.blend = BlendMode::from_raw(read_u32(args, 4)?),
            466 => properties.perspective_origin = read_u32(args, 4)?,
            1066 | 1068 => {
                let camera = read_u32(args, 4)?;
                properties.camera = if self
                    .scene_nodes
                    .get(&camera)
                    .is_some_and(|node| node.kind == 65)
                {
                    camera
                } else {
                    0
                };
            }
            1067 | 1069 => properties.camera = 0,
            _ => return Ok(None), // Existing position/scale/display paths keep updating SceneNode.
        }
        Ok(Some(NativeReturn::Continue))
    }

    pub(super) fn release_movie_plane_target(&mut self, target: u32) -> cs2_script::Result<()> {
        let events = &mut *self.events;
        if let Some(host) = self.movie_host.as_deref_mut() {
            if let Some(handle) = host.plane_target_handle(target) {
                self.movie_resources.remove(&handle);
                self.movie_playbacks.remove(&handle);
            }
            host.release_plane(target)
                .map_err(|cause| record_host_error(events, cause))?;
        }
        Ok(())
    }

    /// Actual task-tree release, separate from replacing only its drawable.
    pub(super) fn release_movie_task_resources(&mut self, target: u32) -> cs2_script::Result<()> {
        self.release_movie_plane_target(target)?;
        if let Some(host) = self.movie_host.as_deref_mut() {
            host.planes.properties.remove(&target);
            host.planes.cameras.remove(&target);
        }
        Ok(())
    }

    pub(super) fn load_movie_plane_name(
        &mut self,
        target: u32,
        kind: PlaneMovieKind,
        name: &str,
        at: [f32; 3],
    ) -> cs2_script::Result<()> {
        self.record_movie_plane_command(
            target,
            if kind == PlaneMovieKind::Movie {
                43
            } else {
                44
            },
        );
        let Some(node) = self.scene_nodes.get(&target) else {
            return Ok(());
        };
        if node.kind != 2 {
            return Err(native_state_error(
                "movie command target is not a PLANE task",
            ));
        }
        if self
            .data_services
            .tasks
            .message_task_state(target)
            .is_some_and(|(_, deleted, _)| deleted != 0)
        {
            return Ok(());
        }
        self.release_movie_plane_target(target)?;
        self.scene_nodes.get_mut(&target).unwrap().initialized = false;
        let category: &[u8] = if kind == PlaneMovieKind::Movie {
            b"movie"
        } else {
            b"veff"
        };
        let prefix = if kind == PlaneMovieKind::Movie {
            "movie"
        } else {
            "veff"
        };
        let names = self.movie_resource_candidates(category, prefix, name)?;
        let Some(resource) = names.into_iter().next() else {
            return Ok(());
        };
        let handle = *self.next_xml_handle;
        let next = handle
            .checked_add(1)
            .filter(|_| handle != 0)
            .ok_or_else(|| native_state_error("movie-plane resource handle space exhausted"))?;
        let events = &mut *self.events;
        let host = self
            .movie_host
            .as_deref_mut()
            .ok_or_else(|| native_state_error("PLANE movie has no real movie host"))?;
        let size = host
            .load_plane(handle, target, kind, resource.clone(), at)
            .map_err(|cause| record_host_error(events, cause))?;
        if let Some(size) = size {
            *self.next_xml_handle = next;
            self.movie_resources.insert(handle, resource);
            let node = self.scene_nodes.get_mut(&target).unwrap();
            node.size = size;
            node.position = [0.; 3];
            node.scale = [1.; 3];
            node.color = u32::MAX;
            node.overlay_color = 0;
            node.initialized = true;
            node.visible = false; // Drawable is hidden until ordinary display command.
        }
        Ok(())
    }

    fn record_movie_plane_command(&mut self, target: u32, command: u32) {
        if let Some(event) = self.events.last_mut() {
            event
                .detail
                .push_str(&format!(" movie_plane target={target} command={command}"));
        }
    }

    fn apply_movie_plane_command(
        &mut self,
        target: u32,
        command: u32,
        data0: u32,
        data1: u32,
        vm: &mut Vm,
    ) -> cs2_script::Result<bool> {
        let host = self
            .movie_host
            .as_deref_mut()
            .ok_or_else(|| native_state_error("PLANE command has no movie host"))?;
        let movie_size = host
            .plane_target_handle(target)
            .and_then(|handle| host.planes.movies.get(&handle).map(|movie| movie.size));
        let camera_valid = self
            .scene_nodes
            .get(&data1)
            .is_some_and(|camera| camera.kind == 65);
        let properties = host.planes.properties.entry(target).or_default();
        let node = self
            .scene_nodes
            .get_mut(&target)
            .ok_or_else(|| native_state_error("PLANE command target disappeared"))?;
        match command {
            47 => {
                properties.display = data1;
                node.visible = data1 != 0;
            }
            48 => node.visible = properties.display != 0 && data1 != 0,
            49 if data0 == 0 => properties.base = vector(vm, data1)?,
            51 if data0 == 0 => node.position = vector(vm, data1)?,
            53 => properties.parent_offset = Some(vector(vm, data1)?),
            54 => properties.parent_offset = None,
            55 if data0 == 0 => {
                properties.rotation = vector(vm, data1)?;
                properties.matrix = None;
            }
            57 if data0 == 0 => properties.matrix = Some(read_f32_matrix(vm, data1)?),
            56 if data0 == 0 && properties.parent_offset.is_some() => {
                properties.rotation = vector(vm, data1)?;
                properties.matrix = None;
            }
            58 if data0 == 0 && properties.parent_offset.is_some() => {
                properties.matrix = Some(read_f32_matrix(vm, data1)?)
            }
            59 => properties.color = (data0 != 0).then_some(data1),
            61 if data0 == 0 => write_vector(vm, data1, properties.base)?,
            62 if data0 == 0 => write_vector(vm, data1, node.position)?,
            63 => {
                properties.color =
                    Some((properties.color.unwrap_or(u32::MAX) & 0x00ff_ffff) | (data1 & 255) << 24)
            }
            66 => node.scale = vector(vm, data1)?,
            67 if properties.parent_offset.is_some() => node.scale = vector(vm, data1)?,
            75 => {
                vm.write(data0, &node.size[0].to_le_bytes())?;
                vm.write(data1, &node.size[1].to_le_bytes())?;
            }
            76 => {
                let size = movie_size
                    .ok_or_else(|| native_state_error("movie plane bounds are unavailable"))?;
                write_vector(vm, data0, [0.; 3])?;
                write_vector(vm, data1, [size[0], size[1], 0.])?;
            }
            81 => properties.blend = BlendMode::from_raw(data1),
            82 => properties.perspective_origin = data1,
            85 | 86 | 49..=52 | 55..=58 | 61 | 62 | 67 => {}
            591 => properties.camera = if camera_valid { data1 } else { 0 },
            592 => properties.camera = 0,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

fn float(args: &[u8], offset: usize) -> cs2_script::Result<f32> {
    read_u32(args, offset).map(f32::from_bits)
}

fn vector(vm: &Vm, address: u32) -> cs2_script::Result<[f32; 3]> {
    let bytes = vm.read(address, 12)?;
    Ok(std::array::from_fn(|axis| {
        f32::from_le_bytes(bytes[axis * 4..axis * 4 + 4].try_into().unwrap())
    }))
}

fn write_vector(vm: &mut Vm, address: u32, vector: [f32; 3]) -> cs2_script::Result<()> {
    let bytes = vector
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect::<Vec<_>>();
    vm.write(address, &bytes)
}
