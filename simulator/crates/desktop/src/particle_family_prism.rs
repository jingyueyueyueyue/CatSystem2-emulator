//! prism3/s_04010210b0/1260/1740. The zero-child helper2090 makes
//! the embedded trail code unreachable; the actual effect is the parent path.
use super::*;

// Original prism10a0/1140/17b0/1ba0; distinct from prism3 below.
#[derive(Debug,Default)]
pub(super) struct PrismTrailConfig { from:Vec<[f32;3]>,to:Vec<[f32;3]> }

fn spawn_prism_trail(d:&mut Drop,children:&mut [Drop],image:[f32;2],p:&[u8;128],
    initial:bool,random:&mut impl ParticleRandomSource)->([f32;3],[f32;3]) {
    d.pivot=image.map(|v|v*0.5);
    d.scale=(f64::from(sample(random))*f64::from(0.1f32)+f64::from(0.1f32)) as f32;
    d.stretch=[1.;2];d.clock=0;d.phase=0;
    //183f/1c2c explicitly convert the underflowed DWORD as unsigned.
    let x=(random.next_integer()%1400).wrapping_sub(100) as f32;
    let z=if trunc(word(p,3))!=0 {(random.next_integer()%900) as f32} else {0.};
    let from=[x,-100.,z];
    let to=[x,(f64::from(sample(random))*300.+620.) as f32,0.];
    d.position=from;d.rotation=(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32;
    if initial {
        if trunc(word(p,2))!=0 {d.clock=trunc(sample(random)*word(p,1)) as u32;}
        //1ba0 leaves captured/motion unchanged, unlike respawn17b0.
    } else {d.captured=from;d.motion=normalize_nonzero(sub(to,from));}
    for child in children {
        child.clock=0;child.phase=0;child.position=d.position;
        child.vertex_rgb=d.vertex_rgb;child.vertex_alpha=d.vertex_alpha;
        child.alpha=d.alpha;child.uv=d.uv;child.pivot=d.pivot;
        //The PE copies colors/UV/pivot here, not rotation or scale.
    }
    (from,to)
}

impl PathFamily {
    pub(super) fn start_prism_trail(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;
        self.active_count=if trunc(word(p,2))!=0 {self.main_count} else {0};
        self.prism_trail.from.clear();self.prism_trail.to.clear();
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let base=index*self.child_count;
            let (from,to)=spawn_prism_trail(d,&mut children[base..base+self.child_count],self.image,p,true,random);
            self.prism_trail.from.push(from);self.prism_trail.to.push(to);
        }
    }

    pub(super) fn update_prism_trail(&mut self,tick:u32,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let period=trunc(word(p,4)).max(1) as u32;
        let emit=period==1 || random.next_integer()%period==0;
        let mut emitted=false;
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        //1140 only clamps active; it never grows the zero non-warm count.
        self.active_count=self.active_count.min(self.main_count);
        if emit {
            for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
                if d.phase==2 && (!emitted || period==1) {
                    let base=index*self.child_count;
                    let (from,to)=spawn_prism_trail(d,&mut children[base..base+self.child_count],self.image,p,false,random);
                    self.prism_trail.from[index]=from;self.prism_trail.to[index]=to;emitted=true;
                }
            }
        }
        let duration=word(p,1);let fade_start=(f64::from(duration)*f64::from(0.9f32)) as f32;
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let now=d.clock as i32 as f32;
            if d.phase<=1 {
                d.position=std::array::from_fn(|axis|super::super::lerp(
                    self.prism_trail.from[index][axis],self.prism_trail.to[index][axis],duration,now));
                if d.phase==0 {
                    d.alpha=(random.next_integer()%135+120) as u8;
                    //SDK50 is floor(x+.5), distinct from the CRT truncator.
                    if (d.clock as i32)>trunc((fade_start+0.5).floor()) {d.phase=1;d.saved_alpha=f32::from(d.alpha);}
                } else {
                    d.alpha=trunc(super::super::lerp(d.saved_alpha,0.,duration-fade_start,now-fade_start)) as u8;
                    if now>duration {d.phase=2;d.phase_origin=d.clock;d.alpha=0;}
                }
            }
            let base=index*self.child_count;let trail=&mut children[base..base+self.child_count];
            if let Some(child)=trail.iter_mut().find(|child|child.phase==0) {
                //The normalized-segment acos result is discarded in1567.
                if length(sub(d.position,d.captured))!=0. {child.rotation=d.rotation;child.rotation_xy=d.rotation_xy;}
                child.pivot=d.pivot;child.scale=d.scale;child.stretch=d.stretch;child.phase=1;
            }
            for child in trail.iter_mut().filter(|child|child.phase!=0) {
                //16bd overwrites the earlier midpoint with the parent.
                child.position=d.position;child.alpha=trunc((f64::from(sample(random))*128.+127.) as f32) as u8;
                child.clock=child.clock.wrapping_add(1);
                if child.clock>self.child_count as u32 {child.clock=0;child.phase=0;}
                //1740 does not clear alpha on the final submitted frame.
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}

#[derive(Debug,Default)]
pub(super) struct PrismConfig {origin:[f32;3],target:[f32;3],size:f32,spread:f32,durations:[i32;3]}

#[derive(Debug,Default)]
pub(super) struct PrismOrbitConfig {center:[f32;2],tilt:f32,size:f32,size_range:f32,radius:f32,peak:i32,durations:[i32;3],blend:u32}

fn spawn_prism_orbit(d:&mut Drop,c:&PrismOrbitConfig,image:[f32;2],random:&mut impl ParticleRandomSource) {
    //1b45 writes full image HEIGHT to pivotY, half WIDTH to pivotX.
    d.pivot=[image[0]*0.5,image[1]];
    d.scale=((sample(random)*2.-1.)*c.size_range)+c.size;d.stretch=[1.,6.];
    d.vertex_rgb=Some(std::array::from_fn(|_| {
        let primary=random.next_integer()%3;
        let rgb=std::array::from_fn::<_,3,_>(|axis|if axis==primary as usize {
            random.next_integer()%31+225
        } else {random.next_integer()%30});
        rgb[0]|(rgb[1]<<8)|(rgb[2]<<16)
    }));
    let angle=(f64::from(sample(random))*360.) as f32;
    let angle=(f64::from(angle)*f64::from(std::f32::consts::PI)/180.) as f32;
    d.motion[0]=trunc(angle*1000.) as f32/1000.;
    let sign=(f64::from(sample(random))*2.-1.) as f32;
    d.motion[1]=(f64::from(sample(random))*f64::from(0.01f32)*f64::from(sign)) as f32;
    d.motion_limit=(((sample(random)*2.-1.)*c.radius)*0.5)+c.radius;
    let peak=((sample(random)*2.-1.)*c.peak as f32)+c.peak as f32;
    d.saved_alpha=trunc(if 0.>peak {0.} else if 255.<=peak {255.} else {peak}) as f32;
    d.clock=0;d.phase_origin=0;d.phase=0;d.alpha=0;
    d.durations=std::array::from_fn(|i| {
        let duration=c.durations[i] as f32;
        trunc((((sample(random)*2.-1.)*duration)*0.5)+duration)
    });
}

fn spawn_prism(d:&mut Drop,c:&PrismConfig,image:[f32;2],random:&mut impl ParticleRandomSource) {
    let tile=(random.next_integer()%30).min(3);
    let origin=[(tile&1) as f32*0.5,(tile>>1) as f32*0.5];
    d.uv=std::array::from_fn(|i|[origin[0]+(i&1) as f32*0.5,origin[1]+(i>>1) as f32*0.5]);
    d.pivot=image.map(|n|n*0.5);
    d.scale=(f64::from(c.size)*f64::from(sample(random))+f64::from(c.size)) as f32;
    d.stretch=[1.;2];d.alpha=0;d.position=c.origin;
    d.rotation_xy=if tile<2 {[0.2,0.9]} else {[0.;2]};
    d.rotation=(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32;
    d.clock=0;d.phase=0;d.captured=d.position;d.path=Trajectory::default();d.path.append(c.origin);
    //SDK lerp stores the random angle before the matrix helper quantizes it.
    let now=(f64::from(sample(random))*100.) as f32;
    let half=(f64::from(c.spread)*0.5) as f32;
    let angle=super::super::lerp(-half,half,100.,now);
    let angle=trunc((f64::from(angle)*1000.) as f32) as f32/1000.;
    let (s,cos)=f64::from(angle).sin_cos();let (s,cos)=(s as f32,cos as f32);
    let vector=sub(c.target,c.origin);
    //1b3c adds origin BEFORE normalization;1cce then appends the scaled
    //normalized absolute point. Replacing it with origin+direction changes PE behavior.
    let world=[vector[0]*cos-vector[1]*s+c.origin[0],vector[0]*s+vector[1]*cos+c.origin[1],vector[2]+c.origin[2]];
    let half_distance=(f64::from(length(sub(c.origin,c.target)))*0.5) as f32;
    let distance=(f64::from(half_distance)*f64::from(sample(random))+f64::from(half_distance)) as f32;
    d.path.append(scale(normalize_nonzero(world),distance));
    let total=c.durations[0].wrapping_add(c.durations[1]).wrapping_add(c.durations[2]);
    d.path.finish(total as f32,1.);
}

impl PathFamily {
    pub(super) fn start_prism_orbit(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let tilt=(f64::from(word(p,3))*f64::from(std::f32::consts::PI)/180.) as f32;
        let blend=if trunc(word(p,13))!=0 {1} else {self.prism_orbit.blend};
        self.prism_orbit=PrismOrbitConfig {center:[word(p,0),word(p,1)],
            tilt:if 0.>tilt {0.} else if std::f32::consts::FRAC_PI_2<=tilt {std::f32::consts::FRAC_PI_2} else {tilt},
            size:word(p,6)/1000.,size_range:word(p,7)/1000.,radius:word(p,8),peak:trunc(word(p,9)),
            durations:std::array::from_fn(|i|trunc(word(p,10+i))),blend};
        self.main_count=self.drops.len();self.child_count=0;self.active_count=0;
        for d in &mut self.drops {spawn_prism_orbit(d,&self.prism_orbit,self.image,random);}
    }

    pub(super) fn update_prism_orbit(&mut self,tick:u32,offset:[f32;2],random:&mut impl ParticleRandomSource) {
        self.active_count=self.active_count.wrapping_add(1).min(self.main_count);
        let c=&self.prism_orbit;
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase<=2 {
                d.motion[0]+=d.motion[1];
                let (sin,cos)=f64::from(d.motion[0]).sin_cos();
                d.position=[(sin * -f64::from(d.motion_limit)+f64::from(c.center[0])) as f32,c.center[1],
                    (cos*f64::from(d.motion_limit)) as f32];
                let direction=normalize_nonzero([c.center[0]-d.position[0],0.,-d.position[2]]);
                let angle=f64::from(-direction[2]).acos() as f32;
                d.rotation_xy=[-c.tilt,if direction[0]>0. {-angle} else {angle}];
                let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
                let duration=d.durations[d.phase as usize];
                if d.phase!=1 {
                    let (from,to)=if d.phase==0 {(0.,d.saved_alpha)} else {(d.saved_alpha,0.)};
                    d.alpha=trunc(super::super::lerp(from,to,duration as f32,elapsed as f32)) as u8;
                }
                if elapsed>duration {d.phase+=1;d.phase_origin=d.clock;}
            } else {spawn_prism_orbit(d,c,self.image,random);}
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn prism_orbit_render_mode(&self)->u32 {self.prism_orbit.blend}

    pub(super) fn start_prism(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.prism=PrismConfig {origin:[word(p,0),word(p,1),0.],target:[word(p,2),word(p,3),0.],
            size:(f64::from(word(p,4))/1000.) as f32,
            spread:(f64::from(word(p,5))*f64::from(std::f32::consts::PI)/180.) as f32,
            durations:std::array::from_fn(|i|trunc(word(p,6+i)))};
        self.main_count=self.drops.len();self.child_count=0;
        let randomize=trunc(word(p,9))!=0;
        self.active_count=if randomize {self.main_count/2} else {1};
        let total=self.prism.durations[0].wrapping_add(self.prism.durations[1]).wrapping_add(self.prism.durations[2]);
        for (index,d) in self.drops.iter_mut().enumerate() {
            d.phase_origin=0;spawn_prism(d,&self.prism,self.image,random);
            if randomize && index<self.main_count/2 {d.clock=(f64::from(sample(random))*f64::from(total)).trunc() as i32 as u32;}
        }
    }

    pub(super) fn update_prism(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        self.active_count=self.active_count.wrapping_add(1).min(self.main_count);
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase<=2 {
                d.position=d.path.at(d.clock);
                let elapsed=if d.phase==0 {d.clock as i32} else {d.clock.wrapping_sub(d.phase_origin) as i32};
                let duration=self.prism.durations[d.phase as usize];
                if d.phase!=1 {
                    let (from,to)=if d.phase==0 {(0.,255.)} else {(255.,0.)};
                    d.alpha=trunc(super::super::lerp(from,to,duration as f32,elapsed as f32)) as u8;
                }
                if elapsed>duration {d.phase+=1;d.phase_origin=d.clock;}
            } else {spawn_prism(d,&self.prism,self.image,random);}
            d.rotation=(f64::from(d.rotation)+f64::from(0.05f32)) as f32;
            d.scale=(f64::from(d.scale)*f64::from(0.997f32)) as f32;
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
