//! SDK2 six-knot floating particles: dust, twinkle and spark2/wc_spark.
//! helper1120 sets childCount=0. Each exact model keeps its own spawn RNG,
//! baseline, SDK rotation and alpha; unreachable trail bodies add no RNG.
use super::*;

#[derive(Debug,Default)]
pub(super) struct FloatingConfig {
    size:f32,step:f32,tangent:f32,durations:[i32;3],horizontal:[f32;2],
    direction:i32,vertical:f32,period:u32,batch:u32,
    baseline:Option<Trajectory>,spin:[[f32;2];3],legacy:bool,
}

fn spawn_floating(d:&mut Drop,c:&FloatingConfig,twinkle:bool,image:[f32;2],random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);
    let baseline=c.baseline.is_some();let spark=baseline && !c.legacy;
    if twinkle || baseline {
        let value=random.next_integer();let tile=if c.legacy {value%3} else {value&3};
        d.uv=std::array::from_fn(|i|[((tile&1)+(i&1) as u32) as f32*0.5,((tile>>1)+(i>>1) as u32) as f32*0.5]);
    }
    let size_sample=f64::from(sample(random));
    d.scale=if baseline {((size_sample+1.)*f64::from(c.size)) as f32}
        else {((size_sample*0.1+0.9)*f64::from(c.size)) as f32};
    d.stretch=[1.;2];d.vertex_rgb=None;d.alpha=0;
    if c.legacy {
        d.vertex_rgb=Some(std::array::from_fn(|_|0x00ff_0000
            |((trunc((f64::from(sample(random))*255.) as f32) as u8 as u32)<<8)));
    }
    d.position=if let Some(baseline)=&c.baseline {
        baseline.at_time((f64::from(sample(random))*100.) as f32)
    } else {
        // These are dust/twinkle PE constants, not guessed Canvas extent.
        [(f64::from(sample(random))*1480.-100.) as f32,
            (f64::from(sample(random))*920.-100.) as f32,0.]
    };
    if baseline {
        d.rotation=(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32;
    }
    if spark {
        //29b0 consumes X, Y, Z in this order; parameter pairs are Z, X, Y.
        d.motion=std::array::from_fn(|axis|sample(random)*c.spin[axis][1]
            +c.spin[axis][0]-c.spin[axis][1]*0.5);
    }
    d.saved_alpha=if twinkle {(random.next_integer()%100+156) as f32} else {255.};
    d.clock=0;d.phase=0;d.captured=d.position;
    let total=c.durations[0].wrapping_add(c.durations[1]).wrapping_add(c.durations[2]);
    d.path=drift_path(d.position,c.horizontal,c.direction,c.vertical,c.step,
        if c.legacy {50.} else {360.},total as f32,c.tangent,random);
}

// Shared by the floating variants and s_080107's1700. Their initialization,
// atlas, random spin, activation and lifetime remain independent consumers.
pub(super) fn drift_path(origin:[f32;3],horizontal:[f32;2],direction:i32,
    vertical:f32,step:f32,degrees:f32,duration:f32,tangent:f32,
    random:&mut impl ParticleRandomSource)->Trajectory {
    let mut path=Trajectory::default();path.append(origin);
    let x=(f64::from(sample(random))*f64::from(horizontal[1])-f64::from(horizontal[0])) as f32;
    let y=match direction {
        1=>-(sample(random)*vertical+0.05),
        2=>(f64::from(sample(random))*f64::from(vertical)-f64::from(vertical*0.5)) as f32,
        _=>(f64::from(sample(random))*f64::from(vertical)+f64::from(0.05f32)) as f32,
    };
    let direction=normalize_nonzero([x,y,0.]);
    let distance=(f64::from(sample(random))*f64::from(step)+f64::from(step)) as f32;
    let mut current=add(origin,scale(direction,distance));path.append(current);
    let initial=normalize_nonzero(sub(current,origin));
    // SDK10 converts degrees to radians before SDK18/1c sine/cosine.
    let angle=(f64::from(degrees)*f64::from(std::f32::consts::PI)/180.) as f32;
    let (sn,cs)=f64::from(angle).sin_cos();let (sn,cs)=(sn as f32,cs as f32);
    let left=[initial[0]*cs+initial[1]*sn,-initial[0]*sn+initial[1]*cs];
    let right=[initial[0]*cs-initial[1]*sn,initial[0]*sn+initial[1]*cs];
    let width=[(right[0]-left[0]).abs(),(right[1]-left[1]).abs()];
    for _ in 0..4 {
        let direction=std::array::from_fn(|axis|if axis<2 {
            let jitter=(f64::from(sample(random))*f64::from(width[axis])-f64::from(width[axis]*0.5)) as f32;
            jitter+initial[axis]
        } else {0.});
        let direction=normalize_nonzero(direction);
        let distance=(f64::from(sample(random))*f64::from(step)+f64::from(step)) as f32;
        current=add(current,scale(direction,distance));path.append(current);
        if random.next_integer()%15==0 {break;}
    }
    path.finish(duration,tangent);path
}

