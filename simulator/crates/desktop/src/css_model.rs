//! Live legacy DAE owner, not a CSS-only map or the later MDL owner.
//! SHA72c9: factory42 /315 configure /316 load /323 camera /341 restore /342 capture.
use std::collections::BTreeMap;
use cs2_renderer::{Canvas,RgbaFrame};
use cs2_runtime::{ResourceIndex,TaskCommand,TaskCommandBuffers,TaskCommandOutcome};
use cs2_script::{NativeReturn,Result,Vm};
use super::{LoggingHost,ScriptTextEncoding,expand_kcs_format,native_state_error,read_u32,trim_nul};
use super::css_dae::Scene;
use super::css_model_shader::Shader;

#[path = "utate_model.rs"]
mod utate;
pub(super) use utate::SHA as UTATE_SHA;

#[path = "fes_model_release.rs"]
mod model_release;
pub(super) use model_release::{
    ModelTaskRelease, plan_model_descendants, request_model_descendants,
};

pub(super) const SHA:&str="72c9c1e7335a92bc2f1c731275a8da34d638d1730b9d5003fb6a8e85fe5b15c8";
const BODY:usize=0x824;
#[derive(Default)]
pub(super) struct CssModelStore{
    pub owners:BTreeMap<u32,Owner>,
    pub utate_models:BTreeMap<u32,utate::UtateModel>,
}
#[derive(Default)]
pub(super) struct Owner{
    // 485590: first path gains '/', second string gains '.' (INT int/ext).
    pub texture_prefix:Vec<u8>,pub texture_extension:Vec<u8>,pub depth:f32,
    pub scene:Option<Scene>,pub scene_name:Vec<u8>,pub camera:Option<usize>,
    // 0x30 is the base image table. 0x34 is selector -> Shader name, not image.
    pub base:Option<(Vec<u8>,Option<RgbaFrame>)>,pub overrides:BTreeMap<Vec<u8>,Vec<u8>>,
    pub node_shaders:BTreeMap<usize,Vec<u8>>,pub material_shaders:BTreeMap<String,Vec<u8>>,
    pub images:BTreeMap<String,RgbaFrame>,pub shaders:BTreeMap<(Vec<u8>,u8),Shader>,
}
impl CssModelStore{
    pub(super) fn register(&mut self,id:u32){self.owners.insert(id,Owner::default());}
    pub(super) fn register_utate(&mut self,id:u32){self.utate_models.insert(id,Default::default());}
    pub(super) fn release(&mut self,id:u32){self.owners.remove(&id);self.utate_models.remove(&id);}
    pub(super) fn draw(&mut self,id:u32,canvas:&mut Canvas,resources:&ResourceIndex,encoding:ScriptTextEncoding)->anyhow::Result<bool>{
        if let Some(owner)=self.utate_models.get(&id){owner.consume_worlds()?;return Ok(true);}
        let Some(owner)=self.owners.get_mut(&id) else{return Ok(false);};
        if owner.scene.is_some(){owner.prepare_draw(resources,encoding)?;super::css_model_render::draw(owner,canvas)?;}
        Ok(true)
    }
}
fn name(bytes:&[u8],offset:usize)->Result<Vec<u8>>{
    let bytes=bytes.get(offset..).ok_or_else(||native_state_error("legacy model string starts outside owned bytes"))?;
    let end=bytes.iter().position(|b|*b==0).ok_or_else(||native_state_error("legacy model string lacks an owned NUL terminator"))?;
    if end>=260{return Err(native_state_error("legacy model path exceeds260-byte scratch capacity"));}Ok(bytes[..end].to_vec())
}
fn shader_name(bytes:&[u8])->Result<Vec<u8>>{
    // 46ad90 copies the basename INCLUDING its extension. Do not decode or
    // normalize the original CSS bytes, or strip the model filename suffix.
    let start=bytes.iter().rposition(|b|matches!(b,b'/'|b'\\')).map_or(0,|i|i+1);
    Ok(bytes[start..].to_vec())
}
fn extension(name:&str,suffix:&str)->String{
    let start=name.rfind(['/', '\\']).map_or(0,|at|at+1);
    let end=name[start..].rfind('.').map_or(name.len(),|at|start+at);
    format!("{}.{suffix}",&name[..end])
}
fn selector(bytes:&[u8])->(Vec<u8>,Vec<u8>){
    match bytes.iter().position(|b|*b==b':'){
        Some(at)=>(bytes[..at].to_vec(),bytes[at+1..].to_vec()),None=>(bytes.to_vec(),Vec::new()),
    }
}
fn encoded(encoding:ScriptTextEncoding,text:&str)->Result<Vec<u8>>{
    let codec=match encoding{ScriptTextEncoding::Utf8=>return Ok(text.as_bytes().to_vec()),ScriptTextEncoding::Cp932=>encoding_rs::SHIFT_JIS,ScriptTextEncoding::Gbk=>encoding_rs::GBK};
    let(bytes,_,invalid)=codec.encode(text);if invalid{return Err(native_state_error("legacy model resource cannot roundtrip script codepage"));}Ok(bytes.into_owned())
}
fn decoded(encoding:ScriptTextEncoding,bytes:&[u8])->Result<String>{
    let codec=match encoding{ScriptTextEncoding::Utf8=>return std::str::from_utf8(bytes).map(str::to_owned).map_err(|_|native_state_error("legacy DAE path is not valid UTF-8")),ScriptTextEncoding::Cp932=>encoding_rs::SHIFT_JIS,ScriptTextEncoding::Gbk=>encoding_rs::GBK};
    let(text,invalid)=codec.decode_without_bom_handling(bytes);if invalid{return Err(native_state_error("legacy DAE path is invalid in the declared script codepage"));}Ok(text.into_owned())
}
impl Owner{
    fn clear_bindings(&mut self){self.base=None;self.overrides.clear();self.node_shaders.clear();self.material_shaders.clear();}
    fn bind_shader(&mut self,raw:&[u8],value:&[u8])->Result<bool>{
        let Some(scene)=self.scene.as_ref()else{return Ok(false);};let(id,node_name)=selector(raw);
        let search=shader_name(&id)?;
        let mut key=search.clone();key.push(b':');key.extend_from_slice(&node_name);
        //341 always supplies a nonnull value pointer, even for an empty
        // string. The map retains that item; only its consumer falls back.
        self.overrides.insert(key,value.to_vec());
        // 480900 compares name(+58) and source model filename(+118).
        // CSS selectors are model-filename:node-name, never DAE array indices.
        let model=shader_name(&self.scene_name)?;
        if let Some(node)=scene.nodes.iter().position(|n|model.eq_ignore_ascii_case(&search)&&n.name.eq_ignore_ascii_case(&node_name)){
            self.node_shaders.remove(&node);if !value.is_empty(){self.node_shaders.insert(node,value.to_vec());}return Ok(true);
        }
        // 485e20 compares material id(+4) exactly and source filename(+44).
        if let Some((material,_))=scene.materials.iter().find(|(_,m)|m.id==node_name&&(search.is_empty()||model==search)){
            let material=material.clone();self.material_shaders.remove(&material);
            if !value.is_empty(){self.material_shaders.insert(material,value.to_vec());}return Ok(true);
        }
        Ok(false)
    }
    fn prepare_draw(&mut self,resources:&ResourceIndex,encoding:ScriptTextEncoding)->anyhow::Result<()>{
        use super::css_model_shader::Texture;
        let Some(scene)=&self.scene else{return Ok(());};
        let mut active=Vec::new();
        for(index,node)in scene.nodes.iter().enumerate(){
            let mut ancestor=Some(index);let mut visible=true;
            while let Some(i)=ancestor{visible&=scene.nodes[i].visible;ancestor=scene.nodes[i].parent;}
            if !visible{continue;}
            for instance in &node.instances{for primitive in &scene.meshes.get(&instance.geometry).ok_or_else(||anyhow::anyhow!("DAE mesh resource is absent"))?.primitives{
                let material_id=instance.materials.get(&primitive.material).ok_or_else(||anyhow::anyhow!("DAE primitive material symbol is unbound"))?;
                let material=scene.materials.get(material_id).ok_or_else(||anyhow::anyhow!("DAE bound material is absent"))?;
                let node_shader=self.node_shaders.get(&index).unwrap_or(&node.shader);
                let material_shader=self.material_shaders.get(material_id).unwrap_or(&material.shader);
                let shader=if node_shader.is_empty(){material_shader}else{node_shader};
                anyhow::ensure!(!shader.is_empty(),"DAE visible primitive has no explicit SHD program");
                active.push((shader.clone(),primitive.attributes,material.texture.clone()));
            }}
        }
        // 48c843: resolve the selected node override, otherwise material
        // override, at actual mesh consumption. Unused source shaders cannot
        // prevent316 or341 from reaching the replacement binding.
        for(name,attributes,image)in active{
            let key=(name.clone(),attributes);
            if !self.shaders.contains_key(&key){
                let decoded=decoded(encoding,&name)?;
                let bytes=resources.read_category(b"shd",&decoded)?;
                let shader=Shader::parse(&bytes,attributes).map_err(|cause|anyhow::anyhow!("DAE SHD {decoded}: {cause}"))?;
                self.shaders.insert(key.clone(),shader);
            }
            if matches!(self.shaders[&key].texture,Texture::Material){
                let image=image.ok_or_else(||anyhow::anyhow!("DAE Shader reads tex0 without material image"))?;
                if !self.images.contains_key(&image){
                    // 49b1a9 replaces the source image extension with hg3;
                    // 4863d0 completes the image category only without a path.
                    let resolved=extension(&image,"hg3");
                    let bytes=resources.read_category(b"image",&resolved)?;
                    let frame=super::image_manager::decode_image_frame(&bytes,0)?
                        .ok_or_else(||anyhow::anyhow!("DAE material image lacks frame0"))?;
                    self.images.insert(image,frame);
                }
            }
        }
        Ok(())
    }
    fn capture(&self)->Result<Vec<u8>>{
        let mut bytes=vec![0;BODY];bytes[..4].copy_from_slice(&(BODY as u32).to_le_bytes());
        let copy=|target:&mut[u8],value:&[u8]|->Result<()>{
            if value.len()>=target.len(){return Err(native_state_error("legacy material capture string exceeds original32-byte output slot"));}
            target[..value.len()].copy_from_slice(value);Ok(())
        };
        if let Some((name,_))=&self.base{copy(&mut bytes[4..0x24],name)?;}
        for(i,(key,value))in self.overrides.iter().take(32).enumerate(){let at=0x24+i*0x40;copy(&mut bytes[at..at+0x20],key)?;copy(&mut bytes[at+0x20..at+0x40],value)?;}Ok(bytes)
    }
}
impl LoggingHost<'_>{
    pub(super) fn legacy_css_model(&self,kind:u32)->bool{kind==42&&matches!(self.css_task_sha(),Some(SHA)|Some(UTATE_SHA))}
    fn css_model_image(&mut self,raw:&[u8])->Result<Option<RgbaFrame>>{
        let path=decoded(self.text_encoding,raw)?;
        let paths=self.resources.category_names(b"image",&path).map_err(|_|native_state_error("legacy model image category resolution failed"))?;
        for path in paths {if let Some(image)=self.load_managed_image(&path,0,false)?{return Ok(Some(image));}}Ok(None)
    }

    fn restore_css_model_bindings(&mut self, target: u32, bytes: &[u8]) -> Result<bool> {
        // The original clears both consumers before inspecting the size field.
        self.data_services.css_models.owners.get_mut(&target)
            .ok_or_else(|| native_state_error("CSS341 target lost its DAE model owner"))?
            .clear_bindings();
        let decoded = (|| -> Result<Option<_>> {
            if read_u32(bytes, 0)? != BODY as u32 {
                return Ok(None);
            }
            if bytes.len() < BODY {
                return Err(native_state_error("CSS341 material record is truncated"));
            }
            let base = name(bytes, 4)?;
            let mut overrides = Vec::new();
            // Preserve strings crossing nominal subslots, bounded by owned CSS.
            for index in 0..32 {
                let start = 0x24 + index * 0x40;
                if bytes[start] != 0 {
                    overrides.push((name(bytes, start)?, name(bytes, start + 0x20)?));
                }
            }
            let base = if base.is_empty() { None } else {
                let image = self.css_model_image(&base)?;
                Some((base, image))
            };
            Ok(Some((base, overrides)))
        })();
        let (base, overrides) = match decoded {
            Ok(Some(record)) => record,
            Ok(None) => return Ok(false),
            Err(error) => {
                self.events.push(super::LogicEvent {
                    frame: self.frame,
                    kind: "save-file-error".into(),
                    detail: format!("CSS341 target={target}: {error}"),
                });
                return Ok(false);
            }
        };
        let owner = self.data_services.css_models.owners.get_mut(&target)
            .ok_or_else(|| native_state_error("CSS341 target lost its DAE model owner"))?;
        owner.base = base;
        for (selector, shader) in overrides {
            owner.bind_shader(&selector, &shader)?;
        }
        Ok(true)
    }

    pub(crate) fn receive_css_model_command(&mut self,command:TaskCommand,buffers:&mut TaskCommandBuffers<'_>,output:&mut u32)->Result<Option<TaskCommandOutcome>>{
        if let Some(outcome)=self.receive_utate_model_command(command,buffers,output)?{return Ok(Some(outcome));}
        let target=command.target as u32;
        if !self.data_services.css_models.owners.contains_key(&target){return Ok(None);}
        if !(315..=342).contains(&command.code){return Ok(None);}
        let body=||buffers.data1.as_deref().ok_or_else(||native_state_error("legacy DAE command needs owned data1"));
        match command.code{
            315=>{if command.data[1]!=0{let bytes=body()?;let mut prefix=name(bytes,0)?;let mut extension=name(bytes,0x104)?;let depth=f32::from_bits(read_u32(bytes,0x208)?);
                if !depth.is_finite(){return Err(native_state_error("legacy DAE depth is not finite"));}
                if !prefix.ends_with(b"/"){prefix.push(b'/');}
                if !extension.starts_with(b"."){extension.insert(0,b'.');}
                let owner=self.data_services.css_models.owners.get_mut(&target).unwrap();owner.texture_prefix=prefix;owner.texture_extension=extension;owner.depth=depth;}},
            316=>{
                let raw=name(body()?,0)?;let decoded=decoded(self.text_encoding,&raw)?;
                // Raw315/316 callers may already provide a routed full path;
                // the ordinary543 wrapper completes the kxs category itself.
                let loaded=self.resources.read_mode(&decoded,cs2_runtime::resource_router::ResourceMode::LooseFirst)
                    .map_err(|cause|self.task_error(target,cause))?;
                let Some(bytes)=loaded else{
                    self.data_services.css_models.owners.get_mut(&target).unwrap().scene=None;
                    if let Some(node)=self.scene_nodes.get_mut(&target){node.initialized=false;}return Ok(Some(TaskCommandOutcome{done:true,propagate:false}));
                };
                self.data_services.css_models.owners.get_mut(&target).unwrap().scene=None;
                let xml=cs2_formats::kxs::decode_model_xml(&bytes).map_err(|cause|self.task_error(target,cs2_runtime::RuntimeError::Host(cause.to_string())))?;
                let mut scene=Scene::parse(&xml).map_err(|cause|self.task_error(target,cs2_runtime::RuntimeError::Host(format!("legacy DAE parse: {cause}"))))?;
                for node in &mut scene.nodes {
                    node.id=encoded(self.text_encoding,std::str::from_utf8(&node.id).map_err(|_|native_state_error("DAE id is not UTF-8"))?)?;
                    node.name=encoded(self.text_encoding,std::str::from_utf8(&node.name).map_err(|_|native_state_error("DAE name is not UTF-8"))?)?;
                    node.shader=encoded(self.text_encoding,std::str::from_utf8(&node.shader).map_err(|_|native_state_error("DAE Shader name is not UTF-8"))?)?;
                }
                for material in scene.materials.values_mut() {
                    material.id=encoded(self.text_encoding,std::str::from_utf8(&material.id).map_err(|_|native_state_error("DAE material id is not UTF-8"))?)?;
                    material.name=encoded(self.text_encoding,std::str::from_utf8(&material.name).map_err(|_|native_state_error("DAE material name is not UTF-8"))?)?;
                    material.shader=encoded(self.text_encoding,std::str::from_utf8(&material.shader).map_err(|_|native_state_error("DAE Shader name is not UTF-8"))?)?;
                }
                // 498e3e reads4807f0 (+58 NAME) for the camera map key;
                // 486267 selects the first camera instance as the default.
                let camera=scene.nodes.iter().position(|n|n.camera.is_some());
                let effective=if bytes.get(..4)==Some(b"kxs\0") {encoded(self.text_encoding,&extension(&decoded,"dae"))?}else{raw};
                let owner=self.data_services.css_models.owners.get_mut(&target).unwrap();owner.clear_bindings();owner.images.clear();owner.shaders.clear();owner.scene_name=effective;owner.scene=Some(scene);owner.camera=camera;
                if let Some(node)=self.scene_nodes.get_mut(&target){node.initialized=true;node.visible=true;node.position[2]=self.data_services.css_models.owners[&target].depth;}
            },
            323=>{if self.data_services.css_models.owners[&target].scene.is_some(){let raw=name(buffers.data0.ok_or_else(||native_state_error("DAE camera requires data0 name"))?,0)?;
                let owner=self.data_services.css_models.owners.get_mut(&target).unwrap();
                // 4862b0 uses the actual camera table key, not a node index.
                if let Some(camera)=owner.scene.as_ref().and_then(|s|s.nodes.iter().position(|n|n.camera.is_some()&&n.name==raw)){owner.camera=Some(camera);}}},
            341=>{
                *output=0;
                if self.data_services.css_models.owners[&target].scene.is_some(){
                    *output=u32::from(self.restore_css_model_bindings(target,body()?)?);
                }
            },
            342=>{*output=0;if self.data_services.css_models.owners[&target].scene.is_some(){
                let bytes=self.data_services.css_models.owners[&target].capture()?;
                buffers.data1.as_deref_mut().and_then(|b|b.get_mut(..BODY)).ok_or_else(||native_state_error("CSS342 needs0x824 writable bytes"))?.copy_from_slice(&bytes);*output=1;}},
            _=>return Err(native_state_error("legacy DAE concrete message branch is not recovered")),
        }
        Ok(Some(TaskCommandOutcome{done:true,propagate:false}))
    }
    pub(crate) fn dispatch_css_model_message(&mut self,target:u32,packet:[u32;4],queued:bool,output:&mut u32,vm:Option<&mut Vm>)->Result<Option<TaskCommandOutcome>>{
        if self.data_services.css_models.utate_models.contains_key(&target){return self.dispatch_utate_model_message(target,packet,queued,output,vm);}
        if !self.data_services.css_models.owners.contains_key(&target)||!(315..=342).contains(&packet[1]){return Ok(None);}
        let command=TaskCommand{target:target as i32,sender:packet[0] as i32,code:packet[1],data:[packet[2],packet[3],0]};
        let mut buffers=if let Some(pending)=self.pending_owned_task_buffers(command){pending}else{
            let read=|address,size|string_read(vm.as_deref(),address,size);
            let loaded=self.data_services.css_models.owners[&target].scene.is_some();
            let data0=if packet[1]==323&&loaded{Some(read(packet[2],None)?)}else{None};
            let data1=match packet[1]{315 if packet[3]!=0=>Some(read(packet[3],Some(0x20c))?),316=>Some(read(packet[3],None)?),341 if loaded=>{
                let header=read(packet[3],Some(4))?;
                Some(if read_u32(&header,0)?==BODY as u32{read(packet[3],Some(BODY))?}else{header})
            },342 if loaded=>Some(read(packet[3],Some(BODY))?),_=>None};
            super::native_tasks::OwnedTaskBuffers::new(data0,data1)?
        };
        if queued{return Ok(self.queue_task_command_buffers(command,&buffers,output)?.then_some(TaskCommandOutcome{done:true,propagate:false}));}
        let result=self.route_task_command_buffers(command,&mut buffers,output,4096)?;
        if packet[1]==342&&buffers.data1_write_length()!=0{
            vm.ok_or_else(||native_state_error("CSS342 output has no caller VM"))?.write(packet[3],buffers.data1().unwrap())?;
        }Ok(result)
    }
    pub(crate) fn receive_owned_css_model_query(&mut self,command:TaskCommand,buffers:&mut super::native_tasks::OwnedTaskBuffers,output:&mut u32)->Result<Option<TaskCommandOutcome>>{
        if let Some(outcome)=self.receive_owned_utate_model_query(command,buffers)?{return Ok(Some(outcome));}
        if command.code!=342||!self.data_services.css_models.owners.contains_key(&(command.target as u32)){return Ok(None);}
        *output=0;let owner=&self.data_services.css_models.owners[&(command.target as u32)];
        if owner.scene.is_some(){let bytes=owner.capture()?;buffers.write_data1(&bytes)?;*output=1;}
        Ok(Some(TaskCommandOutcome{done:true,propagate:false}))
    }
    pub(super) fn css_model_native(&mut self,index:u32,args:&[u8],vm:&mut Vm)->Result<Option<NativeReturn>>{
        if let Some(result)=self.utate_model_native(index,args,vm)?{return Ok(Some(result));}
        if self.css_task_sha()!=Some(SHA)||!matches!(index,543|547|575){return Ok(None);}
        let required=if index==547{8}else{12};if args.len()!=required{return Err(native_state_error("legacy DAE native argument length invalid"));}
        if index==575{
            let handle=read_u32(args,0)?;if handle==0{return Ok(Some(NativeReturn::Value(0)));}
            if !self.css_handles.contains_key(&handle){return Err(native_state_error("CSS575 handle is invalid/wrong type"));}
            let item=read_u32(args,4)?;let target=read_u32(args,8)?;
            if !self.data_services.css_models.owners.contains_key(&target){return Err(native_state_error("legacy CSS575 target lacks its concrete DAE capture receiver"));}
            let command=TaskCommand{target:target as i32,sender:self.current_task_id as i32,code:342,data:[0,1,0]};
            let mut buffers=self.pending_owned_task_buffers(command).unwrap_or(super::native_tasks::OwnedTaskBuffers::new(None,Some(vec![0;BODY]))?);
            let mut output=0;let outcome=self.route_task_command_buffers(command,&mut buffers,&mut output,4096)?;
            if outcome.is_some_and(|o|!o.done){return Ok(Some(NativeReturn::Retry));}
            if output==0{return Ok(Some(NativeReturn::Value(0)));}
            self.css_handles.get_mut(&handle).unwrap().append(item,buffers.data1().unwrap()).map_err(|_|native_state_error("CSS575 append failed"))?;
            return Ok(Some(NativeReturn::Value(1)));
        }
        let target=read_u32(args,0)?;let code=if index==543{316}else{323};
        if !self.data_services.css_models.owners.contains_key(&target){return Err(native_state_error("legacy DAE native target lacks its concrete model owner"));}
        let command=TaskCommand{target:target as i32,sender:self.current_task_id as i32,code,data:[if index==547{1}else{0},if index==543{1}else{0},0]};
        let mut buffers=if let Some(pending)=self.pending_owned_task_buffers(command){pending}else{
            if index==543{
                let startup=self.resources.startup_configuration().map_err(|cause|self.task_error(target,cause))?.ok_or_else(||native_state_error("DAE543 startup configuration is absent"))?;
                let mut config=vec![0;0x20c];let mut found=false;
                for slot in 0..128{let key=format!("int{slot:02}");if !startup.text(&[b"document",b"INT",key.as_bytes(),b"name"]).unwrap_or_default().eq_ignore_ascii_case(b"image"){continue;}
                    for(field,at)in[(b"int".as_slice(),0),(b"ext".as_slice(),0x104)]{let value=startup.text(&[b"document",b"INT",key.as_bytes(),field]).unwrap_or_default();let value=encoded(self.text_encoding,std::str::from_utf8(value).map_err(|_|native_state_error("startup runtime field is not UTF-8"))?)?;if value.len()>=260{return Err(native_state_error("DAE543 texture path exceeds260-byte capacity"));}config[at..at+value.len()].copy_from_slice(&value);}found=true;break;}
                if !found{return Err(native_state_error("DAE543 image category is absent"));}config[0x208..].copy_from_slice(&args[8..12]);
                let mut config_buffers=super::native_tasks::OwnedTaskBuffers::new(None,Some(config))?;
                if self.route_task_command_buffers(TaskCommand{code:315,..command},&mut config_buffers,&mut 0,4096)?.is_some_and(|o|!o.done){return Err(native_state_error("DAE315 configure unexpectedly yielded"));}
            }
            let expanded=expand_kcs_format(vm,&vm.read_string(read_u32(args,4)?)?,0)?;let raw=trim_nul(&expanded);
            let mut name=if index==543{
                let paths=self.resources.category_names(b"kxs",&decoded(self.text_encoding,raw)?).map_err(|cause|self.task_error(target,cause))?;
                encoded(self.text_encoding,paths.first().ok_or_else(||native_state_error("DAE543 kxs category has no path"))?)?
            }else{raw.to_vec()};
            if name.len()>=260{return Err(native_state_error("DAE native name exceeds260-byte capacity"));}name.push(0);
            if index==547{super::native_tasks::OwnedTaskBuffers::new(Some(name),None)?}else{super::native_tasks::OwnedTaskBuffers::new(None,Some(name))?}
        };
        let result=self.route_task_command_buffers(command,&mut buffers,&mut 0,4096)?;
        Ok(Some(if result.is_some_and(|o|!o.done){NativeReturn::Retry}else{NativeReturn::Continue}))
    }
}
fn string_read(vm:Option<&Vm>,address:u32,size:Option<usize>)->Result<Vec<u8>>{
    let vm=vm.ok_or_else(||native_state_error("raw legacy DAE command requires caller VM"))?;
    if let Some(size)=size{vm.read(address,size)}else{let mut name=vm.read_string(address)?;name.push(0);Ok(name)}
}

