//! CPU triangle-list sink for the recovered MDL SHD equations.
use cs2_renderer::{Canvas, RgbaFrame};
use super::shader::Program;

#[derive(Clone,Copy)]
pub(super) struct Vertex {
    pub clip:[f32;4],
    pub color:[f32;4],
    pub uv:[f32;2],
    // factor*clip.w: homogeneous clipping preserves a screen-linear oFog
    // without dividing by a potentially zero/negative W at a clip edge.
    pub fog_w:f32,
}
fn clip(input:&[Vertex], plane:usize)->Vec<Vertex> {
    let distance=|p:Vertex|match plane {0=>p.clip[0]+p.clip[3],1=>p.clip[3]-p.clip[0],
        2=>p.clip[1]+p.clip[3],3=>p.clip[3]-p.clip[1],4=>p.clip[2],_=>p.clip[3]-p.clip[2]};
    let mut out=Vec::new();
    let Some(&mut_before)=input.last() else {return out;};
    let mut before=mut_before;
    let mut d0=distance(before);
    for &after in input {
        let d1=distance(after);
        if (d0>=0.)!=(d1>=0.) {
            let t=d0/(d0-d1);
            out.push(Vertex {clip:std::array::from_fn(|i|before.clip[i]+t*(after.clip[i]-before.clip[i])),
                color:std::array::from_fn(|i|before.color[i]+t*(after.color[i]-before.color[i])),
                uv:std::array::from_fn(|i|before.uv[i]+t*(after.uv[i]-before.uv[i])),
                fog_w:before.fog_w+t*(after.fog_w-before.fog_w)});
        }
        if d1>=0. {out.push(after);}
        before=after;d0=d1;
    }
    out
}
fn texture(frame:&RgbaFrame, uv:[f32;2], linear:bool)->[f32;4] {
    // AddressU/V=WRAP comes from the selected SHD. The view's configured
    // filter selects point or linear; the shipped startup requests linear.
    let width=frame.width() as i64;
    let height=frame.height() as i64;
    let at=|x:i64,y:i64| (y.rem_euclid(height)*width+x.rem_euclid(width)) as usize*4;
    let x=uv[0].rem_euclid(1.)*width as f32;
    let y=uv[1].rem_euclid(1.)*height as f32;
    if !linear {
        let at=at(x.floor() as i64,y.floor() as i64);
        return std::array::from_fn(|i|frame.pixels()[at+i] as f32/255.);
    }
    let x=x-0.5;let y=y-0.5;
    let x0=x.floor() as i64;let y0=y.floor() as i64;
    let fx=x-x.floor();let fy=y-y.floor();
    let mut result=[0.;4];
    for (x,y,weight) in [(x0,y0,(1.-fx)*(1.-fy)),(x0+1,y0,fx*(1.-fy)),
        (x0,y0+1,(1.-fx)*fy),(x0+1,y0+1,fx*fy)] {
        let at=at(x,y);
        for i in 0..4 {result[i]+=weight*frame.pixels()[at+i] as f32/255.;}
    }
    result
}

