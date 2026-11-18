//! Shared independently recovered volt path-emitter and ribbon cores.
//! Exact identities select their real clocks, alpha units, atlas and motion.
use super::*;

#[derive(Debug,Default)]
pub(super) struct VoltConfig {
    path: Trajectory,
    clock: u32,
    current: [f32;3],
    size: f32,
    alpha: f32,
    render_mode: u32,
    previous: [f32;3],
    previous_clock: u32,
    midpoint: [f32;3],
    durations: [i32;3],
}

impl PathFamily {
    pub(super) fn start_volt(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let old=self.profile.kind==PathKind::Volt;
        let start=std::array::from_fn(|i|word(p,i));
        let end=std::array::from_fn(|i|word(p,i+3));
        self.volt.path=volt_path(start,end,word(p,8),word(p,9),
            if old {trunc(word(p,6)) as f32} else {word(p,6)});
        self.volt.clock=0;self.volt.current=start;self.volt.size=word(p,10)/100.;
        self.volt.alpha=if old {word(p,7)} else {trunc(word(p,7)) as f32};
        // The nonzero flag is sticky across start, matching the PE write.
        if word(p,if old {11} else {12})!=0. {self.volt.render_mode=1;}
        self.main_count=self.drops.len();self.active_count=0;
        for d in &mut self.drops {
            spawn_volt(d,self.image,&self.volt,self.profile.kind,random);
        }
    }

