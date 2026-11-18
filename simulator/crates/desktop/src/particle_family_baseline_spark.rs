//! Exact s_080107 identity:11e0/13f0/1700/16f0. Independent Rust model;
//! only PE data was read. The six-knot drift helper is shared with floating.
use super::*;

#[derive(Debug,Default)]
pub(super) struct BaselineSparkConfig {
    baseline:Trajectory,size:f32,step:f32,horizontal:[f32;2],direction:i32,
    vertical:f32,tangent:f32,durations:[i32;3],
}

fn spawn(d:&mut Drop,c:&BaselineSparkConfig,image:[f32;2],random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|n|n*0.5);
    d.uv=[[0.5,0.5],[1.,0.5],[0.5,1.],[1.,1.]];
    d.scale=sample(random)*c.size+c.size;d.stretch=[1.;2];d.alpha=0;
    d.angular_step=(f64::from(sample(random))*f64::from(0.08f32)) as f32;
    if random.next_integer()&1!=0 {d.angular_step = -d.angular_step;}
    d.clock=0;d.phase_origin=0;d.phase=0;
    let baseline=c.baseline.at_time((f64::from(sample(random))*100.) as f32);
    d.position=[(f64::from(sample(random))*200.-100.+f64::from(baseline[0])) as f32,
        (f64::from(sample(random))*100.-50.+f64::from(baseline[1])) as f32,
        (f64::from(sample(random))*50.) as f32];
    d.captured=d.position;
    let duration=c.durations[0].wrapping_add(c.durations[1]).wrapping_add(c.durations[2]);
    d.path=floating::drift_path(d.position,c.horizontal,c.direction,c.vertical,c.step,
        10.,duration as f32,c.tangent,random);
}

impl PathFamily {
    pub(super) fn start_baseline_spark(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let mut baseline=Trajectory::default();
        for i in 0..3 {baseline.append([word(p,i*2),word(p,i*2+1),0.]);}
        let tangent=length(sub(baseline.points[2],baseline.points[0]));
        baseline.finish(100.,tangent);
        self.baseline_spark=BaselineSparkConfig {baseline,size:word(p,6)/100.,step:word(p,7),
            horizontal:[word(p,8)/100.,word(p,9)/100.],direction:trunc(word(p,10)),
            vertical:word(p,11)/100.,tangent:word(p,12),
            durations:std::array::from_fn(|i|trunc(word(p,13+i)))};
        self.main_count=self.drops.len();self.child_count=0;self.active_count=2;
        for d in &mut self.drops {spawn(d,&self.baseline_spark,self.image,random);}
    }

    pub(super) fn update_baseline_spark(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        self.active_count=self.active_count.saturating_add(10).min(self.main_count);
        let c=&self.baseline_spark;
        for d in self.drops.iter_mut().take(self.active_count) {
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            if d.phase<=2 {
                d.position=d.path.at(d.clock);
                match d.phase {
                    0=> {
                        d.alpha=trunc(super::super::lerp(0.,255.,c.durations[0] as f32,d.clock as i32 as f32)) as u8;
                        if d.clock as i32>c.durations[0] {d.phase=1;d.phase_origin=d.clock;}
                    }
                    1=> {
                        d.alpha=(random.next_integer()&255) as u8;
                        //15db compares the absolute clock, not phase age.
                        if d.clock as i32>c.durations[1] {d.phase=2;d.phase_origin=d.clock;}
                    }
                    _=> {
                        d.rotation=(f64::from(d.rotation)+f64::from(0.01f32)) as f32;
                        d.alpha=trunc(super::super::lerp(190.,0.,c.durations[2] as f32,elapsed as f32)) as u8;
                        if elapsed>c.durations[2] {d.phase=3;d.phase_origin=d.clock;}
                    }
                }
                d.rotation+=d.angular_step;
            }
            // Terminal3 retains its transform and never calls spawn again.
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
