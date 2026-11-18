//! rubble11b0/1320/1530: real baseline-distributed fragments and
//! one-shot curved flight. Phase0 exits to3 directly in the fixed PE.
use super::*;

#[derive(Debug,Default)]
pub(super) struct RubbleConfig {emission: u32}

impl PathFamily {
    pub(super) fn start_rubble(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let mut baseline=Trajectory::default();
        baseline.append(std::array::from_fn(|axis|word(p,axis)));
        baseline.append(std::array::from_fn(|axis|word(p,axis+3)));
        baseline.finish(100.,1.);
        self.rubble.emission=trunc(word(p,8)) as u32;
        self.main_count=self.drops.len();self.active_count=0;
        for (index,d) in self.drops.iter_mut().enumerate() {
            d.clock=0;d.phase=0;d.phase_origin=0;
            let atlas=random.next_integer()&3;
            let origin=[(atlas&1) as f32*0.5,(atlas>>1) as f32*0.5];
            d.uv=std::array::from_fn(|i|[origin[0]+(i&1) as f32*0.5,origin[1]+(i>>1) as f32*0.5]);
            d.pivot=self.image.map(|v|v*0.5);
            let size=(f64::from(word(p,6))/100.) as f32;
            let half=f64::from(size)*0.5;
            d.scale=(half*f64::from(sample(random))+half) as f32;
            d.stretch=[1.;2];d.alpha=0;d.rotation=0.;d.rotation_xy=[0.;2];
            let time=(100./self.main_count as f64*index as f64) as f32;
            d.position=baseline.at_time(time);
            let sign=if random.next_integer()&1!=0 {1.} else {-1.};
            d.angular_step=((f64::from(sample(random))*f64::from(0.02f32)+f64::from(0.02f32))*sign) as f32;
            d.path=Trajectory::default();d.path.append(d.position);
            let x=(f64::from(sample(random))*200.+100.+f64::from(d.position[0])) as f32;
            let y=(f64::from(d.position[1])-(f64::from(sample(random))*600.+600.)) as f32;
            let sign=if random.next_integer()&1!=0 {1.} else {-1.};
            let z=((f64::from(sample(random))*50.+50.)*sign+f64::from(d.position[2])) as f32;
            d.path.append([x,y,z]);
            let half=f64::from(word(p,7))*0.5;
            let duration=(half*f64::from(sample(random))+half) as f32;
            d.motion_limit=duration;
            d.path.finish(duration,(f64::from(duration)*f64::from(0.55f32)) as f32);
            d.captured=d.position;
        }
    }

    pub(super) fn update_rubble(&mut self,tick: u32) {
        self.active_count=(self.active_count as u32).wrapping_add(self.rubble.emission).min(self.main_count as u32) as usize;
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase==0 {
                d.position=d.path.at(d.clock);d.rotation+=d.angular_step;
                let duration=(f64::from(d.motion_limit)/10.) as f32;
                d.alpha=trunc(super::super::lerp(0.,255.,duration,d.clock as i32 as f32)) as u8;
                if d.clock as i32 as f32>d.motion_limit {d.phase=3;d.phase_origin=d.clock;}
            } else if d.phase==1 {
                // Present in1320 but no public phase0 transition reaches1.
                d.position=d.path.at(d.clock);
                if d.clock as i32 as f32>d.motion_limit {d.phase=2;d.phase_origin=d.clock;d.alpha=0;}
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
