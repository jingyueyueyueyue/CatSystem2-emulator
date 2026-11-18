//! Real mesh raster sink: homogeneous clipping, perspective UVs, depth and
//! the recovered SHD equations. No proxy rectangle or guessed shader name.
use anyhow::{Result,anyhow,ensure};
use cs2_renderer::{Canvas,RgbaFrame};
use super::css_dae::{Matrix,Camera,multiply,transform};
use super::css_model::Owner;
use super::css_model_shader::{Shader,Diffuse,Texture,Cull,Blend};

#[derive(Clone,Copy)]
struct Vertex{clip:[f32;4],uv:[f32;2],color:[f32;4]}
#[derive(Clone,Copy)]
struct Screen{point:[f32;3],rhw:f32,uv:[f32;2],color:[f32;4]}
fn inverse(matrix:Matrix)->Result<Matrix>{
    let mut rows=[[0f64;8];4];for r in 0..4{for c in 0..4{rows[r][c]=matrix[r][c] as f64;}rows[r][4+r]=1.;}
    for c in 0..4{let pivot=(c..4).max_by(|&a,&b|rows[a][c].abs().total_cmp(&rows[b][c].abs())).unwrap();
        ensure!(rows[pivot][c]!=0.,"singular DAE camera world matrix");rows.swap(c,pivot);let divisor=rows[c][c];for value in &mut rows[c]{*value/=divisor;}
        let row=rows[c];for r in 0..4{if r!=c{let factor=rows[r][c];for k in 0..8{rows[r][k]-=row[k]*factor;}}}
    }
    Ok(std::array::from_fn(|r|std::array::from_fn(|c|rows[r][c+4] as f32)))
}
fn clip(input:Vec<Vertex>,plane:usize)->Vec<Vertex>{
    let distance=|v:Vertex|match plane{0=>v.clip[0]+v.clip[3],1=>v.clip[3]-v.clip[0],2=>v.clip[1]+v.clip[3],3=>v.clip[3]-v.clip[1],4=>v.clip[2],_=>v.clip[3]-v.clip[2]};
    let mut out=Vec::new();let Some(mut previous)=input.last().copied()else{return out;};let mut before=distance(previous);
    for next in input{let after=distance(next);
        if (before>=0.)!=(after>=0.){let t=before/(before-after);out.push(Vertex{clip:std::array::from_fn(|i|previous.clip[i]+t*(next.clip[i]-previous.clip[i])),uv:std::array::from_fn(|i|previous.uv[i]+t*(next.uv[i]-previous.uv[i])),color:std::array::from_fn(|i|previous.color[i]+t*(next.color[i]-previous.color[i]))});}
        if after>=0.{out.push(next);}previous=next;before=after;
    }out
}
pub(super) fn draw(owner:&Owner,canvas:&mut Canvas)->Result<()>{
    let scene=owner.scene.as_ref().ok_or_else(||anyhow!("DAE draw has no live scene"))?;
    if scene.nodes.iter().all(|node|node.instances.is_empty()){return Ok(());}
    let worlds=scene.worlds()?;let camera=owner.camera.ok_or_else(||anyhow!("legacy DAE has no explicitly selected camera; no default projection is invented"))?;
    let view=inverse(*worlds.get(camera).ok_or_else(||anyhow!("DAE selected camera node was released"))?)?;
    let projection=match scene.nodes[camera].camera.as_ref().ok_or_else(||anyhow!("DAE selected node is not a camera"))?{
        Camera::Perspective{yfov,aspect,near,far}=>{
            let aspect=aspect.unwrap_or(canvas.width() as f32/canvas.height() as f32);let y=1./(*yfov*0.5).tan();let z=*far/(*near-*far);
            [[y/aspect,0.,0.,0.],[0.,y,0.,0.],[0.,0.,z,-1.],[0.,0.,z**near,0.]]
        },
        Camera::Orthographic{xmag,ymag,near,far}=>[[1. / *xmag,0.,0.,0.],[0.,1. / *ymag,0.,0.],[0.,0.,1./(*near-*far),0.],[0.,0.,*near/(*near-*far),1.]],
    };
    let count=(canvas.width() as usize).checked_mul(canvas.height() as usize).ok_or_else(||anyhow!("DAE depth surface overflow"))?;
    ensure!(count<=64*1024*1024,"DAE depth surface limit");let mut depth=vec![1f32;count];
    for(index,node)in scene.nodes.iter().enumerate(){
        let mut current=Some(index);let mut visible=true;while let Some(i)=current{visible&=scene.nodes[i].visible;current=scene.nodes[i].parent;}if !visible{continue;}
        let matrix=multiply(multiply(worlds[index],view),projection);
        for instance in &node.instances{
            let mesh=scene.meshes.get(&instance.geometry).ok_or_else(||anyhow!("DAE instance mesh was not loaded"))?;
            for primitive in &mesh.primitives{
                let material_id=instance.materials.get(&primitive.material).ok_or_else(||anyhow!("DAE symbol {} lacks its actual bound material",primitive.material))?;
                let material=scene.materials.get(material_id).ok_or_else(||anyhow!("DAE bound material is absent"))?;
                let node_shader=owner.node_shaders.get(&index).filter(|s|!s.is_empty()).unwrap_or(&node.shader);
                let material_shader=owner.material_shaders.get(material_id).filter(|s|!s.is_empty()).unwrap_or(&material.shader);
                let name=if node_shader.is_empty(){material_shader}else{node_shader};
                let shader=owner.shaders.get(&(name.clone(),primitive.attributes)).ok_or_else(||anyhow!("DAE mesh has no loaded exact SHD program: {:?}",name))?;
                let texture=match shader.texture{Texture::None=>None,
                    Texture::Base=>Some(owner.base.as_ref().and_then(|(_,image)|image.as_ref()).ok_or_else(||anyhow!("DAE Shader reads missing tlts base image"))?),
                    Texture::Material=>Some(owner.images.get(material.texture.as_ref().ok_or_else(||anyhow!("DAE Shader reads tex0 but material has no image"))?).ok_or_else(||anyhow!("DAE material image is not loaded"))?),};
                for triangle in primitive.vertices.chunks_exact(3){
                    let mut polygon=Vec::with_capacity(3);
                    for vertex in triangle{
                        let color=match shader.diffuse{Diffuse::Material=>material.color,Diffuse::Vertex=>vertex.color,Diffuse::Product=>std::array::from_fn(|i|material.color[i]*vertex.color[i])};
                        let position=transform([vertex.position[0],vertex.position[1],vertex.position[2],1.],matrix);
                        ensure!(position.iter().chain(color.iter()).all(|v|v.is_finite()),"DAE transformed vertex is nonfinite");
                        polygon.push(Vertex{clip:position,uv:vertex.uv,color});
                    }
                    for plane in 0..6{polygon=clip(polygon,plane);}
                    for i in 1..polygon.len().saturating_sub(1){
                        let vertices=[polygon[0],polygon[i],polygon[i+1]].map(|v|{
                            let rhw=1./v.clip[3];Screen{point:[(v.clip[0]*rhw+1.)*canvas.width() as f32*0.5,(1.-v.clip[1]*rhw)*canvas.height() as f32*0.5,v.clip[2]*rhw],rhw,uv:v.uv,color:v.color}
                        });raster(canvas,&mut depth,vertices,shader,texture)?;
                    }
                }
            }
        }
    }Ok(())
}
fn edge(a:[f32;3],b:[f32;3],x:f64,y:f64)->f64{(b[0] as f64-a[0] as f64)*(y-a[1] as f64)-(b[1] as f64-a[1] as f64)*(x-a[0] as f64)}
fn top_left(a:[f32;3],b:[f32;3])->bool{b[1]<a[1]||(b[1]==a[1]&&b[0]>a[0])}
fn raster(canvas:&mut Canvas,depth:&mut[f32],mut v:[Screen;3],shader:&Shader,texture:Option<&RgbaFrame>)->Result<()>{
    ensure!(v.iter().all(|v|v.point.iter().chain(v.uv.iter()).chain([v.rhw].iter()).all(|f|f.is_finite())),"nonfinite DAE projected triangle");
    let mut area=edge(v[0].point,v[1].point,v[2].point[0] as f64,v[2].point[1] as f64);
    if area==0.{return Ok(());}if matches!(shader.cull,Cull::Clockwise)&&area>0.||matches!(shader.cull,Cull::CounterClockwise)&&area<0.{return Ok(());}
    if area<0.{v.swap(1,2);area=-area;}
    let bounds=|axis:usize,limit:u32|{
        (v.iter().map(|v|v.point[axis]).fold(f32::INFINITY,f32::min).ceil().clamp(0.,limit as f32) as i32,
         v.iter().map(|v|v.point[axis]).fold(f32::NEG_INFINITY,f32::max).ceil().clamp(0.,limit as f32) as i32)
    };
    let(left,right)=bounds(0,canvas.width());let(top,bottom)=bounds(1,canvas.height());
    let inclusion=[top_left(v[1].point,v[2].point),top_left(v[2].point,v[0].point),top_left(v[0].point,v[1].point)];
    for y in top..bottom{for x in left..right{
        let e=[edge(v[1].point,v[2].point,x as f64,y as f64),edge(v[2].point,v[0].point,x as f64,y as f64),edge(v[0].point,v[1].point,x as f64,y as f64)];
        if(0..3).any(|i|e[i]<0.||(e[i]==0.&&!inclusion[i])){continue;}let w=e.map(|e|e/area);
        let z=(0..3).map(|i|w[i]*v[i].point[2] as f64).sum::<f64>() as f32;let at=y as usize*canvas.width() as usize+x as usize;
        if shader.z_test&&z>depth[at]{continue;}
        let reciprocal=(0..3).map(|i|w[i]*v[i].rhw as f64).sum::<f64>();if reciprocal==0.{continue;}
        let mut color:[f64;4]=std::array::from_fn(|channel|((0..3).map(|i|w[i]*v[i].rhw as f64*v[i].color[channel] as f64).sum::<f64>()/reciprocal).clamp(0.,1.));
        if let Some(texture)=texture{
            let uv=std::array::from_fn(|axis|(0..3).map(|i|w[i]*v[i].rhw as f64*v[i].uv[axis] as f64).sum::<f64>()/reciprocal);
            let texel=sample(texture,uv,shader.wrap,shader.linear);for c in 0..4{color[c]*=texel[c];}
        }
        if shader.alpha_ref.is_some_and(|alpha|color[3]*255.<alpha as f64){continue;}
        let pixel=at*4;let pixels=canvas.pixels_mut();
        for channel in 0..4{
            let source=color[channel]*255.;let destination=pixels[pixel+channel] as f64;
            let value=match shader.blend{Blend::Opaque=>source,Blend::Alpha=>source*color[3]+destination*(1.-color[3]),Blend::Add=>source*color[3]+destination};
            pixels[pixel+channel]=value.round().clamp(0.,255.) as u8;
        }
        if shader.z_write{depth[at]=z;}
    }}Ok(())
}
fn sample(frame:&RgbaFrame,uv:[f64;2],wrap:bool,linear:bool)->[f64;4]{
    let width=frame.width() as i64;let height=frame.height() as i64;
    if width==0||height==0{return [0.;4];}
    let x=uv[0]*width as f64-0.5;let y=(1.-uv[1])*height as f64-0.5;
    let x0=x.floor() as i64;let y0=y.floor() as i64;let fx=x-x.floor();let fy=y-y.floor();
    let address=|v:i64,size:i64|if wrap{v.rem_euclid(size)}else{v.clamp(0,size-1)};
    if !linear{
        let at=(address((y+0.5).floor() as i64,height)*width+address((x+0.5).floor() as i64,width)) as usize*4;
        return std::array::from_fn(|c|frame.pixels()[at+c] as f64/255.);
    }
    let mut result=[0.;4];
    for(px,py,weight)in[(x0,y0,(1.-fx)*(1.-fy)),(x0+1,y0,fx*(1.-fy)),(x0,y0+1,(1.-fx)*fy),(x0+1,y0+1,fx*fy)]{
        let at=(address(py,height)*width+address(px,width)) as usize*4;
        for c in 0..4{result[c]+=weight*frame.pixels()[at+c] as f64/255.;}
    }result
}
