//! Independent fireball/fireball2 emitter. Same recovered three-knot core,
//! distinct PE parameter layouts, motion, byte colors and SDK clocks.
use super::*;

#[derive(Debug,Default)]
pub(super) struct FireConfig {
    path: Trajectory,
    clock: u32,
    current: [f32;3],
    previous: [f32;3],
    size: f32,
    speed: f32,
    peak: i32,
    render_mode: u32,
}

impl PathFamily {
    pub(super) fn start_fire(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let old=self.profile.kind==PathKind::Fireball;
        let start=std::array::from_fn(|i|word(p,i+1));
        let end=std::array::from_fn(|i|word(p,i+4));
        self.fire.path=volt::volt_path(start,end,word(p,if old {9} else {8}),
            word(p,if old {10} else {9}),word(p,7));
        self.fire.size=word(p,if old {11} else {10})/100.;
        self.fire.speed=word(p,if old {12} else {11})/10.;
        self.fire.peak=if old {trunc(word(p,8))} else {40};
        if word(p,0)!=0. {self.fire.render_mode=1;}
        self.fire.clock=0;
        // 2592/25bd initialize these in fireball. fireball2 does not write
        // current308 before start's spawn calls; preserve the owned value.
        if old {self.fire.current=start;self.fire.previous=start;}
        self.main_count=self.drops.len();self.active_count=1.min(self.main_count);
        for d in &mut self.drops {
            d.phase_origin=0;
            spawn_fire(d,self.image,&self.fire,old,self.main_count,random);
        }
    }

    pub(super) fn update_fire(&mut self,tick: u32,offset: [f32;2],random: &mut impl ParticleRandomSource) {
        let old=self.profile.kind==PathKind::Fireball;
        self.active_count=self.active_count.saturating_add(if old {2} else {1}).min(self.main_count);
        self.fire.current=self.fire.path.at(self.fire.clock);
        let backward=normalize_nonzero(sub(self.fire.previous,self.fire.current));
        let size=self.fire.size;let speed=self.fire.speed;
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase>2 {
                spawn_fire(d,self.image,&self.fire,old,self.main_count,random);
            } else {
                if d.phase==0 {d.motion_origin=self.fire.current;d.phase=1;}
                let fading=d.phase==2;
                let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
                let clock=d.clock as i32;
                let span=if fading {if old {14} else {30}} else if old {8} else {5};
                d.alpha=trunc(if fading {super::super::lerp(self.fire.peak as f32,0.,span as f32,elapsed as f32)}
                    else {super::super::lerp(0.,self.fire.peak as f32,span as f32,clock as f32)}) as u8;
                let damping=if fading {if old {0.95} else {0.9}} else {0.975};
                let rgb=d.vertex_rgb.unwrap_or([0x00ff_ffff;4]);
                d.vertex_rgb=Some(rgb.map(|v| {
                    let b=trunc((v&255) as f32*damping) as u8;
                    let g=trunc(((v>>8)&255) as f32*damping) as u8;
                    let r=trunc(((v>>16)&255) as f32*damping) as u8;
                    u32::from(b)|(u32::from(g)<<8)|(u32::from(r)<<16)
                }));
                for axis in 0..2 {
                    let drift=if old {backward[axis]*speed*0.5} else {0.};
                    d.motion_origin[axis]+=d.motion[axis]*speed+drift;
                }
                // fireball2's2b43 subtracts absolute clock*.04 from the
                // internal Y on phase2, not a delta-time acceleration.
                if !old && fading {d.motion_origin[1]-=clock as f32*0.04;}
                d.position=d.motion_origin;
                if old {
                    d.stretch[0]=if fading {super::super::lerp(size*0.3,size*0.01,14.,elapsed as f32)}
                        else {super::super::lerp(size*0.5,size*0.3,8.,clock as f32)};
                    if !fading {d.stretch[1]=super::super::lerp(size*0.4,size*0.2,8.,clock as f32);}
                }
                if (if fading {elapsed} else {clock})>span {
                    d.phase+=1;d.phase_origin=d.clock;
                }
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.captured=d.position;
            d.clock=d.clock.wrapping_add(tick);
        }
        self.fire.clock=self.fire.clock.wrapping_add(tick);self.fire.previous=self.fire.current;
    }

    pub(super) fn fire_render_mode(&self) -> u32 {self.fire.render_mode}
}

fn spawn_fire(d: &mut Drop,image: [f32;2],c: &FireConfig,old: bool,count: usize,
    random: &mut impl ParticleRandomSource) {
    d.pivot=image.map(|n|n*0.5);
    let tile=random.next_integer()&if old {15} else {3};
    let span=if old {0.25} else {0.5};
    let columns=if old {4} else {2};
    let origin=[(tile%columns) as f32*span,(tile/columns) as f32*span];
    d.uv=std::array::from_fn(|i|[origin[0]+(i&1) as f32*span,origin[1]+(i>>1) as f32*span]);
    d.scale=1.;d.stretch=[c.size*0.5,c.size*if old {0.4} else {0.5}];
    d.vertex_rgb=Some([if old {0x00ff_ffff} else {0x0032_3232};4]);d.alpha=0;
    let degrees=(random.next_integer()%count as u32) as f32*(360./count as f32);
    let radians=((f64::from(degrees)*f64::from(std::f32::consts::PI))/180.) as f32;
    let radians=trunc(radians*1000.) as f32/1000.;
    let (sin,cos)=f64::from(radians).sin_cos();
    d.motion_origin=[(sin+f64::from(c.current[0])) as f32,
        (cos+f64::from(c.current[1])) as f32,sample(random)*50.];
    d.position=d.motion_origin;
    d.motion=normalize_nonzero([d.position[0]-c.current[0],d.position[1]-c.current[1],0.]);
    let angle=f64::from(-d.motion[1]).acos() as f32;
    d.rotation=if -d.motion[0]>0. {-angle} else {angle};
    d.clock=0;d.phase=0;d.captured=d.position;
}
