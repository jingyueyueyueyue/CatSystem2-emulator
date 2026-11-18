//! radiate/wc_radiate20e0/21b0/27c0: two-knot emitters and segment pools.
//! Exact PE734ab4e40b; helper1120 partitions the existing SDK quad grid.
use super::*;

#[derive(Debug,Default)]
pub(super) struct RadiateConfig {center:[f32;3],duration:f32,size:[f32;2],distance:f32}

fn spawn_radiate(d:&mut Drop,pool:&mut [Drop],c:&RadiateConfig,image:[f32;2],random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.scale=c.size[0];d.stretch=[1.;2];d.alpha=0;
    d.position=[(sample(random)*2.-1.)*100.+c.center[0],(sample(random)*2.-1.)*100.+c.center[1],0.];
    d.clock=0;d.phase=0;d.captured=d.position;
    let vector=sub(d.position,c.center);
    let distance=(f64::from(vector[0]*vector[0]+vector[1]*vector[1])).sqrt() as f32;
    let direction=if distance>0. {[vector[0]/distance,vector[1]/distance,0.]} else {vector};
    d.path=Trajectory::default();d.path.append(d.position);
    d.path.append(add(c.center,scale(direction,c.distance)));
    let random_part=c.duration*0.7;let fixed_part=c.duration*0.3;
    d.motion_limit=(f64::from(sample(random))*f64::from(random_part)+f64::from(fixed_part)) as f32;
    d.path.finish(d.motion_limit,c.distance);
    for child in pool {
        child.clock=0;child.phase=0;child.position=d.position;child.alpha=0;
    }
}

impl PathFamily {
    pub(super) fn start_radiate(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let divisor=trunc(word(p,0)) as u32;
        self.main_count=if divisor==0 {self.drops.len()} else {self.drops.len()/divisor as usize};
        self.child_count=divisor.saturating_sub(1) as usize;self.active_count=0;
        self.radiate=RadiateConfig {center:[word(p,2),word(p,3),0.],duration:word(p,1),
            size:[word(p,4)/1000.,word(p,5)/1000.],distance:word(p,6)};
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            d.phase_origin=0;
            let first=index*self.child_count;
            spawn_radiate(d,&mut children[first..first+self.child_count],&self.radiate,self.image,random);
        }
    }

    pub(super) fn update_radiate(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        self.active_count=self.active_count.wrapping_add(1).min(self.main_count);
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate().take(self.active_count) {
            let first=index*self.child_count;let pool=&mut children[first..first+self.child_count];
            if d.phase==0 {
                d.position=d.path.at(d.clock);
                d.scale=super::super::lerp(self.radiate.size[0],self.radiate.size[1],d.motion_limit,d.clock as i32 as f32);
                d.stretch=[1.;2];
                if (d.clock as i32 as f32)>d.motion_limit {d.phase=2;d.phase_origin=d.clock;}
            } else if d.phase==1 {
                d.position=d.path.at(d.clock);
                if (d.clock as i32 as f32)>self.radiate.duration {d.phase=2;d.phase_origin=d.clock;}
            } else {spawn_radiate(d,pool,&self.radiate,self.image,random);}
            if d.phase==0 {
                if let Some(child)=pool.iter_mut().find(|c|c.phase==0) {
                    //2398..262f shares the recovered segment orientation,
                    //midpoint, copied pivot/scale and distance*.7 stretch.
                    submit_segment(d,child);
                }
            }
            advance_segment_trail(pool,tick);
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
