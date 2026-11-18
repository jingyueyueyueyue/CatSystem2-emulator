//! happy1070/1140/1410 and photon1070/1180/1460. The SDK storage,
//! interpolation and texture submission are shared; the recovered emitters
//! deliberately retain different absolute/relative clocks and random calls.
use super::*;

#[derive(Debug,Default)]
pub(super) struct GlowConfig {
    spread:[f32;3],
    time:[f32;3],
    initial_distance:f32,
    emission_period:i32,
    emission_step:i32,
    respawn:bool,
}

//dark's module10d70 is initialized to1, shared between instances and not
//reset by1070/finish. This is a separate emitter from the other glow PEs.
static DARK_ACTIVE:AtomicU32=AtomicU32::new(1);

impl PathFamily {
    pub(super) fn start_dark(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.scale=word(p,6)/200.;self.factor=word(p,7)/100.;self.direction=trunc(word(p,5));
        self.glow.respawn=trunc(word(p,8))==0;self.main_count=self.drops.len();
        for d in &mut self.drops {d.phase_origin=0;spawn_dark(d,self.image,self.scale,p,random);}
    }

    pub(super) fn update_dark(&mut self,tick:u32,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let active=DARK_ACTIVE.load(Ordering::Relaxed).wrapping_add(self.direction as u32).min(self.main_count as u32);
        DARK_ACTIVE.store(active,Ordering::Relaxed);self.active_count=active as usize;
        for d in self.drops.iter_mut().take(self.active_count) {
            let now=d.clock as i32 as f32;
            match d.phase {
                0=>{d.alpha=128;d.position[1]-=self.factor;d.phase=1;},
                1=>{
                    d.scale=(f64::from(d.scale)*f64::from(0.99f32)) as f32;d.position[1]-=self.factor;d.alpha=128;
                    if now>=(f64::from(word(p,0))*f64::from(0.2f32)) as f32 {d.phase=2;}
                },
                2=>{
                    let start=(f64::from(word(p,0))*f64::from(0.2f32)) as f32;
                    d.alpha=fade(128.,0.,start,word(p,0),now);
                    d.scale=(f64::from(d.scale)*f64::from(0.99f32)) as f32;d.position[1]-=self.factor;
                    if now>=word(p,0) {d.phase=3;d.alpha=0;}
                },
                _=>if self.glow.respawn {spawn_dark(d,self.image,self.scale,p,random);},
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn start_happy(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.center=[word(p,0),word(p,1),0.];self.scale=word(p,2)/100.;self.factor=word(p,3)/100.;
        self.glow.initial_distance=word(p,4);self.glow.time=std::array::from_fn(|i|word(p,i+5));
        self.main_count=self.drops.len();self.active_count=0;
        for d in &mut self.drops {
            //1660 clears the private records on start, separately from SDK
            //transforms. A later1410 respawn does not clear phase_origin.
            d.phase_origin=0;spawn_happy(d,self.image,self.center,self.scale,self.factor,random);
        }
    }

    pub(super) fn update_happy(&mut self,tick:u32,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        if random.next_integer()%15==0 {self.active_count+=1;}
        self.active_count=self.active_count.min(self.main_count);
        for d in self.drops.iter_mut().take(self.active_count) {
            let now=d.clock as i32 as f32;
            match d.phase {
                0=>{
                    d.position[0]+=d.motion[0]*self.glow.initial_distance;
                    d.position[1]+=d.motion[1]*self.glow.initial_distance;d.phase=1;
                },
                1..=3=>{
                    d.position=add(d.position,d.motion);d.rotation+=d.angular_step;
                    match d.phase {
                        1=>{
                            d.alpha=fade(0.,255.,0.,self.glow.time[0],now);
                            if now>=self.glow.time[0] {d.phase=2;d.phase_origin=d.clock;}
                        },
                        2=>if now>=self.glow.time[1] {d.phase=3;d.phase_origin=d.clock;},
                        _=>{
                            //1353 supplies phase_origin as START and p7 as
                            //absolute END; it is not a relative duration.
                            d.alpha=fade(255.,0.,d.phase_origin as i32 as f32,self.glow.time[2],now);
                            if now>=self.glow.time[2] {d.phase=4;d.phase_origin=d.clock;d.alpha=0;}
                        },
                    }
                },
                _=>if !(word(p,0) < -9000.) {spawn_happy(d,self.image,self.center,self.scale,self.factor,random);},
            }
            d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn start_photon(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;
        self.center=[word(p,1),word(p,2),0.];self.scale=word(p,6)/100.;
        self.glow.spread=std::array::from_fn(|i|word(p,i+3));
        self.glow.time=std::array::from_fn(|i|word(p,i+7));
        self.glow.emission_period=trunc(word(p,10));self.glow.emission_step=trunc(word(p,11));
        self.glow.respawn=trunc(word(p,12))!=0;
        self.active_count=if self.glow.emission_period<=0 {self.main_count} else {self.glow.emission_step as u32 as usize};
        for d in self.drops.iter_mut().take(self.main_count) {
            d.phase_origin=0;spawn_photon(d,self.image,self.center,self.scale,&self.glow,random);
        }
        //1720 maps partition children but the public1180/1460 never updates
        //or hides their SDK quads. Preserve their actual initialized storage.
    }

    pub(super) fn update_photon(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        if self.active_count as i32>=self.main_count as i32 {self.active_count=self.main_count;}
        else if self.glow.emission_period>0 && random.next_integer()%self.glow.emission_period as u32==0 {
            self.active_count=(self.active_count as u32).wrapping_add(self.glow.emission_step as u32) as usize;
        }
        let count=(self.active_count as i32).max(0) as usize;
        for d in self.drops.iter_mut().take(count.min(self.main_count)) {
            let now=d.clock as i32 as f32;let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            match d.phase {
                0=>{
                    d.alpha=fade(0.,255.,0.,self.glow.time[0],now);
                    if now>=self.glow.time[0] {d.phase=1;d.phase_origin=d.clock;}
                },
                1=>{
                    d.position=d.path.at_time(age);
                    if age>=self.glow.time[1] {d.phase=2;d.phase_origin=d.clock;}
                },
                2=>{
                    d.position=d.path.at_time(self.glow.time[1]+age);
                    d.alpha=fade(255.,0.,0.,self.glow.time[2],age);
                    if age>=self.glow.time[2] {d.phase=3;d.phase_origin=d.clock;d.alpha=0;}
                },
                _=>if self.glow.respawn {spawn_photon(d,self.image,self.center,self.scale,&self.glow,random);},
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn fade(from:f32,to:f32,start:f32,end:f32,now:f32)->u8 {
    trunc(super::super::lerp(from,to,end-start,now-start)) as u8
}

fn spawn_dark(d:&mut Drop,image:[f32;2],size:f32,p:&[u8;128],random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.alpha=0;d.stretch=[1.;2];
    d.scale=(f64::from(sample(random))*f64::from(size)+f64::from(size)) as f32;
    d.path=Trajectory::default();d.path.append([word(p,1),word(p,2),0.]);d.path.append([word(p,3),word(p,4),0.]);
    d.path.finish(word(p,0),1.);
    d.position=d.path.at_time(sample(random)*word(p,0));d.captured=d.position;d.clock=0;d.phase=0;
}

fn spawn_happy(d:&mut Drop,image:[f32;2],center:[f32;3],size:f32,speed:f32,
    random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.scale=size;d.stretch=[1.;2];
    d.position=[center[0],center[1],(f64::from(sample(random))*50.) as f32];
    d.rotation=0.;d.rotation_xy=[0.;2];d.vertex_rgb=Some([0x00ff_ffeb;4]);d.alpha=0;
    let sign=if random.next_integer()&1!=0 {1.} else {-1.};
    d.angular_step=((f64::from(sample(random))*f64::from(0.02f32)+f64::from(0.02f32))*sign) as f32;
    for _ in 0..10 {
        d.motion=normalize_nonzero([sample(random)-0.5,sample(random)-0.5,0.]);
        if -d.motion[1]>-0.5 {break;}
    }
    d.motion=scale(d.motion,speed);d.clock=0;d.phase=0;
}

fn spawn_photon(d:&mut Drop,image:[f32;2],center:[f32;3],size:f32,c:&GlowConfig,
    random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.scale=(f64::from(sample(random))*f64::from(size)+f64::from(0.1f32)) as f32;
    d.stretch=[1.;2];d.alpha=0;
    let direction=normalize_nonzero(std::array::from_fn(|i|
        (f64::from(sample(random))*f64::from(c.spread[i])-f64::from(c.spread[i])*0.5) as f32));
    d.position=std::array::from_fn(|i|direction[i]*c.spread[i]+center[i]);
    d.clock=0;d.phase=0;d.captured=d.position;d.path=Trajectory::default();
    d.path.append(d.position);d.path.append(center);d.path.finish(c.time[1]+c.time[2],100.);
    //1623 stores this private scalar even though update never reads it.
    d.motion_limit=(f64::from(sample(random))*f64::from(0.2f32)+f64::from(0.6f32)) as f32;
}
