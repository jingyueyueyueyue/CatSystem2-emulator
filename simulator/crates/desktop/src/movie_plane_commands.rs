//! Movie PLANE and camera messages use the task-owned queue, never VM pointers.
use crate::movie_plane::{PlaneCamera, PlaneMovieKind};
use crate::native_tasks::OwnedTaskBuffers;
use crate::{LoggingHost, native_state_error};
use cs2_renderer::{BlendMode, Rect};
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{Result, Vm};

impl LoggingHost<'_> {
    pub(crate) fn receive_owned_movie_plane_query(
        &mut self, command: TaskCommand, buffers: &mut OwnedTaskBuffers,
        output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        let target = command.target as u32;
        let Some(node) = self.scene_nodes.get(&target).filter(|node| node.kind == 2) else { return Ok(None); };
        let Some(host) = self.movie_host.as_deref().filter(|host| host.plane_target_handle(target).is_some()) else { return Ok(None); };
        match command.code {
            61 | 62 if command.data[0] == 0 => {
                let value = if command.code == 61 {
                    host.planes.properties.get(&target).map_or_else(
                        || crate::movie_plane::PlaneProperties::default().base,
                        |properties| properties.base)
                } else { node.position };
                buffers.write_data1(&float_bytes(&value))?;
            }
            61 | 62 => {},
            75 => {
                buffers.write_data0(&node.size[0].to_le_bytes())?;
                buffers.write_data1(&node.size[1].to_le_bytes())?;
            }
            76 => {
                let size = host.plane_target_handle(target)
                    .and_then(|handle| host.planes.movies.get(&handle).map(|movie| movie.size))
                    .ok_or_else(|| native_state_error("movie plane bounds are unavailable"))?;
                buffers.write_data0(&float_bytes(&[0.; 3]))?;
                buffers.write_data1(&float_bytes(&[size[0], size[1], 0.]))?;
            }
            _ => return Ok(None),
        }
        *output = 0;
        Ok(Some(consumed()))
    }

    pub(crate) fn dispatch_movie_plane_message(
        &mut self, target: u32, packet: [u32; 4], queued: bool,
        output: &mut u32, vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        let Some(node) = self.scene_nodes.get(&target) else { return Ok(None); };
        let camera = node.kind == 65;
        let loaded = self.movie_host.as_ref()
            .is_some_and(|host| host.plane_target_handle(target).is_some());
        let code = packet[1];
        if camera {
            if !matches!(code, 51 | 57 | 66 | 589 | 590) { return Ok(None); }
        } else if node.kind != 2 || !movie_command(code, loaded) {
            return Ok(None);
        }
        if code == 84 && !camera && self.movie_host.is_none() { return Ok(None); }
        // Raw messages to a deleted receiver never dereference their payload
        //or VM destinations. Ordinary query wrappers own separate zeroed
        //scratch buffers and retain their normal status/writeback path.
        if self.data_services.tasks.message_task_state(target)
            .is_some_and(|(_, deleted, _)| deleted != 0)
        {
            *output = 0;
            return Ok(Some(consumed()));
        }
        if matches!(code, 61 | 62 | 75 | 76 | 87 | 88) && !camera {
            // A nonzero selector has no underlying movie geometry. The raw
            //receiver leaves the caller's vector untouched, including invalid
            //addresses; ordinary wrappers supply their own initialized vector.
            if matches!(code, 61 | 62) && packet[2] != 0 {
                *output = 0;
                return Ok(Some(consumed()));
            }
            // Queries write owned data buffers; their scalar output is zero.
            // A queued VM destination needs a retained lease.
            if queued {
                return Err(native_state_error("queued movie query output requires a retained VM destination"));
            }
            let vm = vm.ok_or_else(|| native_state_error("movie query requires its caller VM"))?;
            let command = TaskCommand { target: target as i32, sender: packet[0] as i32,
                code, data: [packet[2], packet[3], 0] };
            let mut owned = self.pending_owned_task_buffers(command)
                .unwrap_or(movie_query_buffers(code)?);
            let outcome = self.route_task_command_buffers(command, &mut owned, output, 4096)?;
            if outcome.is_some_and(|outcome| outcome.done) {
                if matches!(code, 75 | 76) {
                    vm.write(packet[2], owned.data0().ok_or_else(|| native_state_error("movie query omitted data0 output"))?)?;
                }
                vm.write(packet[3], owned.data1().ok_or_else(|| native_state_error("movie query omitted data1 output"))?)?;
            }
            return Ok(outcome);
        }
        let has_parent = self.movie_host.as_ref().and_then(|host| host.planes.properties.get(&target))
            .is_some_and(|properties| properties.parent_offset.is_some());
        let command = TaskCommand { target: target as i32, sender: packet[0] as i32,
            code, data: [packet[2], packet[3], 0] };
        let mut owned = match self.pending_owned_task_buffers(command) {
            Some(buffers) => buffers,
            None => {
                let length = if camera {
                    match code { 51 | 66 => 8, 57 => 64, _ => 0 }
                } else {
                    match code {
                        49 | 51 | 55 if packet[2] == 0 => 12,
                        56 if packet[2] == 0 && (queued || has_parent) => 12,
                        53 | 66 => 12,
                        67 if queued || has_parent => 12,
                        57 if packet[2] == 0 => 64,
                        58 if packet[2] == 0 && (queued || has_parent) => 64,
                        89 if packet[3] != 0 => 16,
                        _ => 0,
                    }
                };
                let payload = if matches!(code, 43 | 44) && !camera {
                    let vm = vm.as_deref().ok_or_else(|| native_state_error("movie load record requires its caller VM"))?;
                    let mut record = vm.read(packet[3], 12)?;
                    let address = packet[3].checked_add(12)
                        .ok_or_else(|| native_state_error("movie name address overflow"))?;
                    let name = vm.read_string(address)?;
                    record.extend(name.into_iter().take_while(|byte| *byte != 0));
                    record.push(0);
                    Some(record)
                } else if length != 0 {
                    Some(vm.as_deref().ok_or_else(|| native_state_error("movie property record requires its caller VM"))?
                        .read(packet[3], length)?)
                } else { None };
                OwnedTaskBuffers::new(None, payload)?
            }
        };
        if queued {
            let found = self.queue_task_command_buffers(command, &owned, output)?;
            Ok(found.then_some(TaskCommandOutcome { done: true, propagate: false }))
        } else {
            self.route_task_command_buffers(command, &mut owned, output, 4096)
        }
    }

    pub(crate) fn receive_movie_plane_command(
        &mut self, command: TaskCommand, buffers: &mut TaskCommandBuffers<'_>,
        output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        let target = command.target as u32;
        let Some(node) = self.scene_nodes.get(&target) else { return Ok(None); };
        let code = command.code;
        let data0 = command.data[0];
        let data1 = command.data[1];
        if node.kind == 65 {
            if !matches!(code, 51 | 57 | 66 | 589 | 590) { return Ok(None); }
            let host = self.movie_host.as_deref_mut()
                .ok_or_else(|| native_state_error("camera command has no movie host"))?;
            let camera = host.planes.cameras.entry(target).or_default();
            match code {
                51 | 66 => {
                    let pair = words::<2>(buffers.data1.as_deref())?.map(f32::from_bits);
                    if code == 51 { camera.offset = pair; } else { camera.scale = pair; }
                }
                57 => camera.matrix = words::<16>(buffers.data1.as_deref())?.map(f32::from_bits),
                589 => { *output = target; return Ok(Some(consumed())); }
                590 => *camera = PlaneCamera::default(),
                _ => unreachable!(),
            }
            *output = 0;
            return Ok(Some(consumed()));
        }
        if node.kind != 2 { return Ok(None); }
        let loaded = self.movie_host.as_ref()
            .is_some_and(|host| host.plane_target_handle(target).is_some());
        if !movie_command(code, loaded) { return Ok(None); }
        if code == 84 && self.movie_host.is_none() { return Ok(None); }
        if matches!(code, 43 | 44) {
            let record = buffers.data1.as_deref()
                .ok_or_else(|| native_state_error("movie load requires an owned XYZ/name record"))?;
            let at = words::<3>(Some(record))?.map(f32::from_bits);
            let tail = record.get(12..)
                .ok_or_else(|| native_state_error("movie load name is absent"))?;
            let end = tail.iter().position(|byte| *byte == 0)
                .ok_or_else(|| native_state_error("movie load name is not terminated"))?;
            let name = self.decode_movie_name(&tail[..end])?;
            self.load_movie_plane_name(target, if code == 43 { PlaneMovieKind::Movie }
                else { PlaneMovieKind::Effect }, &name, at)?;
            *output = 0;
            return Ok(Some(consumed()));
        }
        if matches!(code, 87 | 88) {
            let handle = self.movie_host.as_ref().map_or(0, |host|
                host.plane_movie_handle(target, if code == 87 { PlaneMovieKind::Movie }
                    else { PlaneMovieKind::Effect }));
            let destination = buffers.data1.as_deref_mut().and_then(|bytes| bytes.get_mut(..4))
                .ok_or_else(|| native_state_error("movie handle output storage is absent or truncated"))?;
            destination.copy_from_slice(&handle.to_le_bytes());
            *output = 0;
            return Ok(Some(consumed()));
        }
        if matches!(code, 61 | 62) {
            let host = self.movie_host.as_deref()
                .ok_or_else(|| native_state_error("movie query has no movie host"))?;
            match code {
                61 | 62 if data0 == 0 => {
                    let value = if code == 61 {
                        host.planes.properties.get(&target).map_or_else(
                            || crate::movie_plane::PlaneProperties::default().base,
                            |properties| properties.base)
                    } else { node.position };
                    write_floats(&mut buffers.data1, &value)?;
                }
                _ => {},
            }
            *output = 0;
            return Ok(Some(consumed()));
        }
        if matches!(code, 75 | 76) {
            return Err(native_state_error("two-output movie queries require the owned-buffer receiver"));
        }
        if code == 68 {
            self.release_movie_plane_target(target)?;
            let node = self.scene_nodes.get_mut(&target).unwrap();
            node.initialized = false;
            node.size = [0.; 2];
            *output = 0;
            return Ok(Some(consumed()));
        }
        // Creation belongs to the same PLANE even before a movie is loaded.
        // Keep its ordinary geometry owner and movie owner on the same flag.
        if code == 84 && !loaded {
            self.dispatch_plane_command(command, &mut crate::plane_native::PlaneCommandBuffers {
                data0: None, data1: None,
            }, output)?;
        }
        let camera_valid = self.scene_nodes.get(&data1).is_some_and(|node| node.kind == 65);
        let host = self.movie_host.as_deref_mut()
            .ok_or_else(|| native_state_error("movie property command has no movie host"))?;
        let properties = host.planes.properties.entry(target).or_default();
        let node = self.scene_nodes.get_mut(&target).unwrap();
        match code {
            47 => { properties.display = data1; node.visible = data1 != 0; }
            48 => node.visible = properties.display != 0 && data1 != 0,
            49 if data0 == 0 => properties.base = words::<3>(buffers.data1.as_deref())?.map(f32::from_bits),
            51 if data0 == 0 => node.position = words::<3>(buffers.data1.as_deref())?.map(f32::from_bits),
            53 => properties.parent_offset = Some(words::<3>(buffers.data1.as_deref())?.map(f32::from_bits)),
            54 => properties.parent_offset = None,
            55 if data0 == 0 => {
                properties.rotation = words::<3>(buffers.data1.as_deref())?.map(f32::from_bits);
                properties.matrix = None;
            }
            56 if data0 == 0 && properties.parent_offset.is_some() => {
                properties.rotation = words::<3>(buffers.data1.as_deref())?.map(f32::from_bits);
                properties.matrix = None;
            }
            57 if data0 == 0 => properties.matrix = Some(words::<16>(buffers.data1.as_deref())?.map(f32::from_bits)),
            58 if data0 == 0 && properties.parent_offset.is_some() => {
                properties.matrix = Some(words::<16>(buffers.data1.as_deref())?.map(f32::from_bits));
            }
            59 => properties.color = (data0 != 0).then_some(data1),
            63 => properties.color = Some((properties.color.unwrap_or(u32::MAX) & 0x00ff_ffff) | (data1 & 255) << 24),
            66 => node.scale = words::<3>(buffers.data1.as_deref())?.map(f32::from_bits),
            67 if properties.parent_offset.is_some() => node.scale = words::<3>(buffers.data1.as_deref())?.map(f32::from_bits),
            81 => properties.blend = BlendMode::from_raw(data1),
            82 => properties.perspective_origin = data1,
            84 => {
                properties.create = data1;
                self.data_services.planes.observe_movie_creation_flag(target, data1);
            }
            89 => properties.clip = if data1 == 0 { None } else {
                let rect = words::<4>(buffers.data1.as_deref())?.map(|value| value as i32);
                Some(Rect::new(rect[0], rect[1], rect[2].saturating_sub(rect[0]).max(0) as u32,
                    rect[3].saturating_sub(rect[1]).max(0) as u32))
            },
            591 => properties.camera = if camera_valid { data1 } else { 0 },
            592 => properties.camera = 0,
            49..=52 | 55..=58 | 67 | 85 | 86 => {},
            _ => unreachable!(),
        }
        *output = 0;
        Ok(Some(consumed()))
    }
}

