//! Utate a9f8b087: positive kind42, binary KXS and original380/381/382.
//! Independent from both Campanella's MDL manager and72c9's COLLADA DOM.
//! The recovered locator/camera graph has real local/saved transform banks.
//! Mesh, light and motion imports remain errors until their consumers exist.
use anyhow::Result;
use cs2_formats::{StateNode, StateTree};
use cs2_runtime::{TaskCommand, TaskCommandBuffers, TaskCommandOutcome};
use cs2_runtime::model_state::{LegacyModelState, ModelStateNode, ModelLocalTransforms,
    ModelTransformOverrides, IDENTITY_MODEL_TRANSFORM, decode_local_transform,
    encode_local_transform, model_transform, model_transform_from_rotation_matrix};
use cs2_script::{NativeReturn, Result as ScriptResult, Vm};
use super::super::{LoggingHost, read_u32, expand_kcs_format, trim_nul};
use super::{encoded, decoded, name};
use super::super::native_tasks::OwnedTaskBuffers;

pub(crate) const SHA: &str = "a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52";
type Matrix = [[f32;4];4];
fn identity() -> Matrix { std::array::from_fn(|r| std::array::from_fn(|c| if r==c {1.} else {0.})) }

#[derive(Default)]
pub(crate) struct UtateModel {
    texture_prefix: Vec<u8>, texture_extension: Vec<u8>,
    pub(crate) depth: f32,
    scene_name: Vec<u8>,
    graph: Option<Graph>,
}
struct Graph {
    nodes: Vec<Node>, root_local: ModelLocalTransforms, root_saved: ModelTransformOverrides,
    root_visible: u32, selected_camera: Option<usize>,
}
struct Node {
    name: Vec<u8>, parent: Option<usize>, base: [u32;11],
    local: ModelLocalTransforms, saved: ModelTransformOverrides,
    visible: u32, camera: bool,
}

fn failure(cause: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("Utate kind42: {cause}")
}
fn word(bytes:&[u8], at:usize)->Result<u32>{Ok(read_u32(bytes,at)?)}
fn vector(bytes:&[u8],at:usize)->Result<[f32;3]>{
    let mut result=[0.;3];
    for(i,value)in result.iter_mut().enumerate(){*value=f32::from_bits(word(bytes,at+i*4)?);}
    if result.iter().any(|v|!v.is_finite()){return Err(failure("nonfinite source transform"));}
    Ok(result)
}

