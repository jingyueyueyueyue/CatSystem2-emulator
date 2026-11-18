//! Independent COLLADA 1.4 scene reader for the SHA72c9 CSS/material owner.
//! Attributes and repeated elements must survive; ConfigXml is not a DOM.
use std::collections::BTreeMap;
use anyhow::{Result, anyhow, bail, ensure};
use cs2_runtime::model_state::{ModelLocalTransforms, ModelTransformOverrides};

pub(super) type Matrix = [[f32; 4]; 4];
pub(super) fn identity() -> Matrix {
    std::array::from_fn(|r| std::array::from_fn(|c| if r == c { 1. } else { 0. }))
}
pub(super) fn multiply(a: Matrix, b: Matrix) -> Matrix {
    std::array::from_fn(|r| std::array::from_fn(|c| (0..4).map(|k| a[r][k]*b[k][c]).sum()))
}
pub(super) fn transform(v: [f32;4], m: Matrix) -> [f32;4] {
    std::array::from_fn(|c| (0..4).map(|r| v[r]*m[r][c]).sum())
}

#[derive(Debug)]
struct Xml {
    tag: String,
    attributes: BTreeMap<String,String>,
    text: String,
    children: Vec<Xml>,
}
impl Xml {
    fn attr(&self, key: &str) -> Option<&str> { self.attributes.get(key).map(String::as_str) }
    fn required(&self, key: &str) -> Result<&str> {
        self.attr(key).ok_or_else(|| anyhow!("DAE {} lacks attribute {key}",self.tag))
    }
    fn child(&self, key: &str) -> Result<&Xml> {
        self.children.iter().find(|n|n.tag==key).ok_or_else(||anyhow!("DAE {} lacks {key}",self.tag))
    }
    fn optional(&self, key: &str) -> Option<&Xml> { self.children.iter().find(|n|n.tag==key) }
    fn named<'a>(&'a self,key:&'a str) -> impl Iterator<Item=&'a Xml> {
        self.children.iter().filter(move|n|n.tag==key)
    }
    fn descend<'a>(&'a self, out: &mut BTreeMap<String,&'a Xml>) -> Result<()> {
        if let Some(id)=self.attr("id") { ensure!(out.insert(id.to_owned(),self).is_none(),"duplicate DAE id {id}"); }
        for child in &self.children { child.descend(out)?; }
        Ok(())
    }
}
fn entities(value:&str)->Result<String> {
    let mut result=String::new(); let mut rest=value;
    while let Some(at)=rest.find('&') {
        result.push_str(&rest[..at]); rest=&rest[at+1..];
        let end=rest.find(';').ok_or_else(||anyhow!("unterminated DAE XML entity"))?;
        let entity=&rest[..end];
        let ch=match entity { "amp"=>'&',"lt"=>'<',"gt"=>'>',"quot"=>'"',"apos"=>'\'',
            v if v.starts_with("#x")=>char::from_u32(u32::from_str_radix(&v[2..],16)?).ok_or_else(||anyhow!("invalid XML scalar"))?,
            v if v.starts_with('#')=>char::from_u32(v[1..].parse()?).ok_or_else(||anyhow!("invalid XML scalar"))?,
            _=>bail!("DAE XML external/unknown entity {entity}"), };
        ensure!(ch!='\0',"NUL XML scalar");result.push(ch);rest=&rest[end+1..];
    }
    result.push_str(rest);Ok(result)
}
struct Parser<'a>{ text:&'a str, at:usize, count:usize }
impl<'a> Parser<'a> {
    fn skip(&mut self)->Result<()> {
        loop {
            while self.text.as_bytes().get(self.at).is_some_and(u8::is_ascii_whitespace) {self.at+=1;}
            let suffix=&self.text[self.at..];
            let (start,end)=if suffix.starts_with("<?") {(2,"?>")}
                else if suffix.starts_with("<!--") {(4,"-->")}
                else {return Ok(());};
            self.at+=start+suffix[start..].find(end).ok_or_else(||anyhow!("unterminated DAE XML declaration/comment"))?+end.len();
        }
    }
    fn name(&mut self)->Result<String>{
        let start=self.at;
        while self.text.as_bytes().get(self.at).is_some_and(|b|b.is_ascii_alphanumeric()||b"_-.:".contains(b)){self.at+=1;}
        ensure!(self.at>start,"missing DAE XML name");Ok(self.text[start..self.at].to_owned())
    }
    fn element(&mut self,depth:usize)->Result<Xml>{
        self.skip()?;self.count+=1;
        ensure!(depth<=128&&self.count<=500_000,"DAE XML structural limit");
        ensure!(self.text[self.at..].starts_with('<'),"DAE element expected");self.at+=1;
        let full=self.name()?;let tag=full.rsplit(':').next().unwrap().to_owned();
        let mut node=Xml{tag,attributes:BTreeMap::new(),text:String::new(),children:Vec::new()};
        loop {
            while self.text.as_bytes().get(self.at).is_some_and(u8::is_ascii_whitespace){self.at+=1;}
            if self.text[self.at..].starts_with("/>"){self.at+=2;return Ok(node);}
            if self.text[self.at..].starts_with('>'){self.at+=1;break;}
            let key=self.name()?;
            while self.text.as_bytes().get(self.at).is_some_and(u8::is_ascii_whitespace){self.at+=1;}
            ensure!(self.text.as_bytes().get(self.at)==Some(&b'='),"DAE attribute equals expected");self.at+=1;
            while self.text.as_bytes().get(self.at).is_some_and(u8::is_ascii_whitespace){self.at+=1;}
            let quote=*self.text.as_bytes().get(self.at).ok_or_else(||anyhow!("truncated DAE attribute"))?;
            ensure!(quote==b'\''||quote==b'"',"DAE quoted attribute expected");self.at+=1;
            let end=self.text[self.at..].find(quote as char).ok_or_else(||anyhow!("unterminated DAE attribute"))?+self.at;
            ensure!(node.attributes.insert(key,entities(&self.text[self.at..end])?).is_none(),"duplicate DAE attribute");self.at=end+1;
        }
        loop {
            ensure!(self.at<self.text.len(),"unterminated DAE element");
            let suffix=&self.text[self.at..];
            if suffix.starts_with("</"){
                self.at+=2;ensure!(self.name()?==full,"mismatched DAE closing tag");
                while self.text.as_bytes().get(self.at).is_some_and(u8::is_ascii_whitespace){self.at+=1;}
                ensure!(self.text.as_bytes().get(self.at)==Some(&b'>'),"DAE closing bracket expected");self.at+=1;return Ok(node);
            }
            if suffix.starts_with("<!--")||suffix.starts_with("<?"){self.skip()?;continue;}
            if suffix.starts_with("<![CDATA["){
                let end=suffix[9..].find("]]>").ok_or_else(||anyhow!("unterminated DAE CDATA"))?+9;
                node.text.push_str(&suffix[9..end]);self.at+=end+3;continue;
            }
            ensure!(!suffix.starts_with("<!"),"DAE DTD/external entity declarations are unsupported");
            if suffix.starts_with('<'){node.children.push(self.element(depth+1)?);}
            else {let end=suffix.find('<').ok_or_else(||anyhow!("unterminated DAE text"))?;node.text.push_str(&entities(&suffix[..end])?);self.at+=end;}
        }
    }
}
fn document(bytes:&[u8])->Result<Xml>{
    ensure!(bytes.len()<=64*1024*1024,"DAE resource limit");
    let bytes=bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    // COLLADA1.4 DOM strings are UTF-8. No ACP/content guess is permitted.
    let text=std::str::from_utf8(bytes)?;
    if text.starts_with("<?xml") {
        let end=text.find("?>").ok_or_else(||anyhow!("unterminated XML declaration"))?;
        if let Some(at)=text[..end].find("encoding") {
            let after=text[at+8..end].trim_start().strip_prefix('=').ok_or_else(||anyhow!("invalid XML encoding declaration"))?.trim_start();
            let quote=after.as_bytes().first().copied().ok_or_else(||anyhow!("missing XML encoding"))?;
            ensure!(quote==b'\''||quote==b'"',"invalid XML encoding quote");
            let encoding=after[1..].split(quote as char).next().unwrap();
            ensure!(encoding.eq_ignore_ascii_case("utf-8")||encoding.eq_ignore_ascii_case("utf8"),"DAE encoding {encoding} is not this DOM's UTF-8 contract");
        }
    }
    let mut parser=Parser{text,at:0,count:0};let root=parser.element(0)?;parser.skip()?;
    ensure!(parser.at==text.len(),"trailing DAE XML data");Ok(root)
}
fn floats(node:&Xml)->Result<Vec<f32>> {
    let out:Vec<f32>=node.text.split_ascii_whitespace().map(str::parse).collect::<std::result::Result<_,_>>()?;
    ensure!(out.len()<=8_000_000&&out.iter().all(|v|v.is_finite()),"DAE float array bounds/nonfinite");Ok(out)
}
fn indices(node:&Xml)->Result<Vec<usize>> {
    let out:Vec<usize>=node.text.split_ascii_whitespace().map(str::parse).collect::<std::result::Result<_,_>>()?;
    ensure!(out.len()<=16_000_000,"DAE index limit");Ok(out)
}
fn reference<'a>(value:&str,ids:&BTreeMap<String,&'a Xml>)->Result<&'a Xml>{
    let id=value.strip_prefix('#').ok_or_else(||anyhow!("DAE external reference {value} is unsupported"))?;
    ids.get(id).copied().ok_or_else(||anyhow!("unresolved DAE id {id}"))
}
#[derive(Clone,Copy,Debug)]
pub(super) struct Vertex{pub position:[f32;3],pub uv:[f32;2],pub color:[f32;4]}
#[derive(Debug)]
pub(super) struct Primitive{pub vertices:Vec<Vertex>,pub material:String,pub attributes:u8}
#[derive(Debug)]
pub(super) struct Mesh{pub primitives:Vec<Primitive>}
#[derive(Debug)]
pub(super) struct Material{
    pub id:Vec<u8>,pub name:Vec<u8>,pub color:[f32;4],pub texture:Option<String>,pub shader:Vec<u8>,
}
#[derive(Debug)]
pub(super) struct Instance{pub geometry:String,pub materials:BTreeMap<String,String>}
#[derive(Debug)]
pub(super) enum Camera{Perspective{yfov:f32,aspect:Option<f32>,near:f32,far:f32},Orthographic{xmag:f32,ymag:f32,near:f32,far:f32}}
#[derive(Debug)]
pub(super) struct Node{
    pub id:Vec<u8>,pub name:Vec<u8>,pub parent:Option<usize>,pub local:Matrix,
    pub channels:ModelLocalTransforms,pub overrides:ModelTransformOverrides,
    pub instances:Vec<Instance>,pub camera:Option<Camera>,pub shader:Vec<u8>,pub visible:bool,
}
#[derive(Debug)]
pub(super) struct Scene{
    pub nodes:Vec<Node>,pub meshes:BTreeMap<String,Mesh>,pub materials:BTreeMap<String,Material>,
}