    pub(super) fn update_volt(&mut self,tick: u32,offset: [f32;2],random: &mut impl ParticleRandomSource) {
        self.active_count=self.active_count.saturating_add(1).min(self.main_count);
        self.volt.current=self.volt.path.at(self.volt.clock);
        let old=self.profile.kind==PathKind::Volt;
        let atlas=self.profile.kind==PathKind::Volt3;
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase>2 {spawn_volt(d,self.image,&self.volt,self.profile.kind,random);}
            else {
                if d.phase==0 {
                    if !old {d.motion_origin=self.volt.current;}
                    d.phase=1;
                }
                d.position=if atlas {d.motion_origin} else {self.volt.current};
                if !atlas {
                    d.scale*=if old {f32::from_bits(0x3f7a_e148)} else {f32::from_bits(0x3f66_6666)};
                }
                let (from,to,boundary)=if d.phase==1 {(0.,self.volt.alpha,d.durations[0])}
                    else {(self.volt.alpha,0.,d.durations[1])};
                // The callback receives origin0 in BOTH stages. phase_origin
                // is recorded by the PE but is not subtracted from the age.
                d.alpha=trunc(super::super::lerp(from,to,boundary as f32,d.clock as i32 as f32)) as u8;
                if atlas {volt_atlas(d,random);}
                if d.clock as i32>boundary {d.phase+=1;d.phase_origin=d.clock;}
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
        self.volt.clock=self.volt.clock.wrapping_add(tick);
    }

    pub(super) fn volt_render_mode(&self) -> u32 {self.volt.render_mode}

    pub(super) fn start_volt_ribbon(&mut self,p: &[u8;128]) {
        let start=std::array::from_fn(|i|word(p,i));
        let end=std::array::from_fn(|i|word(p,i+3));
        self.volt.path=volt_path(start,end,word(p,11),word(p,12),trunc(word(p,6)) as f32);
        self.volt.clock=0;self.volt.previous_clock=0;
        self.volt.current=start;self.volt.previous=start;self.volt.midpoint=scale(add(start,end),0.5);
        self.volt.durations=std::array::from_fn(|i|trunc(word(p,i+7)));
        self.volt.alpha=word(p,10);self.volt.size=word(p,13)/100.;
        if trunc(word(p,14))!=0 {self.volt.render_mode=1;}
        self.main_count=self.drops.len();self.active_count=0;
        for d in &mut self.drops {reset_ribbon(d,self.image,self.volt.size);}
    }

    pub(super) fn start_slash_ribbon(&mut self,p: &[u8;128]) {
        let start=std::array::from_fn(|i|word(p,i));
        let control=std::array::from_fn(|i|word(p,i+3));
        let end=std::array::from_fn(|i|word(p,i+6));
        self.volt.path=Trajectory::default();
        for point in [start,control,end] {self.volt.path.append(point);}
        self.volt.path.finish_average(trunc(word(p,9)) as f32);
        let middle=scale(add(start,end),0.5);
        self.volt.midpoint=add(middle,scale(sub(middle,control),0.5));
        self.volt.clock=0;self.volt.previous_clock=0;self.volt.current=start;self.volt.previous=start;
        self.volt.durations=std::array::from_fn(|i|trunc(word(p,i+10)));
        self.volt.alpha=trunc(word(p,14)) as f32;self.volt.size=word(p,13)/100.;
        if trunc(word(p,15))!=0 {self.volt.render_mode=1;}
        self.main_count=self.drops.len();self.active_count=0;
        for d in &mut self.drops {reset_ribbon(d,self.image,self.volt.size);}
    }

    pub(super) fn update_volt_ribbon(&mut self,tick: u32) {
        let slash=self.profile.kind==PathKind::Slash;
        self.volt.current=self.volt.path.at(self.volt.clock);
        let distance=trunc(length(sub(self.volt.current,self.volt.previous))) as f32;
        let segments=trunc(distance/if slash {40.} else {10.}).max(0) as usize;
        // 2b75 uses a strict unsigned capacity comparison. At the exact
        // capacity the PE emits none, preserving its reserved free slot.
        if self.active_count.saturating_add(segments)<self.main_count {
            if segments==0 {
                spawn_ribbon(&mut self.drops[self.active_count],self.image,&self.volt,
                    self.volt.previous,self.volt.current);
                self.active_count+=1;
            } else {
                let step=self.volt.clock.wrapping_sub(self.volt.previous_clock) as i32 as f32/segments as f32;
                for n in 0..segments {
                    let time=self.volt.previous_clock as i32 as f32;
                    let a=self.volt.path.at_time(time+n as f32*step);
                    let b=self.volt.path.at_time(time+(n+1) as f32*step);
                    spawn_ribbon(&mut self.drops[self.active_count+n],self.image,&self.volt,a,b);
                }
                self.active_count+=segments;
            }
        }
        let mut index=0;
        while index<self.active_count {
            let d=&mut self.drops[index];
            let age=d.clock.wrapping_sub(d.phase_origin) as i32;
            // Slash's jump table2b24 maps phase3 straight to2892 (clock
            // only). It remains allocated; only phase>3 removes the slot.
            let expired=d.phase>if slash {3} else {2};
            if !expired {
                match d.phase {
                    0=>{
                        // Public phase0 supplies absolute clock to the
                        // callback, while transition compares elapsed age.
                        d.alpha=trunc(super::super::lerp(0.,self.volt.alpha,self.volt.durations[0] as f32,d.clock as i32 as f32)) as u8;
                        if age>self.volt.durations[0] {d.phase=1;d.phase_origin=d.clock;}
                    }
                    1=>{if age>self.volt.durations[1] {
                        d.phase=if slash && self.volt.durations[2]<0 {3} else {2};d.phase_origin=d.clock;
                    }}
                    2=>{
                        d.alpha=trunc(super::super::lerp(self.volt.alpha,0.,self.volt.durations[2] as f32,age as f32)) as u8;
                        if age>self.volt.durations[2] {d.phase=3;d.phase_origin=d.clock;}
                    }
                    3=>{},
                    _=>unreachable!(),
                }
            }
            d.clock=d.clock.wrapping_add(tick);
            if expired {
                let last=self.active_count-1;
                // 2e00 copies the LAST live SDK quad/private record into
                // this slot, then processes that same index in the same call.
                reset_ribbon(&mut self.drops[index],self.image,self.volt.size);
                if index!=last {
                    self.drops[index]=self.drops[last].clone();
                    reset_ribbon(&mut self.drops[last],self.image,self.volt.size);
                }
                self.active_count-=1;
            } else {index+=1;}
        }
        self.volt.previous_clock=self.volt.clock;self.volt.clock=self.volt.clock.wrapping_add(tick);
        self.volt.previous=self.volt.current;
    }
}

fn reset_ribbon(d: &mut Drop,image: [f32;2],size: f32) {
    d.pivot=image.map(|n|n*0.5);d.scale=size;d.stretch=[1.;2];d.alpha=0;
    d.clock=0;d.phase=0;d.phase_origin=0;
}

fn spawn_ribbon(d: &mut Drop,image: [f32;2],c: &VoltConfig,a: [f32;3],b: [f32;3]) {
    reset_ribbon(d,image,c.size);
    // 30b0/3190 write actual vertex coordinates, not SDK translation.
    // Both bottom vertices are the start/end midpoint configured at2470.
    d.geometry=Some([a,b,c.midpoint,c.midpoint]);
}

// 1840(start,end), reverseXYZ, cross2190, quantized Rz1770 and
// helper1a40's mean-chord tangents build the actual three-knot trajectory.
pub(super) fn volt_path(start: [f32;3],end: [f32;3],degrees: f32,amplitude: f32,duration: f32) -> Trajectory {
    let difference=sub(start,end);
    let forward=normalize_nonzero(difference);
    let reverse=normalize_nonzero([difference[2],difference[1],difference[0]]);
    let cross=normalize_nonzero([
        reverse[1]*forward[2]-reverse[2]*forward[1],
        reverse[2]*forward[0]-forward[2]*reverse[0],
        reverse[0]*forward[1]-reverse[1]*forward[0],
    ]);
    let angle=((f64::from(degrees)*f64::from(std::f32::consts::PI))/180.) as f32;
    let angle=trunc(angle*1000.) as f32/1000.;
    let (sin,cos)=f64::from(angle).sin_cos();let (sin,cos)=(sin as f32,cos as f32);
    let rotated=[cross[0]*cos-cross[1]*sin,cross[0]*sin+cross[1]*cos,cross[2]];
    let control=add(scale(add(start,end),0.5),scale(rotated,amplitude));
    let mut path=Trajectory::default();path.append(start);path.append(control);path.append(end);
    path.finish_average(duration);path
}

fn spawn_volt(d: &mut Drop,image: [f32;2],c: &VoltConfig,kind: PathKind,
    random: &mut impl ParticleRandomSource) {
    d.pivot=image.map(|n|n*0.5);d.alpha=0;
    d.position=[c.current[0],c.current[1],0.];d.motion_origin=d.position;
    if kind==PathKind::Volt3 {
        // Spawn2db0 uses full image HEIGHT as pivotY, unlike2cb0/2ce0.
        d.pivot[1]=image[1];volt_atlas(d,random);
        d.scale=1.;d.stretch=[c.size*f32::from_bits(0x3e99_999a),
            (sample(random)*2.-1.)*0.25+c.size*f32::from_bits(0x3e4c_cccd)];
        d.rotation=sample(random)*std::f32::consts::TAU;
        d.durations[0]=trunc((sample(random)*2.-1.)*5.+10.);
        d.durations[1]=trunc((sample(random)*2.-1.)*15.+20.);
    } else {
        d.scale=c.size;d.stretch=[1.;2];
        d.durations=if kind==PathKind::Volt {[3,30,0]} else {[5,20,0]};
    }
    d.clock=0;d.phase=0;d.phase_origin=0;d.captured=d.position;
}

fn volt_atlas(d: &mut Drop,random: &mut impl ParticleRandomSource) {
    let atlas=random.next_integer()&3;
    d.uv=std::array::from_fn(|i|[atlas as f32*0.25+(i&1) as f32*0.25,(i>>1) as f32]);
}
