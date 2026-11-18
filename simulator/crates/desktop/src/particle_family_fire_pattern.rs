//! wc_fireptn1a30/1c10/1ef0: real sixteen-frame sprite sheet and
//! source-relative draw state. Its initialized three-knot path is never read.
use super::*;

#[derive(Debug,Default)]
pub(super) struct FirePatternConfig {alpha:i32,interval:i32,duration:i32}

impl PathFamily {
    pub(super) fn start_fire_pattern(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let interval=trunc(word(p,3));
        self.fire_pattern=FirePatternConfig {alpha:trunc(word(p,1)),interval,duration:interval.wrapping_mul(17)};
        if word(p,0)!=0. {self.stream_render_mode=1;}
        self.main_count=self.drops.len();self.active_count=0;
        let size=word(p,2)/100.;
        for d in &mut self.drops {
            d.pivot=self.image.map(|v|v*0.5);d.scale=1.;
            let sx=(sample(random)*size)*0.4+size;
            d.stretch=[sx,(f64::from(sample(random))*f64::from(0.01f32)+f64::from(sx)) as f32];
            //1fae consumes integer%count and converts angle, without using
            //the result;2017 samples another float even with zero X range.
            let _unused=random.next_integer();let _unused=sample(random);
            d.position=[640.,300.,0.];d.clock=0;d.phase_origin=0;d.phase=0;
            d.alpha=0;d.atlas_frame=0;d.motion_limit=0.;
            d.vertex_rgb=Some([0x00ff_ffff;4]);
        }
    }

    pub(super) fn update_fire_pattern(&mut self,tick:u32,offset:[f32;2],random:&mut impl ParticleRandomSource) {
        self.active_count=self.active_count.saturating_add(2).min(self.main_count);
        let c=&self.fire_pattern;
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase==0 {
                d.atlas_frame=0;
                d.motion_origin=[(f64::from(sample(random))*1280.) as f32,
                    (f64::from(sample(random))*300.+350.) as f32,0.];
                d.phase=1;
            }
            if d.phase==1 {
                d.position=d.motion_origin;
                let age=d.clock.wrapping_sub(d.phase_origin) as i32;
                d.alpha=trunc(super::super::lerp(c.alpha as f32,0.,c.duration as f32,age as f32)) as u8;
                d.uv=std::array::from_fn(|i|[((d.atlas_frame&3)+(i&1) as u32) as f32*0.25,
                    ((d.atlas_frame>>2)+(i>>1) as u32) as f32*0.25]);
                if d.motion_limit>c.interval as f32 {
                    d.atlas_frame=d.atlas_frame.saturating_add(1).min(15);d.motion_limit=0.;
                }
                if age>c.duration {d.phase=2;d.phase_origin=d.clock;}
            }
            //Terminal2 still accumulates source offset:1e7d is outside the
            //switch. It does not restart or signal SDK04 completion.
            d.position[0]+=offset[0];d.position[1]+=offset[1];
            d.clock=d.clock.wrapping_add(tick);d.motion_limit+=1.;
        }
    }
}