impl Graph {
    fn parse(bytes:&[u8])->Result<Self>{
        // 48e9a0/49a760 scan tag/count/total-byte-span records from offset8.
        // No COLLADA XML or compressed72c9 envelope is inferred from a suffix.
        if bytes.len()<8||bytes.len()>64*1024*1024{return Err(failure("binary KXS size is invalid"));}
        let mut at=8usize;let mut tree=None;
        while at<bytes.len(){
            let tag=word(bytes,at)?;
            let count=word(bytes,at+4)?;
            let span=word(bytes,at+8)? as usize;
            let span=if span==0{bytes.len()-at}else{span};
            let end=at.checked_add(span).filter(|&end|span>=12&&end<=bytes.len())
                .ok_or_else(||failure("binary KXS chunk leaves resource"))?;
            if tag==100{
                let decoded=StateTree::decode(&bytes[at+12..end]).map_err(failure)?;
                if tree.replace(decoded).is_some(){return Err(failure("duplicate graph block100"));}
            }else if tag==104&&count!=0{
                return Err(failure("motion block104 importer/consumer is not recovered"));
            }
            at=end;
        }
        let tree=tree.ok_or_else(||failure("binary KXS has no graph block100"))?;
        if tree.roots.is_empty(){return Err(failure("binary KXS graph block is empty"));}
        let mut nodes=Vec::new();
        Self::import(&tree.roots,None,0,&mut nodes)?;
        // 41f9e0/491710 creates the @default@ source document, then48e340
        // constructs its camera. 49fb70 ->48dc20 ->498a60 initializes the
        // source transform to identity;48e720 ->49fbd0 ->498b10 clones it.
        // 49a120 appends that instance to the dummy root as "default".
        let default=nodes.len();
        nodes.push(Node{name:b"default".to_vec(),parent:None,base:IDENTITY_MODEL_TRANSFORM,
            local:Default::default(),saved:Default::default(),visible:1,camera:true});
        Ok(Self{nodes,root_local:Default::default(),root_saved:Default::default(),
            root_visible:1,selected_camera:Some(default)})
    }
    fn import(records:&[StateNode],parent:Option<usize>,depth:usize,nodes:&mut Vec<Node>)->Result<()>{
        if depth>=128{return Err(failure("model graph exceeds128 levels"));}
        for record in records{
            if word(&record.payload,0)?!=0{continue;}
            let raw=&record.payload;let kind=word(raw,0x48)?;
            if matches!(kind,1|2){
                return Err(failure(format!("binary KXS {} importer/render consumer is not recovered",
                    if kind==1{"mesh/material"}else{"light"})));
            }
            if !matches!(kind,0|3){return Err(failure("unidentified binary KXS object subtype"));}
            if nodes.len()>=65_535{return Err(failure("model graph exceeds65536 records"));}
            let raw_name=raw.get(8..0x48).ok_or_else(||failure("node name is truncated"))?;
            let name=name(raw_name,0)?;
            let mut base=model_transform(vector(raw,0x4c)?,vector(raw,0x58)?,vector(raw,0x64)?)
                .map_err(failure)?;
            base[10]=word(raw,0x70)?;
            let count=word(raw,0x74)? as usize;
            let end=count.checked_mul(64).and_then(|n|n.checked_add(0x78))
                .filter(|&end|end<=raw.len()).ok_or_else(||failure("node properties leave record"))?;
            // Imported string properties are real source data, not animation
            // keys. Validate them without fabricating an animation producer.
            for item in raw[0x78..end].chunks_exact(64){name_field(&item[..32])?;name_field(&item[32..])?;}
            if kind==3 {
                let camera=record.children.iter().find(|r|r.payload.get(..4).is_some_and(|b|b==3u32.to_le_bytes()))
                    .ok_or_else(||failure("camera has no type3 source descriptor"))?;
                for offset in [4,8,12,20]{
                    if !f32::from_bits(word(&camera.payload,offset)?).is_finite(){return Err(failure("camera descriptor is nonfinite"));}
                }
            }
            let index=nodes.len();
            nodes.push(Node{name,parent,base,local:Default::default(),saved:Default::default(),
                visible:base[10],camera:kind==3});
            Self::import(&record.children,Some(index),depth+1,nodes)?;
        }
        Ok(())
    }
    fn resolve(&self,path:&[u8])->Option<Option<usize>>{
        // 49a250 normalizes both separator bytes (5af2ac), then49a6a0
        // searches the real root/child tree in linked-list order.
        let mut current: Option<usize>=None;
        for part in path.split(|b|matches!(b,b'/'|b'\\')).filter(|p|!p.is_empty()){
            if part==b".."{current=self.nodes.get(current?)?.parent;}
            else {current=Some(self.nodes.iter().enumerate().rev()
                .find(|(_,n)|n.parent==current&&n.name==part)?.0);}
        }
        Some(current)
    }
    fn state(&self)->Result<LegacyModelState>{
        LegacyModelState::from_live_nodes(self.root_saved.clone(),self.nodes.iter().map(|node|ModelStateNode{
            name:node.name.clone(),parent:node.parent,alive:true,overrides:node.saved.clone(),
        }).collect()).map_err(failure)
    }
    fn restore(&mut self,tree:&StateTree)->Result<()>{
        let mut prepared=self.state()?;prepared.restore(tree).map_err(failure)?;
        let(root,nodes)=prepared.into_overrides();
        self.root_saved=root;
        for(node,saved)in self.nodes.iter_mut().zip(nodes){node.saved=saved;}
        Ok(())
    }
    fn channels(&self,node:Option<usize>)->&ModelLocalTransforms{
        node.map_or(&self.root_local,|i|&self.nodes[i].local)
    }
    fn change(&mut self,node:Option<usize>,code:u32,values:Option<[u32;11]>)->Result<()>{
        let(local,saved,base)=if let Some(i)=node{let n=&mut self.nodes[i];(&mut n.local,&mut n.saved,n.base)}
            else{(&mut self.root_local,&mut self.root_saved,IDENTITY_MODEL_TRANSFORM)};
        match code{
            350=>local.primary=values.unwrap(),352=>local.secondary=values.unwrap(),
            377=>local.freeze_primary(saved,base),378=>local.freeze_secondary(saved),
            379=>*saved=Default::default(),_=>unreachable!(),
        }
        Ok(())
    }
}
fn name_field(bytes:&[u8])->Result<&[u8]>{
    let end=bytes.iter().position(|&b|b==0).ok_or_else(||failure("bounded name has no terminator"))?;
    Ok(&bytes[..end])
}
fn path(bytes:Option<&[u8]>)->Result<&[u8]>{
    let raw=bytes.ok_or_else(||failure("object command needs owned data0"))?;
    let name=name_field(raw)?;
    if name.len()>=128{return Err(failure("object path exceeds original128-byte save field"));}
    Ok(name)
}
impl UtateModel {
    pub(crate) fn consume_worlds(&self)->anyhow::Result<()>{
        let Some(graph)=&self.graph else{return Ok(());};
        let root=graph.root_saved.world_matrix(identity(),identity(),&graph.root_local)?;
        let mut worlds:Vec<Matrix>=Vec::with_capacity(graph.nodes.len());
        for node in &graph.nodes{
            let base=ModelTransformOverrides{primary:Some(node.base),secondary:None}.local_matrix(identity())?;
            let parent=if let Some(i)=node.parent{worlds[i]}else{root};
            worlds.push(node.saved.world_matrix(base,parent,&node.local)?);
        }
        // Locator/camera nodes submit no triangles in49a7a0. Import rejects
        // mesh/light explicitly; an unsupported drawable cannot disappear.
        Ok(())
    }
    fn query(&self,code:u32,raw:Option<&[u8]>)->Result<Option<[u8;48]>>{
        let Some(graph)=&self.graph else{return Ok(None);};
        let Some(node)=graph.resolve(path(raw)?)else{return Ok(None);};
        let local=graph.channels(node);
        Ok(Some(encode_local_transform(if code==375{&local.primary}else{&local.secondary})))
    }
}
fn consumed()->Option<TaskCommandOutcome>{Some(TaskCommandOutcome{done:true,propagate:false})}

