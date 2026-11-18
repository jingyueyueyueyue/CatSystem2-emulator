//! Independently recovered falling-petal models. Shared with the verified
//! path core, texture snapshot and owner lifecycle; no plugin code is called.
use super::*;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct FallingConfig {
    size: f32,
    distance: f32,
    duration: f32,
    emission_divisor: u32,
    emission_step: i32,
    emission_count: i32,
    loops: bool,
}

impl PathFamily {
    pub(super) fn start_sakura(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        // 1ff0: p0 partitions the actual SDK grid; child storage comes from
        // the remaining portion of that same allocation (1120), not new quads.
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;
        self.falling=FallingConfig {size:word(p,1)/100.,distance:word(p,2),duration:word(p,3),..Default::default()};
        let warm=trunc(word(p,4))!=0;
        self.active_count=if warm {self.main_count/2} else {1};
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            spawn_sakura(d,trail,self.image,&self.falling,random);
            if warm && index<self.active_count {
                d.clock=trunc(sample(random)*self.falling.duration) as u32;
                if d.clock as i32 as f32>self.falling.duration*f32::from_bits(0x3f66_6666) {
                    d.phase=1;d.phase_origin=d.clock;
                }
            }
        }
    }

    pub(super) fn update_sakura(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        if random.next_integer()%10==0 {self.active_count=self.active_count.saturating_add(10);}
        self.active_count=self.active_count.min(self.main_count);
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            if d.phase<=1 {
                // 2160 only writes path X/Y: parent Z stays at its spawn0.
                let position=d.path.at(d.clock);d.position[..2].copy_from_slice(&position[..2]);
                if d.phase==1 {
                    d.alpha=trunc(super::super::lerp(255.,0.,
                        self.falling.duration-d.phase_origin as i32 as f32,
                        d.clock.wrapping_sub(d.phase_origin) as i32 as f32)) as u8;
                } else {d.alpha=255;}
                d.rotation_xy[0]+=sample(random)/20.;
                d.rotation_xy[1]+=sample(random)/20.;
                d.rotation+=sample(random)/20.;
                if d.phase==0 {
                    if d.clock as i32>trunc(self.falling.duration*f32::from_bits(0x3f66_6666)) {
                        d.phase=1;d.phase_origin=d.clock;
                    }
                } else if d.clock as i32 as f32>self.falling.duration {d.phase=2;d.phase_origin=d.clock;}
            } else {spawn_sakura(d,trail,self.image,&self.falling,random);}
            if d.phase==0 {
                if let Some((child_index,child))=trail.iter_mut().enumerate().find(|(_,c)|c.phase==0) {
                    let distance=length(sub(d.position,d.captured));
                    if distance!=0. {
                        // The PE computes/discards direction angle and distance.
                        // It copies XYZ orientation and uses midpoint geometry.
                        child.rotation=d.rotation;child.rotation_xy=d.rotation_xy;
                        child.position=scale(add(d.position,d.captured),0.5);
                        child.position[2]=child_index as f32+1.;
                    } else {child.position=d.position;}
                    child.pivot=d.pivot;child.scale=d.scale;child.stretch=d.stretch;child.phase=1;
                }
            }
            for child in trail.iter_mut().filter(|c|c.phase!=0) {
                child.scale*=f32::from_bits(0x3f4c_cccd);
                child.alpha=trunc(super::super::lerp(90.,0.,self.child_count as f32,child.clock as i32 as f32)) as u8;
                // Trails age by UPDATE CALL, unlike their parent's DWORD delta.
                child.clock=child.clock.wrapping_add(1);
                if child.clock>=self.child_count as u32 {child.clock=0;child.phase=0;}
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn start_paper(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;
        let warm=trunc(word(p,5))!=0;
        self.falling=FallingConfig {size:word(p,2)/100.,duration:trunc(word(p,1)) as f32,
            emission_divisor:61_i32.saturating_sub(trunc(word(p,3))).clamp(1,60) as u32,
            emission_step:trunc(word(p,4)),emission_count:if warm {(self.main_count/2) as i32} else {0},
            loops:trunc(word(p,6))!=0,..Default::default()};
        self.active_count=self.falling.emission_count as usize;
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            spawn_paper(d,trail,self.image,&self.falling,random);
            if warm && index<self.active_count {d.clock=trunc(sample(random)*self.falling.duration) as u32;}
        }
    }

    pub(super) fn update_paper(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        if random.next_integer()%self.falling.emission_divisor==0 {
            self.falling.emission_count=self.falling.emission_count.wrapping_add(self.falling.emission_step);
        }
        self.falling.emission_count=self.falling.emission_count.min(self.main_count as i32);
        self.active_count=self.falling.emission_count.max(0) as usize;
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            if d.phase<=1 {
                d.position=d.path.at(d.clock);
                d.alpha=if d.phase==0 {255} else {
                    trunc(super::super::lerp(255.,0.,self.falling.duration-d.phase_origin as i32 as f32,
                        d.clock.wrapping_sub(d.phase_origin) as i32 as f32)) as u8
                };
                d.rotation_xy[0]+=sample(random)/10.;
                d.rotation_xy[1]+=sample(random)/10.;
                d.rotation+=sample(random)/10.;
                if d.phase==0 && d.clock as i32>trunc(self.falling.duration*f32::from_bits(0x3f66_6666)) {
                    d.phase=1;d.phase_origin=d.clock;
                } else if d.phase==1 && d.clock as i32 as f32>self.falling.duration {d.phase=2;d.phase_origin=d.clock;}
            } else if self.falling.loops {spawn_paper(d,trail,self.image,&self.falling,random);}
            if d.phase==0 {
                if let Some(child)=trail.iter_mut().find(|c|c.phase==0) {
                    if length(sub(d.position,d.captured))!=0. {
                        child.rotation_xy=d.rotation_xy;child.rotation=d.rotation;
                        child.position=scale(add(d.position,d.captured),0.5);
                    } else {child.position=d.position;}
                    child.pivot=d.pivot;child.scale=d.scale;child.stretch=d.stretch;child.phase=1;
                }
            }
            // paper has no sakura-style .8 shrink and starts at alpha255.
            for child in trail.iter_mut().filter(|c|c.phase!=0) {
                child.alpha=trunc(super::super::lerp(255.,0.,self.child_count as f32,child.clock as i32 as f32)) as u8;
                child.clock=child.clock.wrapping_add(1);
                if child.clock>=self.child_count as u32 {child.clock=0;child.phase=0;}
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}

// The statically linked paper CRT rand keeps a default seed1 per thread.
// Its only effect caller is1860 at1aa8; subsequent segment lengths use the
// SDK integer callback instead. Never replace this draw with an extra MT draw.
std::thread_local! {static PAPER_CRT: std::cell::Cell<u32> = const {std::cell::Cell::new(1)};}
fn paper_crt_random() -> u32 {
    PAPER_CRT.with(|seed| {let next=seed.get().wrapping_mul(214013).wrapping_add(2531011);
        seed.set(next);(next>>16)&0x7fff})
}

fn spawn_paper(d: &mut Drop,trail: &mut [Drop],image: [f32;2],
    config: &FallingConfig,random: &mut impl ParticleRandomSource) {
    d.pivot=image.map(|n|n*0.5);
    d.scale=sample(random)*f32::from_bits(0x3d4c_cccd)+config.size;d.stretch=[1.;2];
    const COLORS: [[u32;3];7]=[[240,240,240],[110,255,255],[255,110,255],
        [255,255,110],[110,110,255],[110,255,110],[255,110,110]];
    let rgb=COLORS[(random.next_integer()%7) as usize];
    d.vertex_rgb=Some([rgb[0]|(rgb[1]<<8)|(rgb[2]<<16);4]);d.alpha=0;
    // 1280/-50/100 are this resource's fixed constants, not guessed extents.
    d.position=[sample(random)*1280.,-50.,sample(random)*100.];
    d.rotation_xy=[sample(random)*std::f32::consts::PI,sample(random)*std::f32::consts::PI];
    d.rotation=sample(random)*std::f32::consts::PI;
    d.clock=0;d.phase_origin=0;d.phase=0;d.captured=d.position;
    d.path=Trajectory::default();d.path.append(d.position);
    let mut direction=normalize_nonzero([sample(random)-0.5,sample(random),0.]);
    let distance=(paper_crt_random()%500) as f32/10.+50.;
    let mut point=add(d.position,scale(direction,distance));d.path.append(point);
    for _ in 0..12 {
        direction=falling_direction(direction,10.,random);
        let distance=(random.next_integer()%500) as f32/10.+50.;
        point=add(point,scale(direction,distance));d.path.append(point);
    }
    d.path.finish(config.duration,50.);
    for child in trail {
        child.clock=0;child.phase=0;child.phase_origin=0;child.path=Trajectory::default();
        child.position=d.position;child.vertex_rgb=d.vertex_rgb;child.alpha=0;
    }
}

fn spawn_sakura(d: &mut Drop,trail: &mut [Drop],image: [f32;2],
    config: &FallingConfig,random: &mut impl ParticleRandomSource) {
    d.pivot=image.map(|n|n*0.5);
    let atlas=random.next_integer()&1;
    d.uv=std::array::from_fn(|i|[atlas as f32*0.5+(i&1) as f32*0.5,(i>>1) as f32*0.5]);
    d.scale=(sample(random)*config.size)*f32::from_bits(0x3f19_999a)
        +config.size*f32::from_bits(0x3ecc_cccd);
    d.stretch=[1.,f32::from_bits(0x3fa6_6666)];d.alpha=0;
    d.position=[(random.next_integer()%1280) as f32+480.,-50.,0.];
    d.rotation_xy=[sample(random)*std::f32::consts::PI,sample(random)*std::f32::consts::PI];
    d.rotation=sample(random)*std::f32::consts::PI;
    d.clock=0;d.phase_origin=0;d.phase=0;d.captured=d.position;
    let mut point=d.position;
    d.path=Trajectory::default();d.path.append(point);
    let mut direction=normalize_nonzero([-sample(random),sample(random),0.]);
    let distance=falling_distance(config.distance,random);
    point=add(point,scale(direction,distance));d.path.append(point);
    // 2800 supplies twelve more knots at most; the SDK capacity is twenty.
    for _ in 0..12 {
        direction=falling_direction(direction,30.,random);
        let distance=falling_distance(config.distance,random);
        point=add(point,scale(direction,distance));d.path.append(point);
        if random.next_integer()%100==0 {break;}
    }
    d.path.finish(config.duration,config.duration/5.);
    for child in trail {
        child.clock=0;child.phase=0;child.phase_origin=0;child.path=Trajectory::default();
        child.uv=d.uv;child.position=d.position;child.alpha=0;
    }
}

fn falling_distance(distance: f32,random: &mut impl ParticleRandomSource) -> f32 {
    (sample(random)*distance)*f32::from_bits(0x3e4c_cccd)
        +distance*f32::from_bits(0x3f4c_cccd)
}

fn falling_direction(direction: [f32;3],degrees: f32,random: &mut impl ParticleRandomSource) -> [f32;3] {
    let angle=((f64::from(degrees)*f64::from(std::f32::consts::PI))/180.) as f32;
    let (s,c)=f64::from(angle).sin_cos();let (s,c)=(s as f32,c as f32);
    let left=[direction[0]*c+direction[1]*s,direction[1]*c-direction[0]*s];
    let right=[direction[0]*c-direction[1]*s,direction[1]*c+direction[0]*s];
    let span=[(right[0]-left[0]).abs(),(right[1]-left[1]).abs()];
    let x=direction[0]+sample(random)*span[0]-span[0]*0.5;
    let y=direction[1]+sample(random)*span[1]-span[1]*0.5;
    normalize_nonzero([x,y,0.])
}