fn movie_command(code: u32, loaded: bool) -> bool {
    matches!(code, 43 | 44 | 84 | 87 | 88) || loaded && matches!(code,
        47..=59 | 61..=63 | 66..=68 | 75 | 76 | 81 | 82 | 85 | 86 | 89 | 591 | 592)
        && !matches!(code, 50 | 52)
}

pub(super) fn movie_query_buffers(code: u32) -> Result<OwnedTaskBuffers> {
    let bytes = if matches!(code, 61 | 62 | 76) { 12 } else { 4 };
    OwnedTaskBuffers::new(matches!(code, 75 | 76).then(|| vec![0; bytes]), Some(vec![0; bytes]))
}

fn write_floats(destination: &mut Option<&mut [u8]>, values: &[f32]) -> Result<()> {
    let bytes = destination.as_deref_mut().and_then(|bytes| bytes.get_mut(..values.len() * 4))
        .ok_or_else(|| native_state_error("movie query output is absent or truncated"))?;
    for (word, value) in bytes.chunks_exact_mut(4).zip(values) {
        word.copy_from_slice(&value.to_le_bytes());
    }
    Ok(())
}

fn float_bytes(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|value| value.to_le_bytes()).collect()
}

fn words<const N: usize>(bytes: Option<&[u8]>) -> Result<[u32; N]> {
    let bytes = bytes.and_then(|bytes| bytes.get(..N * 4))
        .ok_or_else(|| native_state_error("movie property input is absent or truncated"))?;
    Ok(std::array::from_fn(|index| u32::from_le_bytes(bytes[index * 4..index * 4 + 4].try_into().unwrap())))
}

fn consumed() -> TaskCommandOutcome { TaskCommandOutcome { done: true, propagate: false } }
