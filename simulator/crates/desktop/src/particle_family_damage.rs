//! wc_damage1c60/1db0/2320. Independent data-only model of the16x16
//! texture pieces; private moving corners are distinct from SDK vertices.
use super::*;

#[derive(Debug,Default)]
pub(super) struct DamageConfig {pieces:Vec<[[f32;3];4]>,size:[f32;2],calls:u32}

impl PathFamily {
    pub(super) fn start_damage(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.damage.size=[word(p,2)/100.,word(p,3)/100.];self.damage.calls=0;
        self.main_count=self.drops.len();self.active_count=1;self.child_count=0;
        self.damage.pieces.clear();
        //2320 receives only the texture WIDTH in xmm2, including for Y.
        let edge=self.image[0]/16.;let half=self.image[0]*0.5;
        for (index,d) in self.drops.iter_mut().enumerate() {
            let x=(index%16) as f32;let y=(index/16) as f32;
            let corners=std::array::from_fn(|i|[(x+(i&1) as f32)*edge-half,
                (y+(i>>1) as f32)*edge-half,0.]);
            self.damage.pieces.push(corners);
            d.uv=std::array::from_fn(|i|[(x+(i&1) as f32)/16.,(y+(i>>1) as f32)/16.]);
            d.vertex_rgb=Some([0x001e_1e1e;4]);d.vertex_alpha=None;d.alpha=0;
            d.motion=std::array::from_fn(|_|(sample(random)*2.-1.)*0.1f32);
            d.saved_alpha=255.;
            d.durations=[trunc((f64::from(sample(random))*10.+10.) as f32),0,
                trunc((f64::from(sample(random))*20.+20.) as f32)];
            d.clock=0;d.phase_origin=0;d.phase=0;
        }
    }

    pub(super) fn update_damage(&mut self,tick:u32,p:&[u8;128],offset:[f32;2],random:&mut impl ParticleRandomSource) {
        let c=&mut self.damage;
        if c.calls>1 {self.active_count=(self.active_count+16).min(self.main_count);c.calls=0;}
        let matrix=matrix::affine([c.size[0],c.size[1],1.],[0.;3],
            [word(p,0)+offset[0],word(p,1)+offset[1],0.]);
        for (d,points) in self.drops.iter_mut().zip(&mut c.pieces).take(self.active_count) {
            let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            match d.phase {
                0=>{
                    d.alpha=trunc(super::super::lerp(0.,d.saved_alpha,d.durations[0] as f32,age)) as u8;
                    if age>d.durations[0] as f32 {d.phase=1;d.phase_origin=d.clock;}
                },
                1=>{
                    d.alpha=d.saved_alpha as u8;
                    if age>d.durations[1] as f32 {d.phase=2;d.phase_origin=d.clock;}
                },
                2=>{
                    d.alpha=trunc(super::super::lerp(d.saved_alpha,0.,d.durations[2] as f32,age)) as u8;
                    if age>d.durations[2] as f32 {d.phase=3;d.phase_origin=d.clock;}
                },
                _=>d.alpha=0,
            }
            //The public update still moves/recolors phase3; it never reseeds.
            let b=trunc((f64::from(sample(random))*100.+155.) as f32) as u8 as u32;
            let g=trunc((f64::from(sample(random))*100.+155.) as f32) as u8 as u32;
            let r=trunc((f64::from(sample(random))*100.+155.) as f32) as u8 as u32;
            d.vertex_rgb=Some([b|(g<<8)|(r<<16);4]);
            for point in &mut *points {*point=add(*point,d.motion);}
            d.geometry=Some(points.map(|point|matrix::transform(matrix,point)));
            d.clock=d.clock.wrapping_add(tick);
        }
        c.calls=c.calls.wrapping_add(1);
    }
}