fn source_values(source:&Xml,ids:&BTreeMap<String,&Xml>)->Result<(Vec<f32>,usize,usize,usize)>{
    let accessor=source.child("technique_common")?.child("accessor")?;
    let array=reference(accessor.required("source")?,ids)?;
    ensure!(array.tag=="float_array","DAE vertex source is not float_array");let data=floats(array)?;
    ensure!(array.required("count")?.parse::<usize>()?==data.len(),"DAE float count mismatch");
    let stride=accessor.attr("stride").unwrap_or("1").parse::<usize>()?;
    let offset=accessor.attr("offset").unwrap_or("0").parse::<usize>()?;
    let count=accessor.required("count")?.parse::<usize>()?;
    ensure!(stride>0&&count.checked_mul(stride).and_then(|n|n.checked_add(offset)).is_some_and(|n|n<=data.len()),"DAE accessor bounds");
    // XYZ/ST order is explicit; named reordering/padding must not be guessed.
    let params:Vec<_>=accessor.named("param").filter_map(|p|p.attr("name")).collect();
    ensure!(params.starts_with(&["X","Y","Z"])||params.starts_with(&["S","T"])||params.starts_with(&["U","V"])||params.starts_with(&["R","G","B"]),"DAE accessor parameter order is unsupported");
    Ok((data,stride,offset,count))
}
fn mesh(node:&Xml,ids:&BTreeMap<String,&Xml>)->Result<Mesh>{
    let mut primitives=Vec::new();
    for item in &node.children {
        if matches!(item.tag.as_str(),"source"|"vertices"|"extra"){continue;}
        ensure!(matches!(item.tag.as_str(),"triangles"|"polylist"),"DAE primitive {} requires independent topology support",item.tag);
        let mut position=None;let mut uv=None;let mut colors=None;let mut stride=0;let mut normals=false;
        for input in item.named("input") {
            let offset=input.required("offset")?.parse::<usize>()?;ensure!(offset<64,"DAE input offset limit");stride=stride.max(offset+1);
            let semantic=input.required("semantic")?;
            if semantic=="VERTEX" {
                let vertices=reference(input.required("source")?,ids)?;
                let p=vertices.named("input").find(|n|n.attr("semantic")==Some("POSITION")).ok_or_else(||anyhow!("DAE vertices lack POSITION"))?;
                position=Some((offset,source_values(reference(p.required("source")?,ids)?,ids)?));
            } else if semantic=="POSITION" {position=Some((offset,source_values(reference(input.required("source")?,ids)?,ids)?));}
            else if semantic=="TEXCOORD" {
                ensure!(input.attr("set").unwrap_or("0")=="0"&&uv.is_none(),"DAE multiple/nonzero UV sets unsupported");
                uv=Some((offset,source_values(reference(input.required("source")?,ids)?,ids)?));
            } else if semantic=="COLOR" {ensure!(colors.is_none()&&input.attr("set").unwrap_or("0")=="0","multiple DAE color sets");colors=Some((offset,source_values(reference(input.required("source")?,ids)?,ids)?));}
            else {ensure!(semantic=="NORMAL","DAE vertex semantic {semantic} unsupported");normals=true;}
        }
        let (position_offset,(positions,ps,po,pc))=position.ok_or_else(||anyhow!("DAE primitive lacks position"))?;
        ensure!(ps>=3&&stride>0,"DAE position stride invalid");
        let raw=indices(item.child("p")?)?;ensure!(raw.len()%stride==0,"DAE index tuple truncated");
        let count=item.required("count")?.parse::<usize>()?;
        // Bound the declaration by its owned index data before allocating
        // triangle counts; a corrupt count must not allocate independently.
        ensure!(count.checked_mul(3).and_then(|n|n.checked_mul(stride))==Some(raw.len()),"DAE primitive count mismatch");
        let counts=if item.tag=="triangles"{vec![3;count]}else{let counts=indices(item.child("vcount")?)?;ensure!(counts.len()==count,"DAE polygon count mismatch");counts};
        // Retain only source triangles; do not invent a polygon diagonal.
        ensure!(counts.iter().all(|n|*n==3),"DAE nontriangle polylist requires source tessellation");
        let mut vertices=Vec::with_capacity(raw.len()/stride);
        for tuple in raw.chunks_exact(stride){
            let index=tuple[position_offset];ensure!(index<pc,"DAE position index out of range");
            let at=po+index*ps;let position=positions[at..at+3].try_into()?;
            let texture=if let Some((offset,(values,us,uo,uc)))=&uv{
                ensure!(*us>=2&&tuple[*offset]<*uc,"DAE UV index/stride invalid");let at=*uo+tuple[*offset]**us;values[at..at+2].try_into()?
            }else{[0.,0.]};
            let color=if let Some((offset,(values,cs,co,cc)))=&colors{
                ensure!(*cs>=3&&tuple[*offset]<*cc,"DAE color index/stride invalid");let at=*co+tuple[*offset]**cs;
                [values[at],values[at+1],values[at+2],if *cs>=4{values[at+3]}else{1.}]
            }else{[1.;4]};
            vertices.push(Vertex{position,uv:texture,color});
        }
        let attributes=1|if normals{2}else{0}|if colors.is_some(){4}else{0}|if uv.is_some(){8}else{0};
        primitives.push(Primitive{vertices,material:item.attr("material").unwrap_or("").to_owned(),attributes});
    }
    Ok(Mesh{primitives})
}

