//! Independent high-use particle3 sprite families. Descriptors select recovered
//! state machines; shared geometry submission uses the actual texture extent.
use super::profiles::SpriteProfile;
use cs2_script::Result;
use cs2_runtime::graph_serde::{Deserialize, Serialize};
use super::super::super::particle_vm::{ParticleCanvas, ParticleQuad,
    ParticleRandomSource, ParticleRenderSnapshot, ParticleVertex};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct Sprite {
    clock: u32,
    origin: u32,
    phase: u32,
    position: [f32;3],
    saved_position: [f32;3],
    destination: [f32;3],
    size: f32,
    scale: f32,
    stretch: [f32;2],
    rotation: f32,
    rotation_x: f32,
    rotation_y: f32,
    motion: [f32;3],
    lifetime: f32,
    peak_alpha: i32,
    rgb: u32,
    vertex_alpha: Option<[u8;4]>,
    flag: u32,
    centered: bool,
    pivot: [f32;2],
    uv: [[f32;2];4],
    alpha: u8,
}
impl Default for Sprite {
    fn default() -> Self {
        Self {clock:0,origin:0,phase:0,position:[0.;3],saved_position:[0.;3],destination:[0.;3],
            size:1.,scale:1.,stretch:[1.;2],rotation:0.,rotation_x:0.,rotation_y:0.,motion:[0.;3],
            lifetime:0.,peak_alpha:0,rgb:0x00ff_ffff,vertex_alpha:None,flag:1,centered:false,
            pivot:[0.;2],uv:[[0.,0.],[1.,0.],[0.,1.],[1.,1.]],alpha:255}
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct FlowerConfig {
    radius: [f32;2],
    minimum: [f32;2],
    durations: [f32;5],
}
impl Default for FlowerConfig {
    fn default() -> Self {Self {radius:[0.;2],minimum:[0.;2],durations:[0.;5]}}
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
struct DigitalConfig {
    from: [f32;2],
    to: [f32;2],
    duration: i32,
    speed: [f32;2],
    size: [f32;2],
    spread: [f32;2],
    radius: f32,
    lifetime: i32,
    alpha: i32,
    clock: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(crate = "cs2_runtime::graph_serde")]
pub(super) struct SpriteFamily {
    profile: SpriteProfile,
    sprites: Vec<Sprite>,
    image: [f32;2],
    viewport_half: [f32;2],
    center: [f32;3],
    scale: f32,
    radius: f32,
    angle: f32,
    mirrored: bool,
    direction: i32,
    texture_factor: f32,
    durations: [i32;9],
    time: [f32;2],
    active_count: usize,
    emission_clock: u32,
    flower: FlowerConfig,
    digital: DigitalConfig,
    render_mode: u32,
    main_count: usize,
    child_count: usize,
}
impl SpriteFamily {
    /// Real particle3 producers share this complete private storage, not their
    /// update algorithm. Keep the saved configuration, pools and clocks rather
    /// than re-running any start/spawn callback to reconstruct them.
    pub(crate) fn capture_checkpoint(&self) -> anyhow::Result<Self> {
        self.validate_checkpoint(self.profile, self.sprites.len() as u32, self.image)?;
        Ok(self.clone())
    }

    pub(crate) fn validate_checkpoint(&self, profile: SpriteProfile, count: u32, image: [f32; 2]) -> anyhow::Result<()> {
        if self.profile != profile
            || self.sprites.len() != count as usize || self.image != image
            || self.active_count > self.sprites.len()
            || self.sprites.len().checked_mul(2048).is_none_or(|n| n > 64 * 1024 * 1024) {
            anyhow::bail!("sprite checkpoint differs from its exact profile/grid/extent");
        }
        if profile == SpriteProfile::Ring && (self.main_count > self.sprites.len()
            || self.active_count > self.main_count
            || self.main_count.checked_mul(self.child_count)
                .is_none_or(|n| n > self.sprites.len() - self.main_count)
            || self.main_count != 0 && self.durations[0] <= 0) {
            anyhow::bail!("ring checkpoint lost its emission clock or existing parent/trail partition");
        }
        Ok(())
    }

    pub(super) fn new(profile: SpriteProfile,count: u32,image: [f32;2]) -> Self {
        Self {profile,sprites:vec![Sprite::default();count as usize],image,viewport_half:[0.;2],
            center:[0.;3],scale:1.,radius:0.,angle:0.,mirrored:false,direction:0,texture_factor:0.5,
            durations:[0;9],time:[0.;2],active_count:0,emission_clock:0,flower:FlowerConfig::default(),
            digital:DigitalConfig::default(),render_mode:0,main_count:0,child_count:0}
    }
    pub(super) fn reset_extent(&mut self,image: [f32;2]) {self.image=image;}
    pub(super) fn start(&mut self,p: &[u8;128],viewport: [u32;2],random: &mut impl ParticleRandomSource) -> Result<()> {
        self.viewport_half=viewport.map(|n|n as f32*0.5);
        self.center=[super::word(p,0),super::word(p,1),0.];
        match self.profile {
            SpriteProfile::Notice | SpriteProfile::Common | SpriteProfile::Surprise => {
                self.direction=super::trunc(super::word(p,2));
                self.mirrored=self.direction!=0;
                let size=super::word(p,3)/100.;self.scale=if 0.001>size {0.001} else {size};
                self.durations=[0;9];
                let count=if self.profile==SpriteProfile::Common {2} else {6};
                for i in 0..count {self.durations[i]=super::trunc(super::word(p,i+4));}
                self.texture_factor=if self.image[1]>256. {1.} else {0.5};
                self.active_count=self.sprites.len();
                for d in &mut self.sprites {
                    d.clock=0;d.origin=0;d.phase=0;d.position=self.center;d.scale=self.scale;d.alpha=0;
                    //1700 changes geometry only; canonical pivot/rotation stay.
                    if self.profile==SpriteProfile::Common {
                        d.rotation=if self.mirrored {0.5} else {-0.5};
                        //19ad..1b70's actual vector expression. It reuses the
                        //new X when forming Y; replacing it with a standard
                        //rotation of (0,-1) changes this effect's excursion.
                        let angle=super::trunc(d.rotation*1000.) as f32/1000.;
                        let (s,c)=f64::from(angle).sin_cos();let (s,c)=(s as f32,c as f32);
                        let direction=[s,s*s-c,0.];
                        d.saved_position=std::array::from_fn(|i|
                            self.center[i]+((direction[i]*self.scale)*300.)*self.texture_factor);
                        d.destination=std::array::from_fn(|i|
                            self.center[i]+((direction[i]*self.scale)*100.)*self.texture_factor);
                    } else if self.profile==SpriteProfile::Surprise {d.scale=self.scale*3.;}
                }
            }
            SpriteProfile::Asease => {
                self.scale=super::word(p,2)/100.;self.radius=super::word(p,3);self.angle=super::word(p,4);
                self.time=[super::word(p,5),super::word(p,6)];self.active_count=0;
                for (index,d) in self.sprites.iter_mut().enumerate() {
                    d.clock=0;d.origin=0;d.phase=0;
                    d.pivot=self.image.map(|n|n*0.5);
                    spawn_asease(d,index,self.scale,self.radius,self.angle,self.center,self.time,random);
                }
            }
            SpriteProfile::Dis => {
                self.mirrored=super::trunc(super::word(p,2))!=0;
                let size=super::word(p,3)/100.;self.scale=if 0.001>size {0.001} else {size};
                self.durations[..4].copy_from_slice(&std::array::from_fn::<_,4,_>(|i|super::trunc(super::word(p,i+4))));
                self.active_count=self.sprites.len();
                for (index,d) in self.sprites.iter_mut().enumerate() {
                    d.clock=0;d.origin=0;d.phase=0;d.position=self.center;
                    d.pivot=self.image.map(|n|n*0.5);d.scale=self.scale;d.alpha=0;
                    d.uv=atlas_uv(if index==0 {0} else {1},self.mirrored,false);
                    let angle=if index==0 {0.} else if self.mirrored {0.7} else {-0.7};
                    let angle=super::trunc(angle*1000.) as f32/1000.;
                    let (s,c)=f64::from(angle).sin_cos();let (s,c)=(s as f32,c as f32);
                    d.saved_position=[self.center[0]+(s*self.scale)*50.,self.center[1]+(-c*self.scale)*50.,0.];
                }
            }
            SpriteProfile::Angry | SpriteProfile::Kyupin => {
                let scale_parameter=if self.profile==SpriteProfile::Angry {2} else {3};
                let size=super::word(p,scale_parameter)/100.;self.scale=if 0.001>size {0.001} else {size};
                self.durations=[0;9];
                let count=if self.profile==SpriteProfile::Angry {9} else {6};
                for i in 0..count {self.durations[i]=super::trunc(super::word(p,i+scale_parameter+1));}
                self.texture_factor=if self.image[1]>256. {1.} else {0.5};
                self.active_count=1.min(self.sprites.len());
                for d in &mut self.sprites {
                    d.clock=0;d.origin=0;d.phase=0;d.position=self.center;d.scale=self.scale*0.1;
                    d.pivot=self.image.map(|n|n*0.5);d.alpha=0;
                }
            }
            SpriteProfile::Double { .. } | SpriteProfile::Exclamation | SpriteProfile::Down | SpriteProfile::Silent => {
                self.start_reaction(p,random);
            }
            SpriteProfile::FlowerBurst => {
                self.scale=super::word(p,2)/100.;
                self.flower=FlowerConfig {radius:[super::word(p,3),super::word(p,4)],
                    minimum:[super::word(p,5)/100.,super::word(p,6)/100.],
                    durations:std::array::from_fn(|i|super::word(p,i+7))};
                self.active_count=0;
                for d in &mut self.sprites {
                    d.pivot=self.image.map(|n|n*0.5);
                    spawn_flower(d,self.scale,self.center,&self.flower,random);
                }
            }
            SpriteProfile::Digital { orbit } => {
                self.digital=DigitalConfig {from:[super::word(p,0),super::word(p,1)],
                    to:[super::word(p,2),super::word(p,3)],duration:super::trunc(super::word(p,4)),
                    speed:[super::word(p,5)/100.,super::word(p,6)/100.],
                    size:[super::word(p,7)/1000.,super::word(p,8)/1000.],
                    spread:[super::word(p,9),super::word(p,10)],
                    radius:if orbit {super::word(p,11)} else {0.},
                    lifetime:super::trunc(super::word(p,if orbit {12} else {11})),
                    alpha:super::trunc(super::word(p,if orbit {13} else {12})),clock:0.};
                if if orbit {super::trunc(super::word(p,14))!=0} else {super::word(p,13)!=0.} {self.render_mode=1;}
                self.active_count=self.sprites.len();
                for d in &mut self.sprites {
                    d.pivot=self.image.map(|n|n*0.5);
                    spawn_digital(d,&self.digital,self.digital.from,orbit,random);
                }
            }
            SpriteProfile::FilmNoise => {
                self.active_count=3.min(self.sprites.len());
                for (index,d) in self.sprites.iter_mut().enumerate() {
                    d.pivot=self.image.map(|n|n*0.5);spawn_film_noise(d,index,random);
                }
            }
            SpriteProfile::Ring => {
                let partition=super::trunc(super::word(p,0)) as u32;
                self.main_count=if partition==0 {self.sprites.len()} else {self.sprites.len()/partition as usize};
                self.child_count=partition.saturating_sub(1) as usize;
                self.center=[super::word(p,1),super::word(p,2),0.];
                self.time=[super::word(p,3)/100.,super::word(p,4)/100.];
                for n in &mut self.time {if 0.001>*n {*n=0.001;}}
                let period=super::trunc(super::word(p,5));
                if period==0 {return Err(super::super::super::native_state_error("ring emission divisor is zero"));}
                self.durations[0]=if period<0 {1} else {period};
                self.durations[1]=super::trunc(super::word(p,6));self.durations[2]=super::trunc(super::word(p,7));
                self.active_count=0;self.emission_clock=0;self.render_mode=1;
                let (parents,children)=self.sprites.split_at_mut(self.main_count);
                for (index,d) in parents.iter_mut().enumerate() {
                    d.clock=0;d.origin=0;d.phase=0;d.position=self.center;d.scale=self.time[0];d.alpha=0;
                    d.pivot=self.image.map(|n|n*0.5);
                    for (i,child) in children[index*self.child_count..(index+1)*self.child_count].iter_mut().enumerate() {
                        child.clock=0;child.phase=0;child.pivot=d.pivot;
                        child.position=[self.center[0],self.center[1],(i+1) as f32];child.alpha=0;
                    }
                }
            }
        }
        Ok(())
    }
    pub(super) fn update(&mut self,delta: f32,source: [f32;3],random: &mut impl ParticleRandomSource) {
        let offset=[source[0]-self.viewport_half[0],source[1]-self.viewport_half[1]];
        let tick=super::trunc(delta) as u32;
        if self.profile==SpriteProfile::FilmNoise {
            self.update_film_noise(tick,random);return;
        }
        if self.profile==SpriteProfile::Ring {self.update_ring(tick,offset);return;}
        if let SpriteProfile::Digital {orbit}=self.profile {
            self.update_digital(delta,tick,offset,orbit,random);return;
        }
        if matches!(self.profile,SpriteProfile::Double { .. }|SpriteProfile::Exclamation|SpriteProfile::Down|SpriteProfile::Silent) {
            self.update_reaction(tick,offset);return;
        }
        if matches!(self.profile,SpriteProfile::Asease|SpriteProfile::FlowerBurst) {
            if random.next_integer()&1==0 {self.active_count=(self.active_count+1).min(self.sprites.len());}
        }
        for (index,d) in self.sprites.iter_mut().enumerate().take(self.active_count) {
            let elapsed=d.clock.wrapping_sub(d.origin) as i32;
            match self.profile {
                SpriteProfile::Double { .. } | SpriteProfile::Exclamation | SpriteProfile::Down | SpriteProfile::Silent => unreachable!("reaction family handled before ordinary sprites"),
                SpriteProfile::Digital { .. } => unreachable!("digital family handled before ordinary sprites"),
                SpriteProfile::FilmNoise => unreachable!("film noise handled before ordinary sprites"),
                SpriteProfile::Ring => unreachable!("ring handled before ordinary sprites"),
                SpriteProfile::FlowerBurst => {
                    d.position=d.saved_position;
                    let now=if d.phase==0 {d.clock as i32 as f32} else {elapsed as f32};
                    if d.phase<=4 {
                        let time=self.flower.durations[d.phase as usize];
                        match d.phase {
                            0=>{d.scale=super::lerp(0.,d.size*1.3,time,now);
                                d.alpha=super::trunc(super::lerp(0.,255.,time,now)) as u8;},
                            1=>{d.rotation+=0.1;d.scale=super::lerp(d.size*1.3,d.size,time,now);},
                            2=>{},
                            3=>{d.scale=super::lerp(d.size*0.8,d.size*1.2,time,now);
                                d.alpha=super::trunc(super::lerp(0.,255.,time,now)) as u8;},
                            4=>{d.rotation+=0.05;d.scale=super::lerp(d.size*1.2,d.size*0.1,time,now);
                                d.alpha=super::trunc(super::lerp(170.,0.,time,now)) as u8;},
                            _=>unreachable!(),
                        }
                        if now>time {
                            if d.phase==2 {d.uv=rect_uv([0.5,0.5],[0.5;2],false);d.alpha=0;}
                            d.phase+=1;d.origin=d.clock;
                        }
                    } else {spawn_flower(d,self.scale,self.center,&self.flower,random);}
                }
                SpriteProfile::Notice => {
                    d.position[..2].copy_from_slice(&self.center[..2]);
                    if d.phase<=4 {
                        d.alpha=if d.phase&1==0 {255} else {0};
                        if elapsed>self.durations[d.phase as usize] {d.phase+=1;d.origin=d.clock;}
                    }
                    //phase5 holds the last visible alpha. p9 is stored by PE
                    //start but no public update branch reads that sixth time.
                }
                SpriteProfile::Asease => {
                    if d.phase<=1 {
                        d.position=d.saved_position;
                        let time=self.time[d.phase as usize];
                        if d.phase==0 {
                            d.scale=super::lerp(0.,d.size,time,d.clock as i32 as f32);
                            d.alpha=super::trunc(super::lerp(0.,255.,time,d.clock as i32 as f32)) as u8;
                        } else {
                            d.scale=super::lerp(d.size*f32::from_bits(0x3fa6_6666),d.size,time,elapsed as f32);
                        }
                        let now=if d.phase==0 {d.clock as i32 as f32} else {elapsed as f32};
                        if now>time {d.phase=if d.phase==0 {1} else {10};d.origin=d.clock;}
                    } else {
                        spawn_asease(d,index,self.scale,self.radius,self.angle,self.center,self.time,random);
                    }
                }
                SpriteProfile::Common => {
                    if d.phase<=1 {
                        let duration=self.durations[d.phase as usize] as f32;
                        let (from,to)=if d.phase==0 {(self.center,d.saved_position)} else {(d.saved_position,d.destination)};
                        for i in 0..2 {d.position[i]=super::lerp(from[i],to[i],duration,elapsed as f32);}
                        let large=self.scale*f32::from_bits(0x3fa6_6666);
                        let (from,to)=if d.phase==0 {(self.scale,large)} else {(large,self.scale)};
                        d.scale=super::lerp(from,to,duration,elapsed as f32);
                        let (from,to)=if d.phase==0 {(0.,128.)} else {(128.,255.)};
                        d.alpha=super::trunc(super::lerp(from,to,duration,elapsed as f32)) as u8;
                        if elapsed>self.durations[d.phase as usize] {d.phase=if d.phase==0 {1} else {10};d.origin=d.clock;}
                    } else {d.position=d.destination;}
                }
                SpriteProfile::Surprise => {
                    d.position=self.center;
                    if d.phase==0 {
                        let duration=self.durations[0] as f32;
                        d.rotation=super::lerp(0.,f32::from_bits(0x4149_0fdb),duration,elapsed as f32);
                        if self.direction==1 {d.rotation*=-1.;}
                        d.scale=super::lerp(self.scale*3.,self.scale,duration,elapsed as f32);
                        d.alpha=super::trunc(super::lerp(0.,255.,duration,elapsed as f32)) as u8;
                        if elapsed>self.durations[0] {d.phase=1;d.origin=d.clock;d.rotation=0.;}
                    } else if d.phase==1 {
                        let distance=super::lerp((self.scale*35.)*self.texture_factor,0.,self.durations[1] as f32,elapsed as f32);
                        let signs=match elapsed%4 {0=>Some([-1.,-1.]),1=>Some([1.,1.]),2=>Some([1.,-1.]),3=>Some([-1.,1.]),_=>None};
                        if let Some(signs)=signs {for i in 0..2 {d.position[i]+=signs[i]*distance;}}
                        if elapsed>self.durations[1] {d.phase=10;d.origin=d.clock;}
                    }
                }
                SpriteProfile::Dis => {
                    if d.phase<=1 {
                        let duration=self.durations[d.phase as usize] as f32;
                        let (from,to)=if d.phase==0 {(self.center,d.saved_position)} else {(d.saved_position,self.center)};
                        for i in 0..2 {d.position[i]=super::lerp(from[i],to[i],duration,elapsed as f32);}
                        let large=self.scale*f32::from_bits(0x3f99_999a);
                        let (from,to)=if d.phase==0 {(self.scale,large)} else {(large,self.scale)};
                        d.scale=super::lerp(from,to,duration,elapsed as f32);
                        let (from,to)=if d.phase==0 {(0.,190.)} else {(190.,255.)};
                        d.alpha=super::trunc(super::lerp(from,to,duration,elapsed as f32)) as u8;
                        if elapsed>self.durations[d.phase as usize] {d.phase+=1;d.origin=d.clock;}
                    } else if d.phase==2 {
                        d.destination=self.center;d.position=self.center;
                        if index==1 {
                            d.destination[0]=(sample(random)*6.-3.)+self.center[0];
                            d.destination[1]=(sample(random)*6.-3.)+self.center[1];d.position=d.destination;
                            let _unused=sample(random);
                            d.rotation=(f64::from(sample(random))*f64::from(f32::from_bits(0x40c9_0fdb))) as f32;
                            let bits=random.next_integer()&3;
                            d.uv=atlas_uv(1,bits&1!=0,bits&2!=0);
                        }
                        d.phase=3;d.origin=d.clock;
                    } else if d.phase==3 {
                        d.position=d.destination;
                        if elapsed>self.durations[3] {d.phase=2;d.origin=d.clock;}
                    } else {d.position=self.center;}
                }
                SpriteProfile::Angry => {
                    d.position=self.center;
                    let now=elapsed as f32;
                    let duration=self.durations[d.phase.min(4) as usize] as f32;
                    match d.phase {
                        0=>{
                            d.scale=super::lerp(self.scale*0.1,self.scale*f32::from_bits(0x3fb3_3333),duration,now);
                            d.rotation=super::lerp(0.,1.,self.durations[1] as f32,now);
                            d.alpha=super::trunc(super::lerp(0.,255.,duration,now)) as u8;
                        }
                        1=>{
                            d.scale=super::lerp(self.scale*f32::from_bits(0x3fb3_3333),self.scale,duration,now);
                            d.rotation=super::lerp(1.,f32::from_bits(0x3e99_999a),duration,now);
                        }
                        2..=4=>{
                            d.position[0]+=((triangle(now,4.)*self.scale)*4.)*self.texture_factor;
                            if d.phase==3 {d.scale=super::lerp(self.scale,self.scale*f32::from_bits(0x3f8c_cccd),duration,now);}
                            if d.phase==4 {d.scale=super::lerp(self.scale*f32::from_bits(0x3f99_999a),self.scale,duration,now);}
                        }
                        _=>{}
                    }
                    if d.phase<=4 && elapsed>self.durations[d.phase as usize] {
                        d.phase=if d.phase==4 {2} else {d.phase+1};d.origin=d.clock;
                    }
                }
                SpriteProfile::Kyupin => {
                    d.position=self.center;
                    let now=elapsed as f32;
                    let large=self.scale*f32::from_bits(0x3f8c_cccd);
                    let small_pulse=self.scale*f32::from_bits(0x3f86_6666);
                    match d.phase {
                        0=>{
                            d.scale=super::lerp(self.scale*0.1,large,self.durations[0] as f32,now);
                            d.alpha=super::trunc(super::lerp(0.,255.,self.durations[0] as f32,now)) as u8;
                        }
                        1=>d.scale=super::lerp(large,self.scale,self.durations[1] as f32,now),
                        3=>d.scale=super::lerp(self.scale,small_pulse,self.durations[1] as f32,now),
                        4=>d.scale=super::lerp(small_pulse,self.scale,self.durations[1] as f32,now),
                        _=>{}
                    }
                    if d.phase<=4 && elapsed>self.durations[d.phase as usize] {
                        if d.phase==0 {d.rotation=0.;}
                        d.phase=if d.phase==4 {2} else {d.phase+1};d.origin=d.clock;
                    }
                }
            }
            // Trail copies must be made before translating the parent's
            //source position and before advancing its private clock.
            if matches!(self.profile,SpriteProfile::Angry|SpriteProfile::Kyupin) {continue;}
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
        if self.profile==SpriteProfile::Angry && self.active_count!=0 {
            let (parents,children)=self.sprites.split_at_mut(1);let main=&mut parents[0];
            if let Some(d)=children.iter_mut().find(|d|d.phase==0) {
                d.saved_position=main.position;d.position=main.position;d.position[2]-=1.;
                d.rotation=main.rotation;d.scale=main.scale;d.phase=1;
            }
            for d in children.iter_mut().filter(|d|d.phase!=0) {
                d.position[..2].copy_from_slice(&d.saved_position[..2]);
                d.alpha=super::trunc(super::lerp(30.,0.,29.,d.clock as i32 as f32)) as u8;
                d.scale*=f32::from_bits(0x3f81_47ae);d.clock=d.clock.wrapping_add(tick);
                if d.clock as i32>29 {d.alpha=0;d.clock=0;d.phase=0;}
                d.position[0]+=offset[0];d.position[1]+=offset[1];
            }
            main.position[0]+=offset[0];main.position[1]+=offset[1];main.clock=main.clock.wrapping_add(tick);
        }
        if self.profile==SpriteProfile::Kyupin && self.active_count!=0 {
            let (parents,children)=self.sprites.split_at_mut(1);let main=&mut parents[0];
            // The integer gate is called for each unused slot until a slot
            //passes; at most one new scatter is initialized per update.
            for d in children.iter_mut() {
                if d.phase!=0 || random.next_integer()%300!=0 {continue;}
                let mut direction=[sample(random),sample(random),0.];
                let length=f64::from(direction[0]*direction[0]+direction[1]*direction[1]).sqrt() as f32;
                if length>0. {for n in &mut direction {*n/=length;}}
                for n in &mut direction[..2] {*n*=if random.next_integer()&1!=0 {1.} else {-1.};}
                let distance=(f64::from(sample(random))*30.+30.) as f32;
                d.saved_position=std::array::from_fn(|i|self.center[i]+(direction[i]*distance)*main.scale);
                let distance=(f64::from(sample(random))*30.+30.) as f32;
                d.destination=std::array::from_fn(|i|d.saved_position[i]+(direction[i]*distance)*main.scale);
                d.position=main.position;d.scale=self.scale*0.1;d.phase=1;break;
            }
            let child_count=children.len() as i32;
            let duration=child_count as f32;
            for d in children.iter_mut().filter(|d|d.phase!=0) {
                let point=scatter_position(d.saved_position,d.destination,d.clock as i32 as f32,duration);
                d.position[..2].copy_from_slice(&point[..2]);
                d.alpha=super::trunc(super::lerp(255.,0.,duration,d.clock as i32 as f32)) as u8;
                d.clock=d.clock.wrapping_add(tick);
                if d.clock as i32>=child_count {d.clock=0;d.phase=0;d.alpha=0;}
                d.position[0]+=offset[0];d.position[1]+=offset[1];
            }
            main.position[0]+=offset[0];main.position[1]+=offset[1];main.clock=main.clock.wrapping_add(tick);
        }
    }

    fn update_ring(&mut self,tick: u32,offset: [f32;2]) {
        if self.emission_clock%self.durations[0] as u32==0 {self.active_count=(self.active_count+1).min(self.main_count);}
        self.emission_clock=self.emission_clock.wrapping_add(1);
        let (parents,children)=self.sprites.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            d.position[..2].copy_from_slice(&self.center[..2]);
            let elapsed=d.clock.wrapping_sub(d.origin) as i32;
            if d.phase<=1 {
                d.scale=super::lerp(self.time[0],self.time[1],self.durations[1].wrapping_add(self.durations[2]) as f32,d.clock as i32 as f32);
                let (from,to)=if d.phase==0 {(0.,200.)} else {(200.,0.)};
                d.alpha=super::trunc(super::lerp(from,to,self.durations[d.phase as usize+1] as f32,elapsed as f32)) as u8;
                if elapsed>self.durations[d.phase as usize+1] {d.phase=if d.phase==0 {1} else {9};d.origin=d.clock;}
            }
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            if d.phase<=3 {
                if let Some(child)=trail.iter_mut().find(|child|child.phase==0) {
                    child.pivot=d.pivot;child.position[..2].copy_from_slice(&d.position[..2]);
                    child.scale=d.scale;child.alpha=0;child.clock=0;child.origin=0;child.phase=1;
                }
            }
            for child in trail.iter_mut().filter(|child|child.phase!=0) {
                child.position[..2].copy_from_slice(&d.position[..2]);
                child.alpha=super::trunc(super::lerp(10.,0.,40.,child.clock as i32 as f32)) as u8;
                child.clock=child.clock.wrapping_add(1);
                //1984..19c3 only hides the quad at40. It does not free the
                //slot or reset its phase; this trail pool is one-shot.
                if child.clock>=40 {child.alpha=0;}
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
    }

    fn update_film_noise(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        if random.next_integer()%30==0 {self.active_count=(self.active_count+1).min(self.sprites.len());}
        for (index,d) in self.sprites.iter_mut().enumerate().take(self.active_count) {
            let elapsed=d.clock.wrapping_sub(d.origin) as i32;
            if index<2 {
                d.flag=u32::from(if d.phase==0 {index==0} else {index!=0});
                if elapsed>1 {d.phase^=1;d.origin=d.clock;}
            } else if d.phase==0 {
                if index!=2 {d.flag=1;}
                if elapsed as f32>d.lifetime {d.phase=1;d.origin=d.clock;}
            } else {spawn_film_noise(d,index,random);}
            //1520 and its three updates never apply the source offset. The
            //1280x720 layout constants below are actual plugin parameters.
            d.clock=d.clock.wrapping_add(tick);
        }
    }

    fn update_digital(&mut self,delta: f32,tick: u32,offset: [f32;2],orbit: bool,
        random: &mut impl ParticleRandomSource) {
        let c=&self.digital;
        let center=std::array::from_fn(|i|super::lerp(c.from[i],c.to[i],c.duration as f32,c.clock));
        for d in &mut self.sprites {
            if d.phase!=0 {
                //Both PE updates allow reseeding only while the moving center
                //has time remaining. After that, the expired record is held.
                if c.duration as f32>c.clock {spawn_digital(d,c,center,orbit,random);}
            } else {
                let elapsed=d.clock.wrapping_sub(d.origin) as i32;
                if orbit {
                    d.motion[2]+=d.size;
                    let (s,cos)=f64::from(d.motion[2]).sin_cos();
                    d.position=[center[0]-(cos as f32)*d.motion[1],center[1]+d.motion[0],(s as f32)*d.motion[1]];
                    let x=center[0]-d.position[0];let z=-d.position[2];
                    let length=f64::from(x*x+z*z).sqrt() as f32;
                    let (x,z)=if length>0. {(x/length,z/length)} else {(x,z)};
                    let angle=f64::from(-z).acos() as f32;
                    d.rotation_x=if x>0. {-angle} else {angle};
                } else {
                    d.saved_position[0]+=d.motion[0];d.saved_position[1]+=d.motion[1];
                    d.position[..2].copy_from_slice(&d.saved_position[..2]);
                }
                d.alpha=super::trunc(super::lerp(d.peak_alpha as f32,0.,d.lifetime,elapsed as f32)) as u8;
                if elapsed as f32>d.lifetime {d.phase=3;d.origin=d.clock;}
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
        self.digital.clock+=delta;
    }

    fn start_reaction(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let down=self.profile==SpriteProfile::Down;let silent=self.profile==SpriteProfile::Silent;
        let scale_index=if down {2} else {3};self.scale=super::word(p,scale_index)/100.;
        if 0.001>self.scale {self.scale=0.001;}
        self.direction=if down {0} else {super::trunc(super::word(p,2))};
        self.mirrored=self.direction!=0;
        self.durations=[0;9];let count=if down||silent {6} else {5};
        for i in 0..count {self.durations[i]=super::trunc(super::word(p,i+if down {5} else {4}));}
        self.time=if down {[super::word(p,3),super::word(p,4)]} else {[0.;2]};
        self.texture_factor=if silent {if self.image[1]>128. {0.5} else {0.25}}
            else if self.image[1]>256. {1.} else {0.5};
        self.active_count=if down||silent {1.min(self.sprites.len())} else {self.sprites.len()};
        self.emission_clock=0;
        for (index,d) in self.sprites.iter_mut().enumerate() {
            d.clock=0;d.origin=0;d.phase=0;d.position=self.center;d.scale=self.scale;d.stretch=[1.;2];
            d.alpha=0;d.centered=!(down||silent);
            if down {
                if index==0 {
                    d.pivot=self.image.map(|n|n*0.5);d.uv=rect_uv([0.25,0.],[0.75,0.5],false);
                    d.saved_position=[self.center[0]+self.time[0],self.center[1]+self.time[1],0.];
                } else {
                    d.pivot=[self.image[0]*0.5,0.];d.uv=rect_uv([0.,0.],[0.25,1.],false);
                    d.saved_position=[self.center[0]+((index as i32).wrapping_mul(50) as f32)*self.scale,
                        (f64::from(sample(random))*f64::from(self.scale*20.)+f64::from(self.center[1])) as f32,index as f32];
                    d.size=(sample(random)*0.3+0.8)*self.scale;d.stretch=[0.025,0.1];
                }
                d.position=d.saved_position;
            } else if silent {
                d.pivot=self.image.map(|n|n*0.5);
                if index==0 {d.uv=rect_uv([0.,0.],[1.,0.75],self.mirrored);d.saved_position=self.center;}
                else {
                    //1d70 indexes the four-entry atlas directly; new rejects
                    //grids beyond that recovered table before allocating.
                    d.uv=rect_uv([index as f32*0.25,0.75],[0.25,0.25],false);
                    d.saved_position=[(self.center[0]-(self.scale*600.)*self.texture_factor)
                        +(((index as i32).wrapping_mul(300) as f32)*self.scale)*self.texture_factor,
                        self.center[1]+(self.scale*(-10.))*self.texture_factor,index as f32];
                }
                d.position=d.saved_position;
            } else {
                d.rotation=if self.mirrored {0.4} else {-0.4};
                let angle=super::trunc(d.rotation*1000.) as f32/1000.;
                let (s,c)=f64::from(angle).sin_cos();let (s,c)=(s as f32,c as f32);
                let vector=[s,s*s-c,0.];
                let distance=if self.profile==SpriteProfile::Exclamation {100.} else {20.};
                d.saved_position=std::array::from_fn(|i|self.center[i]+vector[i]*distance);
                d.destination=std::array::from_fn(|i|self.center[i]+vector[i]*40.);
                if matches!(self.profile,SpriteProfile::Double { .. }) {d.stretch=[3.;2];}
            }
        }
    }

    fn update_reaction(&mut self,tick: u32,offset: [f32;2]) {
        let down=self.profile==SpriteProfile::Down;let silent=self.profile==SpriteProfile::Silent;
        if down||silent {
            if self.emission_clock>if down {5} else {30} {
                self.active_count=(self.active_count+1).min(self.sprites.len());self.emission_clock=0;
            }
        }
        for (index,d) in self.sprites.iter_mut().take(self.active_count).enumerate() {
            let elapsed=d.clock.wrapping_sub(d.origin) as i32;let t=elapsed as f32;
            let duration=|i: usize|self.durations[i] as f32;
            match self.profile {
                SpriteProfile::Double {shake_by_index,loops} => {
                    if d.phase==0 {
                        d.position=self.center;let size=super::lerp(3.,1.,duration(0),t);d.stretch=[size;2];
                        d.alpha=super::trunc(super::lerp(0.,255.,duration(0),t)) as u8;
                        if elapsed>self.durations[0] {d.phase=1;d.origin=d.clock;}
                    } else if d.phase==1 {
                        let distance=super::lerp((self.scale*35.)*self.texture_factor,0.,duration(1),t);
                        let phase=if shake_by_index {index as i32%4} else {elapsed%4};
                        if phase>=0 {
                            let signs=[[-1.,-1.],[1.,1.],[1.,-1.],[-1.,1.]][phase as usize];
                            d.position=[self.center[0]+signs[0]*distance,self.center[1]+signs[1]*distance,0.];
                        }
                        if loops&&elapsed>self.durations[1] {d.phase=2;d.origin=d.clock;}
                    } else if d.phase==2 {
                        d.position=self.center;
                        if elapsed>self.durations[2] {d.phase=1;d.origin=d.clock;}
                    }
                }
                SpriteProfile::Exclamation => {
                    let from_to=match d.phase {0=>Some((self.center,d.saved_position,0)),
                        1=>Some((d.saved_position,self.center,1)),2=>Some((self.center,d.destination,0)),_=>None};
                    if let Some((from,to,clock))=from_to {
                        d.position=std::array::from_fn(|i|super::lerp(from[i],to[i],duration(clock),t));
                    } else {d.position=d.destination;}
                    match d.phase {
                        0=>{d.stretch=[super::lerp(1.,0.6,duration(0),t),super::lerp(1.,1.5,duration(0),t)];
                            d.alpha=super::trunc(super::lerp(0.,128.,duration(0),t)) as u8;},
                        1=>{d.stretch=[super::lerp(0.6,1.8,duration(1),t),super::lerp(1.5,0.5,duration(1),t)];
                            d.alpha=super::trunc(super::lerp(128.,255.,duration(1),t)) as u8;},
                        2=>{d.stretch=[super::lerp(1.8,1.,duration(2),t),super::lerp(0.5,1.,duration(2),t)];},
                        4=>{d.rotation_y+=(f64::from((std::f32::consts::PI/duration(4))*t).sin()*0.5) as f32;},
                        _=>{},
                    }
                    let boundary=match d.phase {0..=2=>d.phase as usize,3=>4,_=>3};
                    if elapsed>self.durations[boundary] {
                        d.phase=if d.phase==4 {d.rotation_y=0.;3} else {d.phase+1};d.origin=d.clock;
                    }
                }
                SpriteProfile::Down | SpriteProfile::Silent => {
                    d.position=d.saved_position;
                    if index==0&&d.phase<=1 {
                        let now=d.clock as i32 as f32;
                        if down {
                            let duration=self.durations[0].wrapping_add(self.durations[1]) as f32;
                            d.stretch=[super::lerp(0.01,2.5,duration,now),super::lerp(0.01,1.8,duration,now)];
                            d.alpha=super::trunc(super::lerp(0.,128.,duration,now)) as u8;
                        } else {
                            let (from,to)=if d.phase==0 {([0.01,0.01],[2.7,2.2])} else {([2.7,2.2],[2.5,2.])};
                            d.stretch=std::array::from_fn(|i|super::lerp(from[i],to[i],duration(d.phase as usize),now));
                            d.alpha=super::trunc(super::lerp(0.,255.,duration(d.phase as usize),now)) as u8;
                        }
                        //Both main helpers compare ABSOLUTE clock separately
                        //against p duration0 and1, despite recording an origin.
                        if d.clock as i32>self.durations[d.phase as usize] {d.phase+=1;d.origin=d.clock;}
                    } else if index!=0&&d.phase<=1 {
                        let clock=d.phase as usize;
                        if down {
                            if d.phase==0 {
                                d.stretch=[super::lerp(0.025,0.25,duration(0),t),
                                    super::lerp(self.scale*0.1,d.size*1.2,duration(0),t)/self.scale];
                            } else {d.stretch[1]=super::lerp(self.scale*1.2,d.size,duration(1),t)/self.scale;}
                        } else {
                            let (from,to)=if d.phase==0 {(0.1,1.2)} else {(1.2,0.7)};
                            d.stretch=[super::lerp(from,to,duration(clock),t);2];
                        }
                        if d.phase==0 {d.alpha=super::trunc(super::lerp(0.,255.,duration(0),t)) as u8;}
                        if elapsed>self.durations[clock] {d.phase+=1;d.origin=d.clock;}
                    }
                }
                _=>unreachable!("ordinary sprite handled by its own update"),
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
        if down||silent {self.emission_clock=self.emission_clock.wrapping_add(1);}
    }
    pub(super) fn snapshot(&self,canvas: ParticleCanvas,active: bool,submitted: bool,
        elapsed: u32,previous_elapsed: u32,opacity: u32) -> ParticleRenderSnapshot {
        let [w,h]=self.image;
        ParticleRenderSnapshot {active,frame_complete:true,elapsed,previous_elapsed,
            current_quad:self.sprites.len() as u32,render_mode:self.render_mode,opacity,canvas,
            quads:self.sprites.iter().enumerate().map(|(index,d)| {
                let mut xy=if submitted && (d.centered || !matches!(self.profile,SpriteProfile::Asease|SpriteProfile::Dis|SpriteProfile::Angry|SpriteProfile::Kyupin|SpriteProfile::Down|SpriteProfile::Silent|SpriteProfile::FlowerBurst|SpriteProfile::Digital { .. }|SpriteProfile::FilmNoise|SpriteProfile::Ring)) {
                    [[-w*0.5,-h*0.5],[w*0.5,-h*0.5],[-w*0.5,w*0.5],[w*0.5,h*0.5]]
                } else {[[0.,0.],[w,0.],[0.,h],[w,h]]};
                if submitted && matches!(self.profile,SpriteProfile::Notice|SpriteProfile::Surprise) && self.mirrored {
                    for p in &mut xy {p[0]=-p[0];}
                }
                let uv=if submitted {d.uv} else {[[0.,0.],[1.,0.],[0.,1.],[1.,1.]]};
                let (s,c)=f64::from(d.rotation).sin_cos();let (s,c)=(s as f32,c as f32);
                let (sy,cy)=f64::from(d.rotation_y).sin_cos();let (sy,cy)=(sy as f32,cy as f32);
                let (sx,cx)=f64::from(d.rotation_x).sin_cos();let (sx,cx)=(sx as f32,cx as f32);
                let vertices=std::array::from_fn(|i| {
                    let [mut x,mut y]=xy[i];let mut z=0.;
                    if submitted {
                        x=(x-d.pivot[0])*(d.scale*d.stretch[0]);y=(y-d.pivot[1])*(d.scale*d.stretch[1]);
                        z=y*sx;y*=cx;
                        let a=x;x=a*cy+z*sy;z=-a*sy+z*cy;
                        let (a,b)=(x,y);x=a*c-b*s+d.position[0];y=a*s+b*c+d.position[1];
                    }
                    ParticleVertex {position:[x,y,if submitted {z+d.position[2]} else {0.}],uv:uv[i],
                        color:if submitted {d.rgb|(u32::from(d.vertex_alpha.map_or(d.alpha,|a|a[i]))<<24)} else {u32::MAX},..Default::default()}
                });
                ParticleQuad {index:index as u32,vertices,offset:[0.;4],depth_bias:0.,flag:if submitted {d.flag} else {1}}
            }).collect()}
    }
}

fn spawn_film_noise(d: &mut Sprite,index: usize,random: &mut impl ParticleRandomSource) {
    d.clock=0;d.origin=0;d.phase=0;
    if index<2 {
        d.uv=rect_uv([0.,index as f32*0.5],[0.625,0.3125],false);
        d.position=[640.,360.,20.];d.scale=1.;d.stretch=[1.25,0.8];d.alpha=5;
        d.flag=0;d.phase=if index==0 {1} else {0};
    } else if index==2 {
        d.uv=rect_uv([0.75,0.5],[0.25,0.5],false);d.scale=1.;d.stretch=[1.;2];
        d.position=[sample(random)*1280.,200.,10.];
        d.stretch[0]=(f64::from(sample(random)*0.1)+0.1) as f32;
        d.vertex_alpha=Some(std::array::from_fn(|_|random.next_integer() as u8));
        d.lifetime=(random.next_integer()%5+1) as f32;d.flag=1;
    } else {
        d.uv=rect_uv([0.75,0.],[0.25;2],false);
        d.position=[sample(random)*1280.,sample(random)*720.,index as f32-2.];
        d.rotation=sample(random)*std::f32::consts::PI;
        d.scale=(f64::from(sample(random))*f64::from(0.01_f32)+f64::from(0.005_f32)) as f32;
        d.vertex_alpha=Some(std::array::from_fn(|_|(random.next_integer()%200) as u8));
        d.lifetime=(random.next_integer()&7) as f32;d.flag=0;
    }
}

fn spawn_digital(d: &mut Sprite,c: &DigitalConfig,center: [f32;2],orbit: bool,
    random: &mut impl ParticleRandomSource) {
    let atlas=match random.next_integer()%10 {0=>0,1=>1,2=>3,_=>2};
    d.uv=rect_uv([(atlas&1) as f32*0.5,(atlas>>1) as f32*0.5],[0.5;2],false);
    d.scale=(sample(random)*2.-1.)*c.size[1]+c.size[0];
    d.rgb=(random.next_integer()%31+225)|((random.next_integer()%31+225)<<8)|((random.next_integer()%31+225)<<16);
    d.alpha=0;
    if orbit {
        let radians=((f64::from(sample(random)*360.)*f64::from(std::f32::consts::PI))/180.) as f32;
        d.motion[2]=super::trunc(radians*1000.) as f32/1000.;
        let sign=sample(random)*2.-1.;d.size=(f64::from(sample(random))*0.1*f64::from(sign)) as f32;
        d.motion[1]=(sample(random)*2.-1.)*50.+c.radius;
        d.motion[0]=(sample(random)*2.-1.)*c.spread[1]+c.spread[1];
    } else {
        d.saved_position[0]=(sample(random)*2.-1.)*c.spread[0]+center[0];
        d.saved_position[1]=(sample(random)*2.-1.)*c.spread[1]+center[1];
        d.saved_position[2]=sample(random)*5.; //stored, but1550 uses only X/Y
        for i in 0..2 {
            d.motion[i]=sample(random);
            if random.next_integer()&1==0 {d.motion[i]*=-1.;}
        }
        let length=f64::from(d.motion[0]*d.motion[0]+d.motion[1]*d.motion[1]).sqrt() as f32;
        if length>0. {for n in &mut d.motion[..2] {*n/=length;}}
        let speed=(sample(random)*2.-1.)*c.speed[1]+c.speed[0];
        for n in &mut d.motion[..2] {*n*=speed;}
    }
    let alpha=(sample(random)*2.-1.)*(c.alpha as f32)+c.alpha as f32;
    //The original ordered bounds discard >=255 to zero; this is not a clamp.
    d.peak_alpha=super::trunc(if alpha>=0.&&alpha<255. {alpha} else {0.});
    let lifetime=(sample(random)*2.-1.)*(c.lifetime as f32)+c.lifetime as f32;
    d.lifetime=if orbit {super::trunc(lifetime) as f32} else {lifetime};
    d.clock=0;d.origin=0;d.phase=0;
}

// ohanaburst1b10: RNG order includes the atlas selector and independent X/Y
// radius samples. The saved Z really is the randomized size, not zero.
fn spawn_flower(d: &mut Sprite,size: f32,center: [f32;3],config: &FlowerConfig,
    random: &mut impl ParticleRandomSource) {
    let atlas=random.next_integer()%3;
    let origin=match atlas {0=>[0.,0.],1=>[0.5,0.],_=>[0.,0.5]};
    d.uv=rect_uv(origin,[0.5;2],false);
    d.size=sample(random)*size+size*0.5;d.scale=0.;d.stretch=[1.;2];
    let sign=if random.next_integer()&1!=0 {1.} else {-1.};
    d.rotation=(sample(random)*0.5+0.5)*sign;d.rotation_y=0.;d.centered=false;
    let mut direction=[sample(random)-0.5,sample(random)-0.5];
    let length=f64::from(direction[0]*direction[0]+direction[1]*direction[1]).sqrt() as f32;
    if length>0. {for n in &mut direction {*n/=length;}}
    d.saved_position=[0.,0.,d.size];
    for i in 0..2 {
        let factor=sample(random)*(1.-config.minimum[i])+config.minimum[i];
        d.saved_position[i]=center[i]+factor*(config.radius[i]*direction[i]);
    }
    d.position=d.saved_position;d.alpha=0;d.clock=0;d.origin=0;d.phase=0;
}

fn spawn_asease(d: &mut Sprite,index: usize,size: f32,radius: f32,angle: f32,
    center: [f32;3],time: [f32;2],random: &mut impl ParticleRandomSource) {
    let r=sample(random)*size;
    d.size=r*f32::from_bits(0x3e4c_cccd)+size*f32::from_bits(0x3f4c_cccd);
    d.scale=0.;d.alpha=0;
    // The PE stores these randomized/truncated times but update uses the
    // configured floats. Consume both samples in their original order.
    for t in time {let _unused=super::trunc((sample(random)*t)*0.5+t*0.5);}
    let sign=if index&1==0 {1.} else {-1.};
    let radians=((f64::from(angle)*f64::from(std::f32::consts::PI))/180.) as f32;
    let (s,c)=f64::from(radians).sin_cos();let (s,c)=(s as f32,c as f32);
    let x=c*sign;
    let low_y=(-s)*sign;let high_y=s*sign;
    let span_x=0.;let span_y=f64::from(low_y-high_y).powf(2.).sqrt() as f32;
    let dx=sample(random)*span_x-span_x*0.5+x;
    let dy=sample(random)*span_y-span_y*0.5;
    let len=f64::from(dx*dx+dy*dy).sqrt() as f32;
    let direction=if len>0. {[dx/len,dy/len,0.]} else {[dx,dy,0.]};
    let distance=(f64::from(sample(random))*f64::from(radius)+f64::from(radius)) as f32;
    d.saved_position=std::array::from_fn(|i|center[i]+direction[i]*distance);
    d.clock=0;d.origin=0;d.phase=0;
}
fn sample(random: &mut impl ParticleRandomSource) -> f32 {f32::from_bits(random.next_float_bits())}

fn rect_uv(origin: [f32;2],span: [f32;2],flip_x: bool) -> [[f32;2];4] {
    std::array::from_fn(|i|[origin[0]+(if flip_x {1-(i&1)} else {i&1}) as f32*span[0],
        origin[1]+(i>>1) as f32*span[1]])
}

fn atlas_uv(column: u32,flip_x: bool,flip_y: bool) -> [[f32;2];4] {
    std::array::from_fn(|i| {
        let x=if flip_x {1-(i&1)} else {i&1};
        let y=if flip_y {1-(i>>1)} else {i>>1};
        [column as f32*0.5+x as f32*0.5,y as f32*0.5]
    })
}

// SDK callback+78: absolute clock modulo absolute period, then four linear
//quarters. This is the shared triangular oscillator, not a sine surrogate.
pub(super) fn triangle(clock: f32,period: f32) -> f32 {
    let now=if clock.abs()>0. {clock.abs()%period.abs()} else {0.};
    let quarter=period.abs()*0.25;
    let fraction=if now>0. {now%quarter} else {0.}/quarter;
    if now<quarter {fraction} else if now<quarter+quarter {1.-fraction}
    else if now<quarter*3. {-fraction} else {fraction-1.}
}

fn scatter_position(from: [f32;3],to: [f32;3],clock: f32,duration: f32) -> [f32;3] {
    let clock=if clock<1. {1.} else {clock};
    if (clock as u32) as f32>=duration {return to;}
    let radians=|degrees: f32|((f64::from(degrees)*f64::from(std::f32::consts::PI))/180.) as f32;
    let start=radians(90.);let angle=super::lerp(start,radians(0.),duration,clock);
    let x=(f64::from(angle).cos() as f32)-(f64::from(start).cos() as f32);
    let start=radians(180.);let angle=super::lerp(start,radians(270.),duration,clock);
    let y=(f64::from(angle).sin() as f32)-(f64::from(start).sin() as f32);
    [from[0]+x*(to[0]-from[0]),from[1]-y*(to[1]-from[1]),from[2]+x*(to[2]-from[2])]
}