impl LoggingHost<'_>{
    pub(crate) fn receive_utate_model_command(&mut self,command:TaskCommand,buffers:&mut TaskCommandBuffers<'_>,output:&mut u32)->ScriptResult<Option<TaskCommandOutcome>>{
        self.receive_utate_model_command_inner(command,buffers,output)
            .map_err(|e|self.task_error(command.target as u32,cs2_runtime::RuntimeError::Host(e.to_string())))
    }
    fn receive_utate_model_command_inner(&mut self,command:TaskCommand,buffers:&mut TaskCommandBuffers<'_>,output:&mut u32)->Result<Option<TaskCommandOutcome>>{
        let id=command.target as u32;
        if !self.data_services.css_models.utate_models.contains_key(&id){return Ok(None);}
        if self.css_task_sha()!=Some(SHA){return Err(failure("live binary KXS owner differs from exact PE"));}
        if command.code==62{return Ok(Some(TaskCommandOutcome{done:true,propagate:true}));}
        if !(338..=382).contains(&command.code){return Ok(None);}
        match command.code{
            338=>{
                if command.data[1]!=0{
                    let bytes=buffers.data1.as_deref().ok_or_else(||failure("configure needs0x20c owned bytes"))?;
                    let prefix=name(bytes.get(..0x104).ok_or_else(||failure("texture prefix truncated"))?,0)?;
                    let extension=name(bytes.get(0x104..0x208).ok_or_else(||failure("texture extension truncated"))?,0)?;
                    let depth=f32::from_bits(word(bytes,0x208)?);
                    if !depth.is_finite(){return Err(failure("nonfinite task depth"));}
                    let owner=self.data_services.css_models.utate_models.get_mut(&id).unwrap();
                    owner.texture_prefix=prefix;owner.texture_extension=extension;owner.depth=depth;
                    if let Some(node)=self.scene_nodes.get_mut(&id){node.position[2]=depth;}
                }
            }
            339=>{
                let raw=name(buffers.data1.as_deref().ok_or_else(||failure("load needs owned model name"))?,0)?;
                if self.data_services.css_models.utate_models[&id].scene_name==raw{return Ok(consumed());}
                // 41eb42 clears the old graph before reading the new file.
                let owner=self.data_services.css_models.utate_models.get_mut(&id).unwrap();owner.graph=None;owner.scene_name.clear();
                if let Some(node)=self.scene_nodes.get_mut(&id){node.initialized=false;}
                let file=decoded(self.text_encoding,&raw)?;
                let bytes=self.resources.read_mode(&file,cs2_runtime::resource_router::ResourceMode::LooseFirst)
                    .map_err(|e|self.task_error(id,e))?;
                // Missing source leaves a real uninitialized owner. Capture
                // subsequently preserves the original unwritten-output guard.
                if let Some(bytes)=bytes{
                    let graph=Graph::parse(&bytes)?;
                    let owner=self.data_services.css_models.utate_models.get_mut(&id).unwrap();owner.graph=Some(graph);owner.scene_name=raw;
                    if let Some(node)=self.scene_nodes.get_mut(&id){node.initialized=true;node.visible=true;}
                }
            }
            342|343=>{
                if command.code==343{*output=0;}
                if let Some(graph)=self.data_services.css_models.utate_models.get_mut(&id).unwrap().graph.as_mut(){
                    if let Some(node)=graph.resolve(path(buffers.data0)?){
                        let value=if let Some(i)=node{&mut graph.nodes[i].visible}else{&mut graph.root_visible};
                        if command.code==342{*value=command.data[1];}else{*output=*value;}
                    }
                }
            }
            346=>{
                if let Some(graph)=self.data_services.css_models.utate_models.get_mut(&id).unwrap().graph.as_mut(){
                    if let Some(Some(index))=graph.resolve(path(buffers.data0)?){if graph.nodes[index].camera{graph.selected_camera=Some(index);}}
                }
            }
            350|352|377..=379=>{
                if let Some(graph)=self.data_services.css_models.utate_models.get_mut(&id).unwrap().graph.as_mut(){
                    if let Some(node)=graph.resolve(path(buffers.data0)?){
                        let values=if matches!(command.code,350|352){Some(decode_local_transform(buffers.data1.as_deref()
                            .ok_or_else(||failure("local transform needs48 owned bytes"))?).map_err(failure)?)}else{None};
                        graph.change(node,command.code,values)?;
                    }
                }
            }
            375|376=>{
                if let Some(bytes)=self.data_services.css_models.utate_models[&id].query(command.code,buffers.data0)?{
                    buffers.data1.as_deref_mut().and_then(|b|b.get_mut(..48)).ok_or_else(||failure("query needs48 writable bytes"))?.copy_from_slice(&bytes);
                }
            }
            380=>{
                if let Some(graph)=&self.data_services.css_models.utate_models[&id].graph{
                    let tree=graph.state()?.capture().map_err(failure)?;
                    *output=self.retain_model_tree(id,tree)?;
                }
            }
            381=>{
                if self.data_services.css_models.utate_models[&id].graph.is_some(){
                    let tree=self.data_services.tasks.model_state_tree_for_owner(id,command.data[1])?;
                    self.data_services.css_models.utate_models.get_mut(&id).unwrap().graph.as_mut().unwrap().restore(&tree)?;
                }
            }
            382=>{self.data_services.tasks.release_model_state_tree(id,command.data[1])?;}
            // 41f7b1 is a proved explicit-zero leaf, not a guessed consumer.
            370|372=>{*output=0;}
            _=>return Err(failure(format!("concrete message{} is not recovered",command.code))),
        }
        Ok(consumed())
    }
    pub(crate) fn receive_owned_utate_model_query(&mut self,command:TaskCommand,buffers:&mut OwnedTaskBuffers)->ScriptResult<Option<TaskCommandOutcome>>{
        self.receive_owned_utate_model_query_inner(command,buffers)
            .map_err(|e|self.task_error(command.target as u32,cs2_runtime::RuntimeError::Host(e.to_string())))
    }
    fn receive_owned_utate_model_query_inner(&mut self,command:TaskCommand,buffers:&mut OwnedTaskBuffers)->Result<Option<TaskCommandOutcome>>{
        if !matches!(command.code,375|376){return Ok(None);}
        let Some(owner)=self.data_services.css_models.utate_models.get(&(command.target as u32))else{return Ok(None);};
        if let Some(bytes)=owner.query(command.code,buffers.data0())?{buffers.write_data1(&bytes)?;}
        Ok(consumed())
    }
    pub(crate) fn dispatch_utate_model_message(&mut self,id:u32,packet:[u32;4],queued:bool,output:&mut u32,vm:Option<&mut Vm>)->ScriptResult<Option<TaskCommandOutcome>>{
        self.dispatch_utate_model_message_inner(id,packet,queued,output,vm)
            .map_err(|e|self.task_error(id,cs2_runtime::RuntimeError::Host(e.to_string())))
    }
    fn dispatch_utate_model_message_inner(&mut self,id:u32,packet:[u32;4],queued:bool,output:&mut u32,mut vm:Option<&mut Vm>)->Result<Option<TaskCommandOutcome>>{
        let code=packet[1];if code!=62&&!(338..=382).contains(&code){return Ok(None);}
        if queued&&matches!(code,375|376){return Err(failure("queued query cannot retain a caller VM output address"));}
        let command=TaskCommand{target:id as i32,sender:packet[0]as i32,code,data:[packet[2],packet[3],0]};
        let mut buffers=if let Some(pending)=self.pending_owned_task_buffers(command){pending}else{
            let owner=&self.data_services.css_models.utate_models[&id];
            let graph=owner.graph.as_ref();
            let read=|address,size:Option<usize>|->Result<Vec<u8>>{
                let vm=vm.as_deref().ok_or_else(||failure("raw buffer needs caller VM"))?;
                if let Some(size)=size{Ok(vm.read(address,size)?)}else{let mut value=vm.read_string(address)?;value.push(0);Ok(value)}
            };
            let data0=if graph.is_some()&&matches!(code,342|343|346|350|352|375..=379){Some(read(packet[2],None)?)}else{None};
            let found=if let(Some(graph),Some(path))=(graph,data0.as_deref()){graph.resolve(name_field(path)?).is_some()}else{false};
            let data1=match code{
                338 if packet[3]!=0=>Some(read(packet[3],Some(0x20c))?),339=>Some(read(packet[3],None)?),
                350|352 if found=>Some(read(packet[3],Some(48))?),
                375|376 if found=>Some(vec![0;48]),_=>None,
            };
            OwnedTaskBuffers::new(data0,data1)?
        };
        if queued{return Ok(self.queue_task_command_buffers(command,&buffers,output)?.then_some(TaskCommandOutcome{done:true,propagate:false}));}
        let result=self.route_task_command_buffers(command,&mut buffers,output,4096)?;
        if buffers.data1_write_length()!=0{
            vm.as_deref_mut().ok_or_else(||failure("query output has no caller VM"))?
                .write(packet[3],&buffers.data1().unwrap()[..buffers.data1_write_length()])?;
        }
        Ok(result)
    }
    pub(super) fn utate_model_native(&mut self,index:u32,args:&[u8],vm:&mut Vm)->ScriptResult<Option<NativeReturn>>{
        self.utate_model_native_inner(index,args,vm)
            .map_err(|e|self.task_error(read_u32(args,0).unwrap_or(self.current_task_id),cs2_runtime::RuntimeError::Host(e.to_string())))
    }
    fn utate_model_native_inner(&mut self,index:u32,args:&[u8],vm:&mut Vm)->Result<Option<NativeReturn>>{
        if self.css_task_sha()!=Some(SHA){return Ok(None);}
        if matches!(index,560|561|671|672|686|687){
            return self.utate_transform_native(index,args,vm).map(Some);
        }
        if !matches!(index,83|543|547|550|551){return Ok(None);}
        let required=match index{83=>4,543|550=>12,_=>8};
        if args.len()!=required{return Err(failure("ordinary argument length is invalid"));}
        let id=read_u32(args,0)?;
        if index==83 {
            if id==0{return Ok(Some(NativeReturn::Continue));}
            if !self.data_services.css_models.utate_models.contains_key(&id){return Ok(None);}
        }
        if !self.data_services.css_models.utate_models.contains_key(&id){return Err(failure("ordinary target lacks its concrete model owner"));}
        if index==551&&self.data_services.tasks.message_task_state(id).is_none_or(|(_,deletion,_)|deletion!=0){
            return Err(failure("visibility query has no live receiver to write its output slot"));
        }
        let code=match index{83=>62,543=>339,547=>346,550=>342,_=>343};
        let command=TaskCommand{target:id as i32,sender:self.current_task_id as i32,code,
            data:[u32::from(matches!(index,547|550|551)),if index==550{read_u32(args,8)?}else{u32::from(index==543)},0]};
        let mut buffers=if let Some(pending)=self.pending_owned_task_buffers(command){pending}else if index==83{OwnedTaskBuffers::default()}else{
            if index==543{
                let startup=self.resources.startup_configuration().map_err(|e|self.task_error(id,e))?
                    .ok_or_else(||failure("texture startup configuration is absent"))?;
                let mut config=vec![0;0x20c];let mut found=false;
                for slot in 0..128{let key=format!("int{slot:02}");
                    if !startup.text(&[b"document",b"INT",key.as_bytes(),b"name"]).unwrap_or_default().eq_ignore_ascii_case(b"texture"){continue;}
                    // 44b8a8/44b8be copy runtime INT+960/+980: int/ext.
                    for(field,offset)in[(b"int".as_slice(),0usize),(b"ext".as_slice(),0x104)]{
                        let value=startup.text(&[b"document",b"INT",key.as_bytes(),field]).unwrap_or_default();
                        let value=encoded(self.text_encoding,std::str::from_utf8(value).map_err(failure)?)?;
                        if value.len()>=260{return Err(failure("texture category exceeds260 bytes"));}
                        config[offset..offset+value.len()].copy_from_slice(&value);
                    }found=true;break;
                }
                if !found{return Err(failure("texture startup category is absent"));}
                config[0x208..].copy_from_slice(&args[8..12]);
                let mut config_buffers=OwnedTaskBuffers::new(None,Some(config))?;
                let outcome=self.route_task_command_buffers(TaskCommand{code:338,..command},&mut config_buffers,&mut 0,4096)?;
                if outcome.is_none_or(|o|!o.done||o.propagate){return Err(failure("configure was not consumed synchronously"));}
            }
            let expanded=expand_kcs_format(vm,&vm.read_string(read_u32(args,4)?)?,0)?;
            let raw=trim_nul(&expanded);
            let mut raw=if index==543{
                let candidates=self.resources.category_names(b"kxs",&decoded(self.text_encoding,raw)?).map_err(|e|self.task_error(id,e))?;
                encoded(self.text_encoding,candidates.first().ok_or_else(||failure("kxs category has no routed candidate"))?)?
            }else{raw.to_vec()};
            if raw.len()>=260{return Err(failure("formatted resource/path exceeds260 bytes"));}raw.push(0);
            if index==543{OwnedTaskBuffers::new(None,Some(raw))?}else{OwnedTaskBuffers::new(Some(raw),None)?}
        };
        let mut output=0;
        let outcome=self.route_task_command_buffers(command,&mut buffers,&mut output,4096)?;
        let outcome=outcome.ok_or_else(||failure("ordinary model command did not reach its receiver"))?;
        Ok(Some(if !outcome.done{NativeReturn::Retry}
            else if index==551{
                //343 writes zero before graph/path lookup, then copies the
                //full visibility DWORD on a match. FFFFFFFF is a valid value.
                NativeReturn::Value(output)
            }else{NativeReturn::Continue}))
    }

    // Fixed table29de80:560/561 construct local350/352;671/672 read
    //376/375.686 selects378/377 by its last DWORD,687 clears379.
    // All payloads enter the same live graph and original688/689 banks.
    fn utate_transform_native(&mut self,index:u32,args:&[u8],vm:&mut Vm)->Result<NativeReturn>{
        let required=match index{560|561|671|672=>20,686=>12,687=>8,_=>unreachable!()};
        if args.len()!=required{return Err(failure("model transform native argument length is invalid"));}
        let id=word(args,0)?;
        if !self.data_services.css_models.utate_models.contains_key(&id){
            return Err(failure("model transform native lacks its exact binary KXS owner"));
        }
        let code=match index{560=>350,561=>352,671=>376,672=>375,
            686=>if word(args,8)?!=0{378}else{377},687=>379,_=>unreachable!()};
        let query=matches!(index,671|672);
        let command=TaskCommand{target:id as i32,sender:self.current_task_id as i32,code,
            data:[1,u32::from(matches!(index,560|561|671|672)),0]};
        // Resolve real VM outputs while this native owns its VM. No VM
        // pointer, scratch address or borrowed slice enters a queued route.
        let destinations=if query{
            [word(args,8)?,word(args,12)?,word(args,16)?]
        }else{[u32::MAX;3]};
        for(address,length)in destinations.into_iter().zip([12,12,64]){
            if address!=u32::MAX{vm.read(address,length)?;}
        }
        let mut buffers=if let Some(saved)=self.pending_owned_task_buffers(command){saved}else{
            let mut object=expand_kcs_format(vm,&vm.read_string(word(args,4)?)?,0)?;
            object.truncate(trim_nul(&object).len());
            if object.len()>=260{return Err(failure("formatted model path exceeds260 bytes"));}
            object.push(0);
            let body=if matches!(index,560|561){
                //FFFFFFFF omits an input vector/matrix; VM address0 is a
                //valid offset and must not be treated as a null host pointer.
                let input=|at:usize,default:[f32;3]|->Result<[f32;3]>{
                    let address=word(args,at)?;
                    if address==u32::MAX{Ok(default)}else{vector(&vm.read(address,12)?,0)}
                };
                let position=input(8,[0.;3])?;let scale=input(12,[1.;3])?;
                let address=word(args,16)?;
                let rotation=if address==u32::MAX{identity()}else{
                    let raw=vm.read(address,64)?;
                    let mut matrix=identity();
                    for(row,values)in matrix.iter_mut().enumerate(){
                        for(column,value)in values.iter_mut().enumerate(){
                            *value=f32::from_bits(word(&raw,(row*4+column)*4)?);
                        }
                    }
                    matrix
                };
                let values=model_transform_from_rotation_matrix(position,scale,rotation).map_err(failure)?;
                Some(encode_local_transform(&values).to_vec())
            }else if query{Some(vec![0;48])}else{None};
            OwnedTaskBuffers::new(Some(object),body)?
        };
        let outcome=self.route_task_command_buffers(command,&mut buffers,&mut 0,4096)?
            .ok_or_else(||failure("model transform command did not reach its actual receiver"))?;
        if !outcome.done{return Ok(NativeReturn::Retry);}
        if query{
            if buffers.data1_write_length()!=48{
                return Err(failure("model local query found no loaded path; original scratch output is unwritten"));
            }
            let raw=buffers.data1().ok_or_else(||failure("model local query lost its owned output"))?;
            let values=decode_local_transform(raw).map_err(failure)?;
            let encoded_matrix=if destinations[2]!=u32::MAX{
                let mut rotation=IDENTITY_MODEL_TRANSFORM;rotation[6..10].copy_from_slice(&values[6..10]);
                let matrix=ModelTransformOverrides{primary:Some(rotation),secondary:None}.local_matrix(identity())?;
                matrix.iter().flatten().flat_map(|value|value.to_bits().to_le_bytes()).collect::<Vec<_>>()
            }else{Vec::new()};
            // Preflight above and the owned receiver's explicit write marker
            // distinguish a real all-zero result from an untouched scratch.
            for(address,bytes)in destinations.into_iter().zip([&raw[..12],&raw[16..28],encoded_matrix.as_slice()]){
                if address!=u32::MAX{vm.write(address,bytes)?;}
            }
        }
        Ok(NativeReturn::Continue)
    }
}
