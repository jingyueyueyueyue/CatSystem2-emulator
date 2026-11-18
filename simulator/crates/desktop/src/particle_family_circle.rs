//! a_light/a_080102: normalized circular placement, distance color,
//! sequential activation and an absolute hold followed by upward fade-out.
use super::*;

impl PathFamily {
    pub(super) fn start_circle_light(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let tuned=self.profile.kind==PathKind::CircleLightTuned;
        self.center=[word(p,0),word(p,1),0.];self.scale=word(p,2)/100.;
        self.radius=if tuned {word(p,3)} else {100.};self.factor=if tuned {word(p,4)/100.} else {0.5};
        self.durations=if tuned {std::array::from_fn(|i|trunc(word(p,i+5)))} else {[20,120,30]};
        self.main_count=self.drops.len();self.active_count=1;
        for d in &mut self.drops {spawn(d,self.image,self.center,self.scale,self.radius,tuned,random);}
    }

    pub(super) fn update_circle_light(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        self.active_count=(self.active_count+2).min(self.main_count);
        let tuned=self.profile.kind==PathKind::CircleLightTuned;let peak=if tuned {150.} else {180.};
        for d in self.drops.iter_mut().take(self.active_count) {
            let now=d.clock as i32 as f32;let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            match d.phase {
                0=>{
                    d.alpha=trunc(super::super::lerp(0.,peak,self.durations[0] as f32,now)) as u8;
                    d.position[1]-=self.factor;
                    if d.clock as i32>self.durations[1] {d.phase=1;d.phase_origin=d.clock;}
                },
                1=>{
                    d.alpha=trunc(super::super::lerp(peak,0.,self.durations[2] as f32,age)) as u8;
                    d.position[1]=(f64::from(d.position[1])-f64::from(self.factor)*f64::from(0.4f32)) as f32;
                    if age>self.durations[2] as f32 {d.phase=2;d.phase_origin=d.clock;}
                },
                _=>spawn(d,self.image,self.center,self.scale,self.radius,tuned,random),
            }
            //13d7/1414 capture actual SDK coordinates, with no source offset.
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn spawn(d:&mut Drop,image:[f32;2],center:[f32;3],size:f32,radius:f32,tuned:bool,
    random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.scale=size;d.stretch=[1.;2];
    if tuned {d.uv=[[0.,0.],[0.5,0.],[0.,0.5],[0.5,0.5]];}
    let direction=normalize_nonzero([(f64::from(sample(random))*100.-50.) as f32,
        (f64::from(sample(random))*100.-50.) as f32,0.]);
    let distance=(f64::from(sample(random))*f64::from(radius)) as f32;
    let offset=scale(direction,distance);let position=add(center,offset);
    d.position=[position[0],position[1],(f64::from(sample(random))*50.) as f32];
    let distance=length(sub(position,center));
    let green=trunc(super::super::lerp(150.,40.,80.,distance)) as u8;
    //Both PEs store BGRA20/green/150/0 at all four SDK vertices.
    d.vertex_rgb=Some([20|(u32::from(green)<<8)|(150<<16);4]);d.alpha=0;
    d.clock=0;d.phase=0;d.captured=d.position;
}
