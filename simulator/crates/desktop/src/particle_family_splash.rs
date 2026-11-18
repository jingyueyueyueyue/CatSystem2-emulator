//! s_020402/splash11e0/13b0/16c0: one-shot branching stream.
//! Fixed producer knots and the three different alpha/time branches are PE data.
use super::*;

#[derive(Debug,Default)]
pub(super) struct SplashConfig {
    baseline: Trajectory,
    size: f32,
    segment_length: f32,
    horizontal: [f32;2],
    vertical: f32,
    direction: i32,
    tangent: f32,
    durations: [i32;3],
}

impl PathFamily {
    pub(super) fn start_splash(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        // p0/1 are stored by11e0 but neither13b0 nor16c0 reads them.
        self.splash.size=word(p,2)/100.;self.splash.segment_length=word(p,3);
        self.splash.horizontal=[word(p,4)/100.,word(p,5)/100.];
        self.splash.direction=trunc(word(p,6));self.splash.vertical=word(p,7)/100.;
        self.splash.tangent=word(p,8);
        self.splash.durations=std::array::from_fn(|i|trunc(word(p,i+9)));
        let path=&mut self.splash.baseline;*path=Trajectory::default();
        for point in [[0.,280.,0.],[270.,470.,0.],[940.,550.,0.]] {path.append(point);}
        path.finish(100.,200.);
        self.main_count=self.drops.len();self.active_count=2.min(self.main_count);
        for d in &mut self.drops {spawn_splash(d,self.image,&self.splash,random);}
    }

    pub(super) fn update_splash(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        self.active_count=self.active_count.saturating_add(10).min(self.main_count);
        for d in self.drops.iter_mut().take(self.active_count) {
            let clock=d.clock as i32;let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            match d.phase {
                0=>{
                    d.position=d.path.at(d.clock);
                    d.alpha=trunc(super::super::lerp(0.,255.,self.splash.durations[0] as f32,clock as f32)) as u8;
                    d.rotation+=d.angular_step;
                    if clock>self.splash.durations[0] {d.phase=1;d.phase_origin=d.clock;}
                }
                1=>{
                    d.position=d.path.at(d.clock);d.alpha=(random.next_integer()&255) as u8;
                    d.rotation+=d.angular_step;
                    // 159b compares absolute clock, not time since phase1.
                    if clock>self.splash.durations[1] {d.phase=2;d.phase_origin=d.clock;}
                }
                2=>{
                    d.position=d.path.at(d.clock);d.rotation+=0.01;
                    d.alpha=trunc(super::super::lerp(190.,0.,self.splash.durations[2] as f32,elapsed as f32)) as u8;
                    d.rotation+=d.angular_step;
                    if elapsed>self.splash.durations[2] {d.phase=3;d.phase_origin=d.clock;}
                }
                _=>{} // One shot:13b0 has no respawn branch.
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn spawn_splash(d: &mut Drop,image: [f32;2],c: &SplashConfig,random: &mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);
    d.uv=std::array::from_fn(|i|[0.5+(i&1) as f32*0.5,0.5+(i>>1) as f32*0.5]);
    d.scale=sample(random)*c.size+c.size;d.stretch=[1.;2];d.alpha=0;
    d.angular_step=sample(random)*0.08;
    if random.next_integer()&1!=0 {d.angular_step*=-1.;}
    d.phase=0;d.clock=0;d.phase_origin=0;
    let base=c.baseline.at_time(sample(random)*100.);
    d.position=[sample(random)*200.-100.+base[0],sample(random)*100.-50.+base[1],sample(random)*50.];
    d.path=Trajectory::default();d.path.append(d.position);
    let x=sample(random)*c.horizontal[1]-c.horizontal[0];
    let y=match c.direction {
        1=>-(sample(random)*c.vertical+0.05),
        2=>sample(random)*c.vertical-c.vertical*0.5,
        _=>sample(random)*c.vertical+0.05,
    };
    let direction=normalize_nonzero([x,y,0.]);
    let mut current=add(d.position,scale(direction,sample(random)*c.segment_length+c.segment_length));
    d.path.append(current);d.motion=normalize_nonzero(sub(current,d.position));
    //16c0 builds at most four further points, with a MT%15 early exit.
    // These rotations are +/-10 degrees through the SDK's .001-radian input.
    let radians=trunc((10.*std::f32::consts::PI/180.)*1000.) as f32/1000.;
    let (sin,cos)=f64::from(radians).sin_cos();let (sin,cos)=(sin as f32,cos as f32);
    let [x,y,_]=d.motion;
    let left=[cos*x+sin*y,cos*y-sin*x];let right=[cos*x-sin*y,cos*y+sin*x];
    let width=[(right[0]-left[0]).abs(),(right[1]-left[1]).abs()];
    for _ in 0..4 {
        let jitter=[sample(random)*width[0]-width[0]*0.5+x,
            sample(random)*width[1]-width[1]*0.5+y,0.];
        let next=normalize_nonzero(jitter);
        current=add(current,scale(next,sample(random)*c.segment_length+c.segment_length));
        d.path.append(current);
        if random.next_integer()%15==0 {break;}
    }
    let duration=c.durations.iter().fold(0i32,|n,v|n.wrapping_add(*v));
    d.path.finish(duration as f32,c.tangent);d.captured=d.position;
}