fn material(node:&Xml,ids:&BTreeMap<String,&Xml>)->Result<Material>{
    let effect=reference(node.child("instance_effect")?.required("url")?,ids)?;
    let profile=effect.child("profile_COMMON")?;
    let technique=profile.child("technique")?;
    // The engine uses the material's numeric inputs and its real SHD program;
    // profile_COMMON's model name is not permission to invent fixed lighting.
    let shader=technique.children.iter().find(|n|matches!(n.tag.as_str(),"constant"|"lambert"|"phong"|"blinn")).ok_or_else(||anyhow!("DAE common shader model missing"))?;
    let diffuse=shader.child(if shader.tag=="constant"{"emission"}else{"diffuse"})?;
    let (color,texture)=if let Some(color)=diffuse.optional("color"){
        let f=floats(color)?;ensure!(f.len()==4,"DAE color requires RGBA");(f.try_into().unwrap(),None)
    }else{
        let tex=diffuse.child("texture")?;let sampler=tex.required("texture")?;
        let sampler=profile.named("newparam").find(|n|n.attr("sid")==Some(sampler)).ok_or_else(||anyhow!("DAE sampler sid missing"))?;
        let surface=sampler.child("sampler2D")?.child("source")?.text.trim();
        let surface=profile.named("newparam").find(|n|n.attr("sid")==Some(surface)).ok_or_else(||anyhow!("DAE texture surface sid missing"))?;
        let image_id=surface.child("surface")?.child("init_from")?.text.trim();
        let image=ids.get(image_id).ok_or_else(||anyhow!("DAE texture image missing"))?;
        ([1.;4],Some(image.child("init_from")?.text.trim().to_owned()))
    };
    ensure!(shader.optional("transparent").is_none()&&shader.optional("transparency").is_none(),"DAE transparency equation requires independent support");
    Ok(Material{id:node.required("id")?.as_bytes().to_vec(),name:node.attr("name").unwrap_or("").as_bytes().to_vec(),color,texture,shader:dynamic(node,"Shader").unwrap_or("").as_bytes().to_vec()})
}
fn dynamic<'a>(node:&'a Xml,key:&str)->Option<&'a str>{
    // 499032 /49ad1e: exact dynamic_attributes child "Shader" (case-sensitive).
    for extra in node.named("extra") {for technique in &extra.children {
        if let Some(attributes)=technique.optional("dynamic_attributes") {
            if let Some(value)=attributes.optional(key){return Some(value.text.trim());}
        }
    }}None
}
fn scalar(node:&Xml,key:&str)->Result<f32>{let f=floats(node.child(key)?)?;ensure!(f.len()==1,"DAE {key} is not a scalar");Ok(f[0])}
fn camera(node:&Xml)->Result<Camera>{
    let common=node.child("optics")?.child("technique_common")?;
    if let Some(p)=common.optional("perspective") {
        let aspect=p.optional("aspect_ratio").map(|_|scalar(p,"aspect_ratio")).transpose()?;
        let yfov=if p.optional("yfov").is_some(){scalar(p,"yfov")?.to_radians()}
            else {let a=aspect.ok_or_else(||anyhow!("xfov requires aspect ratio"))?;2.*((scalar(p,"xfov")?.to_radians()*0.5).tan()/a).atan()};
        let near=scalar(p,"znear")?;let far=scalar(p,"zfar")?;
        ensure!(yfov>0.&&yfov<std::f32::consts::PI&&near>0.&&far>near&&aspect.is_none_or(|v|v>0.),"invalid DAE perspective");
        Ok(Camera::Perspective{yfov,aspect,near,far})
    }else{let p=common.child("orthographic")?;let xmag=scalar(p,"xmag")?;let ymag=scalar(p,"ymag")?;
        let near=scalar(p,"znear")?;let far=scalar(p,"zfar")?;ensure!(xmag>0.&&ymag>0.&&far>near,"invalid DAE orthographic");Ok(Camera::Orthographic{xmag,ymag,near,far})}
}
fn local(node:&Xml)->Result<Matrix>{
    let mut matrix=identity();
    for item in &node.children{
        let m=match item.tag.as_str(){
            "matrix"=>{let f=floats(item)?;ensure!(f.len()==16,"DAE matrix needs16 values");std::array::from_fn(|r|std::array::from_fn(|c|f[r*4+c]))},
            "translate"=>{let f=floats(item)?;ensure!(f.len()==3,"DAE translate needs3 values");let mut m=identity();m[3][..3].copy_from_slice(&f);m},
            "scale"=>{let f=floats(item)?;ensure!(f.len()==3,"DAE scale needs3 values");let mut m=identity();for i in 0..3{m[i][i]=f[i];}m},
            "rotate"=>{let f=floats(item)?;ensure!(f.len()==4,"DAE rotate needs4 values");let length=(f[0]*f[0]+f[1]*f[1]+f[2]*f[2]).sqrt();ensure!(length>0.,"zero DAE rotation axis");
                let [x,y,z]=[f[0]/length,f[1]/length,f[2]/length];let(s,c)=f[3].to_radians().sin_cos();let t=1.-c;
                [[t*x*x+c,t*x*y+s*z,t*x*z-s*y,0.],[t*x*y-s*z,t*y*y+c,t*y*z+s*x,0.],[t*x*z+s*y,t*y*z-s*x,t*z*z+c,0.],[0.,0.,0.,1.]]},
            "lookat"|"skew"=>bail!("DAE {} transform not recovered",item.tag),_=>continue,
        };
        // COLLADA column matrices postmultiply in XML order; our row-vector
        // representation is their transpose, so prepend each transform.
        matrix=multiply(m,matrix);
    }
    Ok(matrix)
}
fn add_node(node:&Xml,parent:Option<usize>,ids:&BTreeMap<String,&Xml>,nodes:&mut Vec<Node>,depth:usize)->Result<()> {
    ensure!(depth<=128&&nodes.len()<65536,"DAE scene graph limit");
    ensure!(node.optional("instance_controller").is_none()&&node.optional("instance_light").is_none()&&node.optional("instance_node").is_none(),"DAE controller/light/shared-node graph requires independent support");
    let mut instances=Vec::new();
    for instance in node.named("instance_geometry"){
        let geometry=reference(instance.required("url")?,ids)?.required("id")?.to_owned();let mut materials=BTreeMap::new();
        if let Some(bind)=instance.optional("bind_material"){
            for binding in bind.child("technique_common")?.named("instance_material") {
                let target=reference(binding.required("target")?,ids)?.required("id")?.to_owned();
                materials.insert(binding.required("symbol")?.to_owned(),target);
            }
        }
        instances.push(Instance{geometry,materials});
    }
    let cameras:Vec<_>=node.named("instance_camera").collect();ensure!(cameras.len()<=1,"multiple DAE cameras on one node");
    let camera=cameras.first().map(|n|camera(reference(n.required("url")?,ids)?)).transpose()?;
    let index=nodes.len();nodes.push(Node{id:node.attr("id").unwrap_or("").as_bytes().to_vec(),name:node.attr("name").unwrap_or("").as_bytes().to_vec(),parent,local:local(node)?,channels:Default::default(),overrides:Default::default(),instances,camera,shader:dynamic(node,"Shader").unwrap_or("").as_bytes().to_vec(),visible:dynamic(node,"Visible").map_or(Ok(true),|s|s.parse::<i32>().map(|v|v!=0))?});
    for child in node.named("node"){add_node(child,Some(index),ids,nodes,depth+1)?;}Ok(())
}
impl Scene {
    pub(super) fn parse(bytes:&[u8])->Result<Self>{
        let root=document(bytes)?;ensure!(root.tag=="COLLADA"&&matches!(root.attr("version"),Some("1.4.0"|"1.4.1")),"DAE version is not COLLADA1.4.0/1.4.1");
        if let Some(asset)=root.optional("asset"){
            ensure!(asset.optional("up_axis").is_none_or(|axis|axis.text.trim()=="Y_UP"),"DAE non-Y_UP basis needs the original485fb0 conversion");
            if let Some(unit)=asset.optional("unit") {ensure!(unit.attr("meter").unwrap_or("1").parse::<f32>()?==1.,"DAE non-meter asset unit requires an explicit basis conversion");}
        }
        ensure!(root.optional("library_animations").is_none()&&root.optional("library_controllers").is_none(),"animated/skinned DAE is not a static mesh");
        let mut ids=BTreeMap::new();root.descend(&mut ids)?;
        let mut meshes=BTreeMap::new();let mut materials=BTreeMap::new();
        if let Some(library)=root.optional("library_geometries") {for geometry in library.named("geometry"){meshes.insert(geometry.required("id")?.to_owned(),mesh(geometry.child("mesh")?,&ids)?);}}
        if let Some(library)=root.optional("library_materials") {for mat in library.named("material"){materials.insert(mat.required("id")?.to_owned(),material(mat,&ids)?);}}
        let scene=reference(root.child("scene")?.child("instance_visual_scene")?.required("url")?,&ids)?;
        ensure!(scene.tag=="visual_scene","DAE scene reference is not visual_scene");
        let mut nodes=Vec::new();
        // 498aca..498af8: a separate root has no DAE id. A present scene name
        // is formatted with the exact @-TOPNODE-%s literal, not copied as-is.
        let root_name=scene.attr("name").map(|name|format!("@-TOPNODE-{name}")).unwrap_or_default();
        nodes.push(Node{id:Vec::new(),name:root_name.into_bytes(),parent:None,local:identity(),channels:Default::default(),overrides:Default::default(),instances:Vec::new(),camera:None,shader:Vec::new(),visible:true});
        for node in scene.named("node"){add_node(node,Some(0),&ids,&mut nodes,0)?;}
        Ok(Self{nodes,meshes,materials})
    }
    pub(super) fn worlds(&self)->Result<Vec<Matrix>>{
        let mut out=Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {let parent=node.parent.map_or(identity(),|i|out[i]);out.push(node.overrides.world_matrix(node.local,parent,&node.channels)?);}Ok(out)
    }
    pub(super) fn find_node(&self,id:&[u8],name:&[u8])->Option<usize>{
        self.nodes.iter().position(|n|n.id.eq_ignore_ascii_case(id)&&n.name.eq_ignore_ascii_case(name))
    }
}