pub(super) fn triangle(output:&mut Canvas, depth:&mut[f32], vertices:[Vertex;3], frame:Option<&RgbaFrame>, program:&Program, linear:bool, fog_rgb:Option<[f32;3]>)->anyhow::Result<()> {
    if vertices.iter().any(|v|!v.fog_w.is_finite() || v.clip.iter().chain(v.color.iter()).chain(v.uv.iter()).any(|x|!x.is_finite())) {
        anyhow::bail!("MDL vertex shader produced a non-finite value");
    }
    if frame.is_some_and(|frame|frame.width()==0 || frame.height()==0) {
        anyhow::bail!("MDL bound texture has no texels");
    }
    let mut polygon=vertices.to_vec();
    for plane in 0..6 {polygon=clip(&polygon,plane);}
    for i in 1..polygon.len().saturating_sub(1) {
        raster(output,depth,[polygon[0],polygon[i],polygon[i+1]],frame,program,linear,fog_rgb);
    }
    Ok(())
}
fn raster(output:&mut Canvas, depth:&mut[f32], mut vertices:[Vertex;3], frame:Option<&RgbaFrame>, program:&Program, linear:bool, fog_rgb:Option<[f32;3]>) {
    if vertices.iter().any(|v|v.clip[3]<=0.) {return;}
    let width=output.width() as usize;
    let height=output.height() as usize;
    let screen=|v:Vertex|[(v.clip[0]/v.clip[3]+1.)*width as f32*0.5,(1.-v.clip[1]/v.clip[3])*height as f32*0.5];
    let mut positions=vertices.map(screen);
    let edge=|a:[f32;2],b:[f32;2],p:[f32;2]|(b[0]-a[0])*(p[1]-a[1])-(b[1]-a[1])*(p[0]-a[0]);
    let mut area=edge(positions[0],positions[1],positions[2]);
    if area==0.||!area.is_finite()||(program.cull_cw&&area>0.) {return;}
    if area<0. {positions.swap(1,2);vertices.swap(1,2);area=-area;}
    let min_x=positions.iter().map(|p|p[0]).fold(f32::INFINITY,f32::min).ceil().max(0.) as usize;
    let min_y=positions.iter().map(|p|p[1]).fold(f32::INFINITY,f32::min).ceil().max(0.) as usize;
    let max_x=positions.iter().map(|p|p[0]).fold(f32::NEG_INFINITY,f32::max).floor().min(width as f32-1.) as usize;
    let max_y=positions.iter().map(|p|p[1]).fold(f32::NEG_INFINITY,f32::max).floor().min(height as f32-1.) as usize;
    if min_x>=width||min_y>=height||min_x>max_x||min_y>max_y {return;}
    let top_left=|a:[f32;2],b:[f32;2]|b[1]<a[1]||(b[1]==a[1]&&b[0]>a[0]);
    let edges=[(positions[1],positions[2]),(positions[2],positions[0]),(positions[0],positions[1])];
    let pixels=output.pixels_mut();
    for y in min_y..=max_y {for x in min_x..=max_x {
        // D3D8 viewport coordinates place pixel centers at integer positions.
        let e=edges.map(|(a,b)|edge(a,b,[x as f32,y as f32]));
        if e.iter().enumerate().any(|(i,&v)|v<0.||(v==0.&&!top_left(edges[i].0,edges[i].1))) {continue;}
        let bary=e.map(|v|v/area);
        let z=(0..3).map(|i|bary[i]*vertices[i].clip[2]/vertices[i].clip[3]).sum::<f32>();
        let pixel=y*width+x;
        if z>depth[pixel] {continue;}
        let inverse_w=(0..3).map(|i|bary[i]/vertices[i].clip[3]).sum::<f32>();
        if inverse_w<=0. {continue;}
        let interpolate=|channel:usize,uv:bool| (0..3).map(|i|bary[i]/vertices[i].clip[3]*
            if uv {vertices[i].uv[channel]} else {vertices[i].color[channel]}).sum::<f32>()/inverse_w;
        let mut color=std::array::from_fn::<_,4,_>(|channel|interpolate(channel,false).clamp(0.,1.));
        if let Some(frame)=frame {
            let sample=texture(frame,[interpolate(0,true),interpolate(1,true)],linear);
            for channel in 0..4 {color[channel]*=sample[channel];}
        }
        if let Some(rgb)=fog_rgb {
            // oFog is screen-linear (unlike the perspective UV/color above).
            // Fog follows texture modulation and preserves source alpha.
            let factor=(0..3).map(|i|bary[i]*vertices[i].fog_w/vertices[i].clip[3]).sum::<f32>().clamp(0.,1.);
            for channel in 0..3 {color[channel]=color[channel]*factor+rgb[channel]*(1.-factor);}
        }
        let source=color.map(|v|(v*255.).round().clamp(0.,255.) as u32);
        if source[3]<1 {continue;}
        if program.z_write {depth[pixel]=z;}
        let at=pixel*4;
        let alpha=source[3];
        for channel in 0..4 {
            let destination=u32::from(pixels[at+channel]);
            let value=(source[channel]*alpha+destination*if program.additive {255} else {255-alpha}+127)/255;
            pixels[at+channel]=value.min(255) as u8;
        }
    }}
}
