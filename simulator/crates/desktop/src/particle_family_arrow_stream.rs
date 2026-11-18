//! wc_arrow2370/2890/3190/3540: baseline launches into depth, not
//! Hermite trails.1120/1171 explicitly assigns every parent zero children.
use super::*;
#[derive(Debug,Default)]
pub(super) struct StreamArrowConfig {
    speed:f32,speed_range:f32,size:f32,size_range:f32,alpha:f32,alpha_range:f32,
    durations:[i32;3],baseline:Trajectory,divisor:u32,step:usize,align:bool,
    //2370 stores these; neither public spawn nor update consumes them.
    _angle:[f32;2],
}
fn spawn_stream_arrow(d:&mut Drop,c:&StreamArrowConfig,image:[f32;2],random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.scale=sample(random)*c.size_range-c.size_range*0.5+c.size;
    d.alpha=255;let from=c.baseline.at_time(sample(random)*100.);
    d.motion_origin=[from[0],from[1],1000.];d.position=d.motion_origin;
    let target=[640.,-360.-sample(random)*360.,-1000.];
    let speed=sample(random)*c.speed_range+c.speed_range*0.5+c.speed;
    d.motion=scale(normalize_nonzero(sub(target,d.motion_origin)),speed);
    d.saved_alpha=sample(random)*c.alpha_range-c.alpha_range*0.5+c.alpha;
    d.clock=0;d.phase=0;d.captured=d.position;
    //3190 does not reset the retained relative phase origin or rotation.
}
impl PathFamily {
    pub(super) fn start_stream_arrow(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let mut baseline=Trajectory::default();baseline.append([word(p,11),word(p,12),0.]);
        baseline.append([word(p,13),word(p,14),0.]);baseline.finish(100.,1.);
        self.stream_arrow=StreamArrowConfig {speed:word(p,0).max(10.),speed_range:word(p,1).max(0.),
            size:(word(p,4)/100.).max(0.01),size_range:(word(p,5)/100.).max(0.),
            alpha:word(p,6).max(0.),alpha_range:word(p,7).max(0.),
            durations:std::array::from_fn(|i|trunc(word(p,8+i)).max(0)),baseline,
            divisor:61i32.wrapping_sub(trunc(word(p,15))).clamp(1,60) as u32,
            step:trunc(word(p,16)).max(1) as usize,align:trunc(word(p,19))!=0,
            _angle:[word(p,2)/100.,word(p,3)/100.].map(|v|v.clamp(-std::f32::consts::PI,std::f32::consts::PI))};
        self.main_count=self.drops.len();self.child_count=0;
        let warm=trunc(word(p,17))!=0;self.active_count=if warm {self.main_count/2} else {1};
        if trunc(word(p,18))!=0 {self.stream_render_mode=1;}
        let c=&self.stream_arrow;
        for (index,d) in self.drops.iter_mut().enumerate() {
            spawn_stream_arrow(d,c,self.image,random);
            if warm && index<self.active_count {
                d.clock=trunc(sample(random)*c.durations.iter().fold(0i32,|a,b|a.wrapping_add(*b)) as f32) as u32;
                if d.clock as i32>c.durations[0] {d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(c.durations[0] as u32);d.alpha=128;}
                //3540's second comparison uses phase1 alone, not its sum.
                if d.clock as i32>c.durations[1] {d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(c.durations[1] as u32);}
            }
        }
    }
    pub(super) fn update_stream_arrow(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        let c=&self.stream_arrow;
        if random.next_integer()%c.divisor==0 {self.active_count=self.active_count.wrapping_add(c.step).min(self.main_count);}
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase>=3 {spawn_stream_arrow(d,c,self.image,random);}
            else {
                let age=d.clock.wrapping_sub(d.phase_origin) as i32;
                if d.phase==1 {d.alpha=trunc(d.saved_alpha) as u8;}
                else if d.phase==2 {d.alpha=trunc(super::super::lerp(d.saved_alpha,0.,c.durations[2] as f32,age as f32)) as u8;}
                d.motion_origin=add(d.motion_origin,d.motion);d.position=d.motion_origin;
                if c.align {
                    let direction=normalize_nonzero(sub(d.position,d.captured));
                    let angle=f64::from(-direction[1]).acos() as f32;
                    d.rotation=(if -direction[0]>0. {-angle} else {angle})-std::f32::consts::FRAC_PI_2;
                }
                let next=match d.phase {0=>d.clock as i32>=c.durations[0],1=>age>=c.durations[1],_=>age>c.durations[2]};
                if next {d.phase+=1;d.phase_origin=d.clock;}
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