// Fixed SDK callback1b04b0: remainder by period, then period/4; a triangle
// over all four quadrants. This does not consume particle RNG or host time.
fn twinkle_wave(now:f32)->f32 {
    let phase=now.abs()%8.;let fraction=(phase%2.)/2.;
    if phase<2. {fraction} else if phase<4. {1.-fraction}
        else if phase<6. {-fraction} else {fraction-1.}
}

impl PathFamily {
    pub(super) fn start_floating(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let twinkle=self.profile.kind==PathKind::Twinkle;
        let spark=self.profile.kind==PathKind::FloatingSpark;
        let legacy=self.profile.kind==PathKind::ASpark;
        let size=word(p,if legacy {5} else {4})/100.;
        let baseline=if spark || legacy {
            let mut path=Trajectory::default();
            let first=usize::from(legacy);
            path.append([word(p,first),word(p,first+1),0.]);
            path.append([word(p,first+2),word(p,first+3),0.]);path.finish(100.,1.);Some(path)
        } else {None};
        if spark && trunc(word(p,16))!=0 {self.stream_render_mode=1;}
        self.floating=FloatingConfig {size:if !legacy && 0.01>size {0.01} else {size},
            step:word(p,if legacy {6} else {5}),tangent:if legacy {word(p,10)} else {word(p,5)*1.25},
            durations:std::array::from_fn(|i|if legacy {trunc(word(p,7+i))} else {trunc(word(p,6+i)).max(0)}),
            horizontal:if legacy {[word(p,11)/100.,word(p,12)/100.]} else {[word(p,9)/100.,word(p,10)/100.]},
            direction:trunc(word(p,if legacy {13} else {11})),vertical:word(p,if legacy {14} else {12})/100.,
            period:if legacy {10} else {61i32.wrapping_sub(trunc(word(p,13))).clamp(1,60) as u32},
            batch:if legacy {1} else {trunc(word(p,14)) as u32},
            baseline,legacy,spin:std::array::from_fn(|axis| {
                let index=[19,21,17][axis];[word(p,index)/100.,word(p,index+1)/100.]
            })};
        self.main_count=self.drops.len();self.child_count=0;
        self.active_count=if trunc(word(p,15))!=0 {self.main_count/2} else {1};
        let c=&self.floating;
        for (index,d) in self.drops.iter_mut().enumerate() {
            d.phase_origin=0;
            spawn_floating(d,c,twinkle,self.image,random);
            if index<self.active_count && trunc(word(p,15))!=0 {
                let total=c.durations[0].wrapping_add(c.durations[1]).wrapping_add(c.durations[2]);
                d.clock=trunc((f64::from(sample(random))*f64::from(total as f32)) as f32) as u32;
                if !legacy && d.clock as i32>c.durations[0] {d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(c.durations[0] as u32);d.alpha=255;}
                // The wrapper compares the absolute clock with hold alone.
                if !legacy && d.clock as i32>c.durations[1] {d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(c.durations[1] as u32);}
            }
        }
    }

    pub(super) fn update_floating(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        let twinkle=self.profile.kind==PathKind::Twinkle;let c=&self.floating;
        let spark=c.baseline.is_some() && !c.legacy;
        if random.next_integer()%c.period==0 {self.active_count=(self.active_count as u32).wrapping_add(c.batch) as usize;}
        self.active_count=self.active_count.min(self.main_count);
        for d in self.drops.iter_mut().take(self.active_count) {
            let now=d.clock as i32 as f32;
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            if spark && d.phase<3 {
                d.rotation+=d.motion[2];
                d.rotation_xy[0]+=d.motion[0];d.rotation_xy[1]+=d.motion[1];
            }
            match d.phase {
                0=> {
                    d.position=d.path.at(d.clock);
                    let alpha=trunc(super::super::lerp(0.,d.saved_alpha,c.durations[0] as f32,now));
                    d.alpha=if twinkle {trunc((twinkle_wave(now)*alpha as f32)*0.1+alpha as f32*0.9) as u8} else {alpha as u8};
                    if d.clock as i32>c.durations[0] {d.phase=1;d.phase_origin=d.clock;}
                }
                1=> {
                    d.position=d.path.at(d.clock);
                    if twinkle {d.alpha=trunc((twinkle_wave(now)*d.saved_alpha)*0.1+d.saved_alpha*0.9) as u8;}
                    if elapsed>c.durations[1] {d.phase=2;d.phase_origin=d.clock;}
                }
                2=> {
                    d.position=d.path.at(d.clock);
                    let alpha=trunc(super::super::lerp(d.saved_alpha,0.,c.durations[2] as f32,elapsed as f32));
                    d.alpha=if twinkle {trunc((twinkle_wave(now)*alpha as f32)*0.1+alpha as f32*0.9) as u8} else {alpha as u8};
                    // Only spark2 produces private angular steps. Dust and
                    // twinkle retain the SDK rotation initialized by1120.
                    if elapsed>c.durations[2] {d.phase=3;d.phase_origin=d.clock;}
                }
                _=>spawn_floating(d,c,twinkle,self.image,random),
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
