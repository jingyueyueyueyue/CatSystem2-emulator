//! Independent firebase2/3/4 models: shared three-phase receiver, exact
//! parameter layouts and atlas/motion differences from the fixed PE data.
use super::*;

#[derive(Debug,Default)]
pub(super) struct FirebaseConfig {
    center: [f32;3], size: f32, peak: i32,
    render_mode: u32, counter: i32, fade: bool,
}

#[derive(Debug,Default)]
pub(super) struct FirebaseStreamConfig {
    center: [f32;2], size: f32, peak: i32, render_mode: u32,
}

impl PathFamily {
    pub(super) fn start_firebase_stream(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        self.firebase_stream.center=[word(p,0),word(p,1)];
        self.firebase_stream.size=word(p,2)/1000.;self.firebase_stream.peak=trunc(word(p,3));
        if word(p,4)!=0. {self.firebase_stream.render_mode=1;}
        self.main_count=self.drops.len();self.active_count=0;
        for d in &mut self.drops {
            d.phase_origin=0;spawn_firebase_stream(d,self.image,&self.firebase_stream,random);
        }
    }

    pub(super) fn update_firebase_stream(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        self.active_count=self.active_count.saturating_add(1).min(self.main_count);
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase>2 {spawn_firebase_stream(d,self.image,&self.firebase_stream,random);} else {
                d.motion_origin[1]+=0.025;firebase_stream_uv(d);
                let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
                let phase=d.phase as usize;
                let (from,to)=if phase==2 {(self.firebase_stream.peak as f32,0.)} else {(0.,self.firebase_stream.peak as f32)};
                // 1820 and1910 both use absolute clock for the alpha ramp;
                // phase1 transition also compares absolute clock against0.
                let now=if phase==2 {elapsed} else {d.clock as i32};
                d.alpha=trunc(super::super::lerp(from,to,[4.,0.,16.][phase],now as f32)) as u8;
                let age=if phase==1 {d.clock as i32} else {elapsed};
                if age>[4,0,16][phase] {d.phase+=1;d.phase_origin=d.clock;}
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn firebase_stream_render_mode(&self)->u32 {self.firebase_stream.render_mode}

    pub(super) fn start_firebase(&mut self,p: &[u8;128]) {
        let kind=self.profile.kind;
        let simple=kind==PathKind::Firebase2;
        let fourth=kind==PathKind::Firebase4;
        self.firebase.center=[word(p,0),word(p,1),0.];
        self.firebase.size=word(p,4)/100.;self.firebase.peak=trunc(word(p,3));
        self.firebase.counter=0;self.firebase.fade=fourth && trunc(word(p,9))!=0;
        // firebase3 deliberately uses p8 for BOTH duration2 and sticky blend.
        if word(p,if fourth {10} else {8})!=0. {self.firebase.render_mode=1;}
        self.main_count=self.drops.len();self.active_count=if simple {1} else {12};
        let durations=std::array::from_fn(|i|trunc(word(p,i+if simple {5} else {6})));
        for (index,d) in self.drops.iter_mut().enumerate() {
            //1120 clears private storage on every start, but not SDK position.
            d.clock=0;d.phase_origin=0;d.phase=0;d.atlas_frame=0;
            d.motion_origin=[0.,0.,-1000.];d.motion=[0.;3];
            d.path=Trajectory::default();d.durations=durations;
            d.pivot=self.image.map(|n|n*0.5);d.scale=self.firebase.size;
            d.stretch=[1.;2];d.alpha=0;
            let angle=index as f32*f32::from_bits(0x3f06_0a92);
            d.rotation=angle;d.angular_step=word(p,2)/100.;
            if !simple {
                // SDK+18/+1c, float angle first, double trig then speed product.
                let direction=(index%12) as f32*f32::from_bits(0x3f06_0a92);
                d.motion[0]=(f64::from(direction).sin() as f32)*word(p,5);
                d.motion[1]=(f64::from(direction).cos() as f32)*word(p,5);
            }
            firebase_atlas(d,if kind==PathKind::Firebase3 {1} else {index%if simple {3} else {4}},kind);
        }
    }

    pub(super) fn update_firebase(&mut self,tick: u32,offset: [f32;2]) {
        let kind=self.profile.kind;let simple=kind==PathKind::Firebase2;
        if self.firebase.counter>1 {
            self.active_count=self.active_count.saturating_add(if simple {1} else {12}).min(self.main_count);
            self.firebase.counter=0;
        }
        for d in self.drops.iter_mut().take(self.active_count) {
            if kind==PathKind::Firebase3 {
                if d.clock&1==0 {d.atlas_frame+=1;if d.atlas_frame>15 {d.atlas_frame=8;}}
                firebase_atlas(d,d.atlas_frame as usize,kind);
            }
            if d.phase<=2 {
                if !simple {
                    d.motion[0]*=f32::from_bits(0x3f81_47ae);
                    d.motion[1]*=f32::from_bits(0x3f81_47ae);
                }
                d.motion_origin[0]+=d.motion[0];d.motion_origin[1]+=d.motion[1];
                d.position=[self.firebase.center[0]+d.motion_origin[0],
                    self.firebase.center[1]+d.motion_origin[1],self.firebase.center[2]];
                let total=d.durations[0].wrapping_add(d.durations[1]).wrapping_add(d.durations[2]);
                d.scale=super::super::lerp(self.firebase.size,self.firebase.size*2.,total as f32,d.clock as i32 as f32);
                d.rotation+=d.angular_step;
                let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
                if d.phase==0 {
                    d.alpha=trunc(super::super::lerp(0.,self.firebase.peak as f32,d.durations[0] as f32,elapsed as f32)) as u8;
                } else if d.phase==2 && self.firebase.fade {
                    d.alpha=trunc(super::super::lerp(self.firebase.peak as f32,0.,d.durations[2] as f32,elapsed as f32)) as u8;
                }
                if elapsed>d.durations[d.phase as usize] {d.phase+=1;d.phase_origin=d.clock;}
            } else if !simple {
                //3/4 rewrite the center even in terminal phase;2 retains SDK
                //position, so its trailing source offset can accumulate.
                d.position=[self.firebase.center[0]+d.motion_origin[0],
                    self.firebase.center[1]+d.motion_origin[1],self.firebase.center[2]];
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
        // Child count is explicitly zero in1120: no invented trail particles
        // or RNG calls from the unreachable child-emission/secondary-pool code.
        self.firebase.counter=self.firebase.counter.wrapping_add(1);
    }

    pub(super) fn firebase_render_mode(&self)->u32 {self.firebase.render_mode}
}

fn spawn_firebase_stream(d: &mut Drop,image: [f32;2],c: &FirebaseStreamConfig,random: &mut impl ParticleRandomSource) {
    d.clock=0;d.phase=0;d.motion_origin=[sample(random),0.,0.];
    d.pivot=image.map(|n|n*0.5);firebase_stream_uv(d);
    d.scale=c.size;d.stretch=[1.,0.5];d.alpha=0;
    d.rotation=(sample(random)*2.-1.)*0.3;
    // SDK+38 is abs(float), and+74 maps abs(angle)0..0.3 to100..0.
    let lift=super::super::lerp(100.,0.,0.3,d.rotation.abs());
    d.position=[c.center[0],c.center[1]-lift,0.];d.captured=d.position;
}

fn firebase_stream_uv(d: &mut Drop) {
    let [u,v,_]=d.motion_origin;
    d.uv=std::array::from_fn(|i|[u+(i&1) as f32,v+(i>>1) as f32*0.5]);
}

fn firebase_atlas(d: &mut Drop,index: usize,kind: PathKind) {
    //150b0 is a stored table, not repeated addition of .2 (entry3 is .6).
    const FIFTHS:[f32;5]=[0.,0.2,0.4,0.6,0.8];
    let (origin,span)=if kind==PathKind::Firebase3 {
        ([FIFTHS[index%5],FIFTHS[index/5]],0.2)
    } else {([(index&1) as f32*0.5,(index>>1) as f32*0.5],0.5)};
    d.uv=std::array::from_fn(|i|[origin[0]+(i&1) as f32*span,origin[1]+(i>>1) as f32*span]);
}
