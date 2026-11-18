//! SHA-specific kcTask3DView messages. Campanella:543->332/333,
//! 547->340,552->360,554->363,555->365,578->352,579/580->353/354,652->368.
//! 553 sends361 to an unhandled branch in this PE; do not guess it as362.
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_script::{NativeReturn, Result, Vm};
use super::{LoggingHost, native_state_error, read_u32, trim_nul, expand_kcs_format};

impl LoggingHost<'_> {
    /// Add these slots to the existing SHA-gated front guard. No other
    /// version's ordinary552/554/555 is claimed by this receiver.
    pub(super) fn particle_canvas_motion_native(&mut self, index: u32, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let required = match index {552 => 12, 554 | 555 => 8, _ => return Err(native_state_error("unrecognized legacy3D motion native"))};
        if args.len() != required { return Err(native_state_error("legacy3D motion native argument length is invalid")); }
        let command = TaskCommand {target:read_u32(args,0)? as i32, sender:self.current_task_id as i32,
            code:match index {552=>360,554=>363,_=>365}, data:[0,0,0]};
        let mut buffers = if let Some(buffers)=self.pending_owned_task_buffers(command) {buffers} else {
            let expand = |at, capacity| -> Result<Vec<u8>> {
                let bytes=expand_kcs_format(vm,&vm.read_string(read_u32(args,at)?)?,0)?;
                let bytes=trim_nul(&bytes);
                if bytes.len()>=capacity {return Err(native_state_error("legacy3D motion formatted name exceeds its PE buffer"));}
                let mut owned=bytes.to_vec();owned.push(0);Ok(owned)
            };
            // 448590 owns two64-byte formatted buffers. Its empty second
            // buffer sends a NULL clip pointer (detach), distinct from lookup failure.
            let path=expand(4,if index==552 {64} else {260})?;
            let clip=if index==552 {let name=expand(8,64)?;if name.len()==1 {None} else {Some(name)}} else {None};
            super::native_tasks::OwnedTaskBuffers::new(Some(path),clip)?
        };
        let mut value=0;
        let result=self.route_task_command_buffers(command,&mut buffers,&mut value,4096)?;
        if result.is_some_and(|o|!o.done) {return Ok(NativeReturn::Retry);}
        Ok(if index!=552 {NativeReturn::Value(value)} else {NativeReturn::Continue})
    }

    pub(super) fn legacy_particle_canvas(&self, kind: u32) -> bool {
        kind == 42 && self.data_services.particle_external.legacy_kc_view()
    }

    pub(super) fn particle_canvas_native(&mut self, index: u32, args: &[u8], vm: &Vm) -> Result<NativeReturn> {
        let required = match index { 543 | 579 | 652 => 12, 547 | 580 => 8, 578 => 28,
            _ => return Err(native_state_error("unrecognized legacy3D native")) };
        if args.len() != required { return Err(native_state_error("legacy3D native argument length is invalid")); }
        let target = read_u32(args, 0)?;
        let code = match index { 543 => 333, 547 => 340, 578 => 352, 579 => 353, 580 => 354, _ => 368 };
        let command = TaskCommand { target: target as i32, sender: self.current_task_id as i32,
            code, data: match index {578 => [read_u32(args,4)?,0,0],579 => [0,read_u32(args,8)?,0],
                580 => [0;3],_ => [0,1,0]} };
        if index == 580 && !self.data_services.particle_external.canvases.get(&target).is_some_and(|c|c.configured) {
            // This PE leaves its local return scratch uninitialized without
            // a manager. Do not turn that branch into a fabricated zero.
            return Err(native_state_error("3D color query has no configured manager/defined return value"));
        }
        let mut buffers = if let Some(buffers) = self.pending_owned_task_buffers(command) { buffers }
        else {
            if index == 543 {
                let config = self.resources.startup_configuration().map_err(|_| native_state_error("3D texture startup configuration failed"))?
                    .ok_or_else(|| native_state_error("3D texture startup configuration is absent"))?;
                let mut record = vec![0; 0x20c];
                let mut found = false;
                for slot in 0..128 {
                    let key = format!("int{slot:02}");
                    let path = [b"document".as_slice(), b"INT", key.as_bytes(), b"name"];
                    if !config.text(&path).unwrap_or_default().eq_ignore_ascii_case(b"texture") { continue; }
                    for (field, offset) in [(b"dir".as_slice(),0usize),(b"int".as_slice(),0x104)] {
                        let value = config.text(&[b"document",b"INT",key.as_bytes(),field]).unwrap_or_default();
                        if value.len() >= 260 { return Err(native_state_error("3D texture category exceeds260-byte capacity")); }
                        record[offset..offset+value.len()].copy_from_slice(value);
                    }
                    found = true; break;
                }
                if !found { return Err(native_state_error("3D texture startup category is absent")); }
                record[0x208..].copy_from_slice(&args[8..12]);
                let mut config_buffers = super::native_tasks::OwnedTaskBuffers::new(None,Some(record))?;
                let configured = self.route_task_command_buffers(TaskCommand { code:332,..command }, &mut config_buffers,&mut 0,4096)?;
                if configured.is_some_and(|o| !o.done) { return Err(native_state_error("3D configure receiver unexpectedly yielded before owned load")); }
            }
            if index == 578 {
                // 449100: target/enable/near/far/R/G/B. The float words
                // are immediate bits; each color contributes its low byte.
                let mut record = vec![0;12];
                record[..8].copy_from_slice(&args[8..16]);
                record[8] = args[16]; record[9] = args[20]; record[10] = args[24];
                super::native_tasks::OwnedTaskBuffers::new(None,Some(record))?
            } else if index == 652 {
                // Unlike611's VM pointer,652 owns two immediate float values.
                super::native_tasks::OwnedTaskBuffers::new(None, Some(args[4..12].to_vec()))?
            } else {
                let expanded = expand_kcs_format(vm,&vm.read_string(read_u32(args,4)?)?,0)?;
                let name = trim_nul(&expanded);
                let capacity = if matches!(index,579|580) {64} else {260};
                if name.len() >= capacity { return Err(native_state_error("3D formatted resource/path exceeds its PE buffer capacity")); }
                let name = if index == 543 {
                    let name = self.text_encoding.decode(name);
                    let candidates = self.resources.category_names(b"kxs",&name)
                        .map_err(|_| native_state_error("3D MDL category completion failed"))?;
                    let resolved = candidates.first().ok_or_else(|| native_state_error("3D category has no candidate"))?;
                    encode(self.text_encoding,resolved)?
                } else { name.to_vec() };
                let mut name = name; name.push(0);
                if index == 580 { super::native_tasks::OwnedTaskBuffers::new(Some(name),Some(u32::MAX.to_le_bytes().to_vec()))? }
                else if matches!(index,547|579) { super::native_tasks::OwnedTaskBuffers::new(Some(name),None)? }
                else { super::native_tasks::OwnedTaskBuffers::new(None,Some(name))? }
            }
        };
        let result = self.route_task_command_buffers(command,&mut buffers,&mut 0,4096)?;
        if result.is_some_and(|o| !o.done) {return Ok(NativeReturn::Retry);}
        if index == 580 {
            if result.is_none() {return Err(native_state_error("3D color query lost its task receiver"));}
            let value=read_u32(buffers.data1().ok_or_else(||native_state_error("3D color query lacks output scratch"))?,0)?;
            return Ok(NativeReturn::Value(value));
        }
        Ok(NativeReturn::Continue)
    }

    pub(crate) fn receive_particle_canvas_command(&mut self, command: TaskCommand,
        buffers: &mut TaskCommandBuffers<'_>, output: &mut u32,
    ) -> Result<Option<TaskCommandOutcome>> {
        let target = command.target as u32;
        if !self.data_services.particle_external.canvases.contains_key(&target) { return Ok(None); }
        let data = |which: u32| -> Result<&[u8]> {
            (if which == 0 { buffers.data0.as_deref() } else { buffers.data1.as_deref() })
                .ok_or_else(|| native_state_error("3D message requires an owned buffer"))
        };
        match command.code {
            332 => {
                let bytes = data(1)?;
                if bytes.len() < 0x20c { return Err(native_state_error("3D manager configuration requires0x20c bytes")); }
                let depth = f32::from_bits(read_u32(bytes,0x208)?);
                if !depth.is_finite() { return Err(native_state_error("3D draw depth is non-finite")); }
                let directory = bounded_name(&bytes[..0x104])?;
                let archive = bounded_name(&bytes[0x104..0x208])?;
                let canvas = self.data_services.particle_external.canvases.get_mut(&target).unwrap();
                canvas.texture_directory = directory; canvas.texture_archive = archive; canvas.depth = depth; canvas.configured = true;
                if let Some(node) = self.scene_nodes.get_mut(&target) { node.position[2] = depth; }
            }
            333 => {
                let name = bounded_name(data(1)?)?;
                let decoded = self.text_encoding.decode(&name);
                self.data_services.particle_external.clear_canvas_graph(target);
                // Preserve the live manager configuration across graph reload.
                let bytes = self.resources.read_mode(&decoded,cs2_runtime::resource_router::ResourceMode::LooseFirst)
                    .map_err(|_| native_state_error("3D MDL resource read failed"))?
                    .ok_or_else(|| native_state_error("3D MDL resource is unavailable"))?;
                let texture_policy=self.legacy_model_texture_policy()?;
                let canvas=self.data_services.particle_external.canvases.get_mut(&target).unwrap();
                let prepared=canvas.load(name,&bytes).and_then(|()|canvas.prepare_assets(self.resources,self.text_encoding,texture_policy));
                let absent=match prepared {
                    Ok(absent)=>absent,
                    Err(error)=>{
                        canvas.clear_graph();
                        return Err(super::native_state_error_owned(format!("3D MDL geometry/material load failed: {error}")));
                    }
                };
                for detail in absent {
                    self.events.push(super::LogicEvent {frame:self.frame,kind:"mdl-texture-absent".into(),
                        detail:format!("target={target} {detail}")});
                }
                if let Some(node) = self.scene_nodes.get_mut(&target) { node.initialized = true; node.visible = true; }
            }
            340 => {
                let path = bounded_name(data(0)?)?;
                let canvas = self.data_services.particle_external.canvases.get_mut(&target).unwrap();
                if let Some(camera) = canvas.resolve(&path).filter(|&i| canvas.nodes[i].is_camera()) {
                    canvas.selected_camera = Some(camera);
                }
            }
            336 | 337 => {
                // 1f121/1f16c -> 940a0: an absent path is skipped, while
                // the real dummy root is a valid object (empty path).
                if command.code == 337 { *output = 0; }
                let canvas = &self.data_services.particle_external.canvases[&target];
                if canvas.loaded {
                    let path = bounded_name(data(0)?)?;
                    if let Some(found) = canvas.resolve_object(&path) {
                        let canvas = self.data_services.particle_external.canvases.get_mut(&target).unwrap();
                        let value = match found {
                            Some(index) => &mut canvas.nodes[index].visible,
                            None => &mut canvas.root_visible,
                        };
                        if command.code == 336 { *value = command.data[1]; } else { *output = *value; }
                    }
                }
            }
            338 => self.data_services.particle_external.canvases.get_mut(&target).unwrap()
                .render_enabled = Some(command.data[1]),
            339 => *output = self.data_services.particle_external.canvases[&target].render_enabled.unwrap_or(1),
            352 => {
                // 41f66e checks the manager before configuration/data1.
                // Invalid fogtype or enable0 disables without reading data1.
                if self.data_services.particle_external.canvases[&target].configured {
                    let mode = self.legacy_model_fog_mode()?;
                    let record = if mode <= 1 && command.data[0] != 0 { Some(data(1)?) } else { None };
                    self.data_services.particle_external.canvases.get_mut(&target).unwrap()
                        .configure_fog(mode,command.data[0],record)
                        .map_err(|e|super::native_state_error_owned(format!("3D fog configuration: {e}")))?;
                }
            }
            353 => {
                // data1 is packed immediate RGBA, not a VM pointer/record.
                // Only this resolved type1 source owns a color multiplier.
                let canvas=&self.data_services.particle_external.canvases[&target];
                if canvas.configured && canvas.loaded {
                    if let Some(index)=canvas.resolve(&bounded_name(data(0)?)?)
                        .filter(|&index|canvas.nodes[index].kind==1)
                    {
                        self.data_services.particle_external.canvases.get_mut(&target).unwrap()
                            .nodes[index].set_material_color(command.data[1]);
                    }
                }
            }
            354 => {
                let canvas=&self.data_services.particle_external.canvases[&target];
                if canvas.configured {
                    let destination=buffers.data1.as_deref_mut().and_then(|b|b.get_mut(..4))
                        .ok_or_else(||native_state_error("3D color query needs four writable output bytes"))?;
                    // 41f7ce writes its sentinel before resolving data0.
                    destination.copy_from_slice(&u32::MAX.to_le_bytes());
                    if canvas.loaded {
                        let path=bounded_name(buffers.data0.ok_or_else(||native_state_error("3D color query lacks its owned path"))?)?;
                        if let Some(index)=canvas.resolve(&path).filter(|&index|canvas.nodes[index].kind==1) {
                            destination.copy_from_slice(&canvas.nodes[index].packed_material_color().to_le_bytes());
                        }
                    }
                }
            }
            360 | 362 | 363 | 365 => {
                // Fixed table41fd3c:360 bind,362 play,363 query. Resolve the
                // path before reading the second owned record, as the PE does.
                if matches!(command.code,363|365) {*output=0;}
                let canvas=&self.data_services.particle_external.canvases[&target];
                if canvas.loaded {
                    let path=bounded_name(data(0)?)?;
                    if let Some(found)=canvas.resolve_object(&path) {
                        if let Some(index)=found {
                            match command.code {
                                360 => {
                                    let name=buffers.data1.as_deref().map(bounded_name).transpose()?;
                                    self.data_services.particle_external.canvases.get_mut(&target).unwrap()
                                        .bind_motion(index,name.as_deref());
                                }
                                362 => {
                                    let bytes=data(1)?;
                                    let words=[read_u32(bytes,0)?,read_u32(bytes,4)?,read_u32(bytes,8)?,read_u32(bytes,12)?];
                                    let canvas=self.data_services.particle_external.canvases.get_mut(&target).unwrap();
                                    canvas.play_motion(index,words)
                                        .map_err(|e|super::native_state_error_owned(format!("3D motion playback: {e}")))?;
                                }
                                363 => *output=canvas.motion_pending(index),
                                365 => *output=canvas.motion_end_word(index),
                                _=>unreachable!(),
                            }
                        } else if command.code==363 {
                            // Dummy root has no source pointer:48acd0 returns
                            // false, then41fc4d returns1. A missing path returns0.
                            *output=1;
                        }
                    }
                }
            }
            368 => {
                let bytes = data(1)?;
                self.data_services.particle_external.bind(target,[f32::from_bits(read_u32(bytes,0)?),f32::from_bits(read_u32(bytes,4)?)])?;
            }
            334|335|342|348..=351 => {} // Exact PE consumed no-op entries.
            332..=376 => return Err(native_state_error("legacy3D message branch requires an independent graph/model implementation")),
            _ => return Ok(None),
        }
        Ok(Some(TaskCommandOutcome { done:true,propagate:false }))
    }

    pub(crate) fn dispatch_particle_canvas_message(&mut self, target: u32, packet: [u32;4],
        queued: bool, output: &mut u32, mut vm: Option<&mut Vm>,
    ) -> Result<Option<TaskCommandOutcome>> {
        if !(332..=376).contains(&packet[1]) { return Ok(None); }
        if queued && packet[1] == 354 {
            return Err(native_state_error("queued3D color query cannot retain a VM output address"));
        }
        let color_output = packet[1] == 354
            && self.data_services.particle_external.canvases.get(&target).is_some_and(|c|c.configured);
        let command = TaskCommand { target:target as i32,sender:packet[0] as i32,code:packet[1],data:[packet[2],packet[3],0] };
        let mut buffers = if let Some(buffers) = self.pending_owned_task_buffers(command) { buffers } else {
            let read = |address, string: bool, size| -> Result<Vec<u8>> {
                let vm = vm.as_deref().ok_or_else(|| native_state_error("raw3D message needs caller VM"))?;
                if string { let mut bytes=vm.read_string(address)?;bytes.push(0);Ok(bytes) } else { vm.read(address,size) }
            };
            //354 writes its output sentinel before resolving the path.
            // Snapshot that path below, after the same caller-VM write.
            let data0 = if packet[1] == 354 {None}
                else if matches!(packet[1], 336 | 337 | 340 | 353 | 360 | 362 | 363 | 365)
                && self.data_services.particle_external.canvases.get(&target).is_some_and(|canvas|
                    canvas.loaded && (packet[1] != 353 || canvas.configured))
                { Some(read(packet[2],true,0)?) } else { None };
            let found=self.data_services.particle_external.canvases.get(&target).is_some_and(|canvas|
                data0.as_deref().and_then(|bytes|bytes.strip_suffix(&[0])).and_then(|path|canvas.resolve(path)).is_some());
            let fog_record = packet[1] == 352 && packet[2] != 0
                && self.data_services.particle_external.canvases.get(&target).is_some_and(|c|c.configured)
                && self.legacy_model_fog_mode()? <= 1;
            let data1 = match packet[1] {
                332=>Some(read(packet[3],false,0x20c)?),333=>Some(read(packet[3],true,0)?),368=>Some(read(packet[3],false,8)?),
                352 if fog_record=>Some(read(packet[3],false,12)?),
                354 if color_output=>Some(u32::MAX.to_le_bytes().to_vec()),
                360 if found && packet[3]!=0 =>Some(read(packet[3],true,0)?),
                362 if found=>Some(read(packet[3],false,16)?),_=>None };
            let mut data0=data0;
            if color_output {
                let caller=vm.as_deref_mut().ok_or_else(||native_state_error("raw3D color query needs caller VM"))?;
                caller.write(packet[3],&u32::MAX.to_le_bytes())?;
                if self.data_services.particle_external.canvases.get(&target).is_some_and(|c|c.loaded) {
                    let mut path=caller.read_string(packet[2])?;path.push(0);data0=Some(path);
                }
            }
            super::native_tasks::OwnedTaskBuffers::new(data0,data1)?
        };
        if queued {
            Ok(self.queue_task_command_buffers(command,&buffers,output)?.then_some(TaskCommandOutcome { done:true,propagate:false }))
        } else {
            let result=self.route_task_command_buffers(command,&mut buffers,output,4096);
            if color_output {
                // A white/default result still writes, even when all output
                // bytes equal the sentinel or the route's previous scratch.
                vm.as_deref_mut().ok_or_else(||native_state_error("raw3D color query needs caller VM"))?
                    .write(packet[3],buffers.data1().and_then(|b|b.get(..4))
                        .ok_or_else(||native_state_error("raw3D color query lost output scratch"))?)?;
            }
            result
        }
    }
}

