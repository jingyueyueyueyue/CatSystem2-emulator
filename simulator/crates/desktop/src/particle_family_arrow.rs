//! s_arrow/wc_s_arrow1090/1150/1790: a moving arrow with a separately
//! timed four-knot child pool. The module emission counter survives starts.
use super::*;
static ARROW_ACTIVE:AtomicU32=AtomicU32::new(1);

#[derive(Debug,Default)]
pub(super) struct ArrowConfig {from:[f32;3],to:[f32;3],size:f32,duration:i32}

impl PathFamily {
    pub(super) fn start_arrow(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;
        self.arrow=ArrowConfig {from:[word(p,1),word(p,2),0.],to:[word(p,3),word(p,4),0.],
            size:word(p,5)/100.,duration:trunc(word(p,6))};
        let direction=normalize_nonzero(sub(self.arrow.to,self.arrow.from));
        let angle=f64::from(direction[1]).acos() as f32;
        let angle=(f64::from(if self.arrow.to[0]>self.arrow.from[0] {-angle} else {angle})
            +f64::from(std::f32::consts::FRAC_PI_2)) as f32;
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            d.uv=std::array::from_fn(|i|[(i&1) as f32,(i>>1) as f32*0.5]);
            d.pivot=self.image.map(|n|n*0.5);d.scale=1.;
            d.stretch=[self.arrow.size*2.,(f64::from(self.arrow.size)*f64::from(0.1f32)) as f32];
            d.position=self.arrow.from;d.rotation=angle;d.alpha=128;
            d.clock=0;d.phase_origin=0;d.phase=0;d.angular_step=0.;
            let first=index*self.child_count;
            for child in &mut children[first..first+self.child_count] {
                let tile=random.next_integer()&3;
                child.uv=std::array::from_fn(|i|[((tile&1)+(i&1) as u32) as f32*0.25,
                    0.5+((tile>>1)+(i>>1) as u32) as f32*0.25]);
                child.pivot=d.pivot;child.vertex_rgb=Some([0x00ff_ffff;4]);
                child.clock=0;child.phase=0;child.phase_origin=0;child.alpha=0;
            }
        }
    }

    pub(super) fn update_arrow(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        let mut active=ARROW_ACTIVE.load(Ordering::Relaxed);
        if random.next_integer()%10==0 {active=active.wrapping_add(1);}
        active=active.min(self.main_count as u32);ARROW_ACTIVE.store(active,Ordering::Relaxed);
        self.active_count=active as usize;
        let c=&self.arrow;let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            if d.phase==0 {
                d.position=std::array::from_fn(|axis|super::super::lerp(c.from[axis],c.to[axis],c.duration as f32,d.clock as i32 as f32));
                if d.clock as i32>c.duration {d.phase=1;d.phase_origin=d.clock;}
            } else if d.phase==1 && d.clock as i32>60 {
                //1200 compares absolute private clock with60, not phase age.
                d.phase=2;d.phase_origin=d.clock;
            }
            let first=index*self.child_count;let trail=&mut children[first..first+self.child_count];
            if d.phase==0 {
                for child in trail.iter_mut().filter(|child|child.phase==0) {
                    child.position=d.position;child.path=Trajectory::default();child.path.append(child.position);
                    let mut point=[(f64::from(sample(random))*60.+f64::from(d.position[0])) as f32,
                        (f64::from(sample(random))*20.+f64::from(d.position[1])) as f32,0.];
                    child.path.append(point);
                    for _ in 0..2 {
                        point[0]=(f64::from(sample(random))*80.+f64::from(point[0])) as f32;
                        point[1]=(f64::from(sample(random))*80.+f64::from(point[1])) as f32;
                        child.path.append(point);
                    }
                    child.path.finish(self.child_count as f32,20.);
                    child.pivot=d.pivot;child.scale=(f64::from(c.size)*f64::from(0.1f32)) as f32;child.stretch=[1.;2];
                    //1521 writes this unused angle to the PARENT private2b8;
                    //it never changes a child SDK rotation. Keep its RNG draw.
                    d.angular_step=(f64::from(sample(random))*f64::from(std::f32::consts::PI)
                        -f64::from(std::f32::consts::FRAC_PI_2)) as f32;
                    child.phase=1;
                    if random.next_integer()%5==0 {break;}
                }
            }
            for child in trail.iter_mut().filter(|child|child.phase!=0) {
                child.position=child.path.at(child.clock);child.scale*=0.99f32;
                child.alpha=trunc(super::super::lerp(255.,0.,self.child_count as f32,child.clock as i32 as f32)) as u8;
                child.clock=child.clock.wrapping_add(tick);
                if child.clock as i32>=self.child_count as i32 {child.clock=0;child.phase=0;child.alpha=0;}
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
        //No SDK completion or respawn: terminal2 retains the real main arrow.
    }
}
