//! Direct texture background consumers: wc_rotbg1390/14f0 and
//! blur1c60/1e30/2430. No PE is called; actual texture/viewport dimensions
//! feed the shared matrix and Canvas snapshot.
use super::*;

#[derive(Debug,Default)]
pub(super) struct BackgroundConfig {
    size:[f32;3],
    angles:[f32;3],
    color:[u8;3],
    color_range:[u8;3],
}

impl PathFamily {
    pub(super) fn start_rotbg(&mut self,p:&[u8;128]) {
        self.center=[word(p,1),word(p,2),0.];let size=word(p,3)/100.;
        //140a stores0.01 when the input falls below0.001.
        self.scale=if 0.001>size {0.01} else {size};self.factor=word(p,4)/10000.;self.direction=0;
        self.main_count=self.drops.len();self.active_count=self.main_count;
        for d in &mut self.drops {
            d.pivot=self.image.map(|v|v*0.5);d.position=self.center;d.scale=self.scale;d.stretch=[1.;2];
            d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=255;d.clock=0;d.phase_origin=0;d.phase=0;
            //1390 does not reset SDK rotation. Repeated start retains it.
        }
    }

    pub(super) fn update_rotbg(&mut self,tick:u32,p:&[u8;128]) {
        if word(p,0)!=self.direction as f32 {self.direction=trunc(word(p,0));self.factor=word(p,4)/10000.;}
        for d in &mut self.drops {
            if self.direction==0 {
                if d.phase==0 {d.rotation+=self.factor;} else {d.position[..2].copy_from_slice(&self.center[..2]);}
            }
            if self.direction==1 && d.phase!=0 {d.position[..2].copy_from_slice(&self.center[..2]);}
            d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn start_blur(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.background.size=[word(p,4)/100.,word(p,5)/100.,1.];
        self.background.angles=std::array::from_fn(|i|
            (f64::from(word(p,i+6))*f64::from(std::f32::consts::PI)/180.) as f32);
        self.background.color=std::array::from_fn(|i|trunc(word(p,i+9)) as u8);
        self.background.color_range=std::array::from_fn(|i|trunc(word(p,i+12)) as u8);
        if trunc(word(p,15))!=0 {self.stream_render_mode=1;}
        self.main_count=self.drops.len();self.active_count=self.main_count;self.direction=0;
        let n=self.main_count as f32;let width=self.image[0];let step=width/n;let uv_step=1./n;
        for (i,d) in self.drops.iter_mut().enumerate() {
            d.path=Trajectory::default();let y=i as f32*step;
            for point in [[-width*0.5,y,0.],[width*0.5,y,0.],[-width*0.5,y+step,0.],[width*0.5,y+step,0.]] {
                d.path.append(point);
            }
            d.uv=std::array::from_fn(|j|[(j&1) as f32,i as f32*uv_step+(j>>1) as f32*uv_step]);
            //2430 samples all three initial channels from B/B-range.1e30
            //uses B,G,R separately on the first and each subsequent frame.
            let rgb=std::array::from_fn(|_|random_color(self.background.color[0],self.background.color_range[0],random));
            d.vertex_rgb=Some([pack_color(rgb);4]);d.alpha=0;
            d.motion_limit=word(p,3);d.saved_alpha=255.;d.clock=0;d.phase_origin=0;d.phase=0;
            d.durations=[trunc((f64::from(sample(random))*10.+10.) as f32),0,
                trunc((f64::from(sample(random))*20.+20.) as f32)];
        }
    }

    pub(super) fn update_blur(&mut self,tick:u32,p:&[u8;128],offset:[f32;2],random:&mut impl ParticleRandomSource) {
        self.direction=trunc(word(p,0));
        let m=matrix::affine(self.background.size,self.background.angles,[word(p,1)+offset[0],word(p,2)+offset[1],0.]);
        for d in &mut self.drops {
            let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            //The0x2410 jump table maps phase0 directly to2038. Public start
            //never creates phase1. Preserve that state and the discarded
            //phase2 interpolation; its result is not the submitted alpha.
            if d.phase==1 && age>d.durations[1] as f32 {d.phase=2;d.phase_origin=d.clock;}
            else if d.phase==2 && age>d.durations[2] as f32 {d.phase=3;d.phase_origin=d.clock;}
            let rgb=std::array::from_fn(|i|random_color(self.background.color[i],self.background.color_range[i],random));
            d.vertex_rgb=Some([pack_color(rgb);4]);d.alpha=255;
            let points=std::array::from_fn(|i| {
                let mut point=d.path.points[i];
                if self.direction!=0 {
                    let amplitude=if random.next_integer()&127==0 {d.motion_limit*50.} else {d.motion_limit};
                    point[0]+=(sample(random)*2.-1.)*amplitude;
                }
                matrix::transform(m,point)
            });
            //The plugin writes final XYZ directly to SDK vertices, leaving
            //the default SDK transform intact; do not apply it a second time.
            d.geometry=Some(points);d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn random_color(base:u8,range:u8,random:&mut impl ParticleRandomSource)->u8 {
    let value=(f64::from(sample(random))*f64::from(range)+f64::from(base)) as f32;
    trunc(value.max(f32::from(base)).min(255.)) as u8
}
fn pack_color(c:[u8;3])->u32 {u32::from(c[0])|(u32::from(c[1])<<8)|(u32::from(c[2])<<16)}