impl LoggingHost<'_> {
    fn legacy_model_texture_policy(&mut self)->Result<[u32;2]> {
        self.ensure_configuration_state()
            .map_err(|e|super::native_state_error_owned(format!("3D texture platform configuration: {e}")))?;
        let value=|key|self.data_services.configuration.platform_value(key)
            .ok_or_else(||native_state_error("3D texture platform field is absent"));
        // Root+5350/5354 is the same global+5140/5144 used by49fbd0.
        // These are image options, independent of auxiliary surface limits.
        Ok([value("D3D/tex_size")?,value("D3D/tex_comp")?])
    }

    fn legacy_model_fog_mode(&mut self)->Result<u32> {
        self.ensure_configuration_state()
            .map_err(|e|super::native_state_error_owned(format!("3D fog platform configuration: {e}")))?;
        self.data_services.configuration.platform_value("D3D/fogtype")
            .ok_or_else(||native_state_error("3D fog platform field is absent"))
    }

    pub(super) fn advance_legacy_particle_canvas(&mut self,target:u32)->Result<()> {
        let elapsed=self.data_services.tasks.task_clock(target)
            .ok_or_else(||native_state_error("legacy3D update lacks task clock"))?.elapsed();
        let canvas=self.data_services.particle_external.canvases.get_mut(&target)
            .ok_or_else(||native_state_error("legacy3D update lacks canvas owner"))?;
        canvas.advance(elapsed).map_err(|e|super::native_state_error_owned(format!("legacy3D motion update: {e}")))
    }
}

fn bounded_name(bytes: &[u8]) -> Result<Vec<u8>> {
    let end=bytes.iter().position(|&b|b==0).filter(|&n|n<260)
        .ok_or_else(||native_state_error("3D name lacks its260-byte bounded terminator"))?;
    Ok(bytes[..end].to_vec())
}
fn encode(encoding: super::ScriptTextEncoding, value: &str) -> Result<Vec<u8>> {
    let codec=match encoding {super::ScriptTextEncoding::Cp932=>encoding_rs::SHIFT_JIS,
        super::ScriptTextEncoding::Gbk=>encoding_rs::GBK,super::ScriptTextEncoding::Utf8=>return Ok(value.as_bytes().to_vec())};
    let (bytes,_,errors)=codec.encode(value);
    if errors {return Err(native_state_error("3D name cannot be encoded for this script"));} Ok(bytes.into_owned())
}
