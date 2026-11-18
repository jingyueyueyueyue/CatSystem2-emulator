//! wc_pond1c60/1e30/2510: actual private matrix vertices, per-quad
//! depth bias, independent circular spawn and reusable three-stage pool.
use super::*;

#[derive(Debug,Default)]
pub(super) struct PondConfig {center:[f32;3],angles:[f32;3],size:f32}

impl PathFamily {
    pub(super) fn start_pond(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.pond=PondConfig {center:std::array::from_fn(|i|word(p,i)),
            angles:std::array::from_fn(|i|((f64::from(word(p,i+3))*f64::from(std::f32::consts::PI))/180.) as f32),
            size:word(p,7)/100.};
        if word(p,8)!=0. {self.stream_render_mode=1;}
        //p6 is retained by1c60 but never consumed by1e30/2510.
        self.main_count=self.drops.len();self.active_count=0;
        for(index,d) in self.drops.iter_mut().enumerate() {
            d.depth_bias=index as f32*0.00048;d.position=[0.;3];
            spawn_pond(d,self.image,&self.pond,random);
        }
    }

    pub(super) fn update_pond(&mut self,tick:u32,offset:[f32;2],random:&mut impl ParticleRandomSource) {
        self.active_count=self.active_count.saturating_add(1).min(self.main_count);
        let c=&self.pond;let parent=matrix::affine([1.;3],c.angles,c.center);
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase>2 {spawn_pond(d,self.image,c,random);}
            else {
                d.motion[2]+=d.angular_step;d.captured[1]*=d.motion_limit;
                let now=d.clock as i32;
                let age=d.clock.wrapping_sub(d.phase_origin) as i32;
                match d.phase {
                    0=>{
                        d.alpha=trunc(super::super::lerp(0.,100.,d.durations[0] as f32,now as f32)) as u8;
                        if now>d.durations[0] {d.phase=1;d.phase_origin=d.clock;}
                    },
                    1=>if now>d.durations[1] {d.phase=2;d.phase_origin=d.clock;},
                    _=>{
                        d.alpha=trunc(super::super::lerp(100.,0.,d.durations[2] as f32,age as f32)) as u8;
                        if age>d.durations[2] {d.phase=3;d.phase_origin=d.clock;}
                    },
                }
            }
            let local=matrix::affine(d.captured,d.motion,d.motion_origin);
            let combined=matrix::product(local,parent);
            d.geometry=Some(std::array::from_fn(|i|matrix::transform(combined,
                [(i&1) as f32*self.image[0],(i>>1) as f32*self.image[1],0.])));
            //Positions were explicitly baked to SDK vertices. Its own size
            //stays identity; the separately written pivot/translation still
            //participate in the existing real snapshot/Canvas consumer.
            d.position[0]+=offset[0];d.position[1]+=offset[1];
            d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn spawn_pond(d:&mut Drop,image:[f32;2],c:&PondConfig,random:&mut impl ParticleRandomSource) {
    let tile=random.next_integer()&3;
    d.uv=std::array::from_fn(|i|[((tile&1)+(i&1) as u32) as f32*0.5,
        ((tile>>1)+(i>>1) as u32) as f32*0.5]);
    d.pivot=image.map(|v|v*0.5);d.scale=1.;d.stretch=[1.;2];d.captured=[c.size,c.size,1.];
    d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;
    //2510 does not reset private64/68/6c; rotation survives respawns.
    let angle=(f64::from(sample(random))*f64::from(std::f32::consts::TAU)) as f32;
    let sy=f64::from(angle).sin() as f32;let rx=sample(random);
    let x=(sy*rx)*150.+(1.-c.size)*(image[0]*0.5);
    let cx=f64::from(angle).cos() as f32;let ry=sample(random);
    let y=(cx*ry)*150.+(1.-c.size)*(image[1]*0.5);
    d.motion_origin=[x,y,0.];
    d.durations=std::array::from_fn(|i| {let base=[60.,20.,60.][i];trunc((sample(random)-0.5)*base+base)});
    let sign=(f64::from(sample(random))*2.-1.) as f32;
    d.angular_step=(f64::from(sample(random))*f64::from(0.0015f32)*f64::from(sign)) as f32;
    d.motion_limit=(1.-f64::from(sample(random))*f64::from(0.003f32)) as f32;
    d.clock=0;d.phase=0;
}