#[cfg(test)]
mod tests {
    use super::*;
    use cs2_formats::CssFile;
    use cs2_script::Host;

    #[test]
    #[ignore = "requires the installed Tsunaban engine"]
    fn native_css_restore_legacy_model_rejects_truncated_record_and_continues() -> anyhow::Result<()> {
        let games = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let game = games.read_dir()?.filter_map(|entry| entry.ok())
            .find(|entry| entry.file_name().to_string_lossy().starts_with("\u{30c4}\u{30ca}\u{30d0}\u{30f3}"))
            .expect("installed Tsunaban game");
        let scratch = crate::tests::TempDir::new();
        let mut player = crate::HeadlessPlayer::open_with_save_root(game.path(), &scratch.0)?;
        let mut body = vec![0; BODY];
        body[..4].copy_from_slice(&(BODY as u32).to_le_bytes());
        body[0x24..0x2c].copy_from_slice(b":surface");
        body[0x44..0x4a].copy_from_slice(b"shader");
        let mut css = CssFile::new(1000);
        css.append(13, &body)?;
        css.append(14, &body[..0x60])?;
        let mut unterminated = body;
        let last_shader = 0x44 + 31 * 0x40;
        unterminated[last_shader - 0x20] = b':';
        unterminated[last_shader..].fill(b'A');
        css.append(15, &unterminated)?;
        player.with_host(|host| {
            let target = host.create_task_object(42, 0, b"3D View", b"")?;
            let owner = host.data_services.css_models.owners.get_mut(&target).unwrap();
            owner.scene = Some(Scene {
                nodes: Vec::new(), meshes: BTreeMap::new(),
                materials: BTreeMap::from([("test".into(), super::super::css_dae::Material {
                    id: b"surface".to_vec(), name: Vec::new(), color: [1.0; 4],
                    texture: None, shader: Vec::new(),
                })]),
            });
            host.css_handles.insert(1, CssFile::parse(&css.to_css()?)?);
            let args = |item| [1, item, target].map(u32::to_le_bytes).concat();
            let mut vm = Vm::new();
            for (item, expected) in [(13, 1), (14, 0), (13, 1), (15, 0), (13, 1)] {
                assert_eq!(host.call(576, &args(item), &mut vm)?, NativeReturn::Value(expected));
                let owner = &host.data_services.css_models.owners[&target];
                assert_eq!(owner.material_shaders.get("test").map(Vec::as_slice),
                    (expected != 0).then_some(b"shader".as_slice()));
                assert_eq!(owner.overrides.len(), expected as usize);
            }
            assert_eq!(host.events.iter().filter(|event| event.kind == "save-file-error"
                && event.detail.contains("CSS341")).count(), 2);
            Ok(())
        })
    }
}
