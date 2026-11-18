//! fall_spark10b0/1230/1930: grouped ten-knot falling path and a real,
//! reusable six-call afterimage pool. No source-position translation is read.
use super::*;

std::thread_local! {static FALL_CRT:std::cell::Cell<u32>=const {std::cell::Cell::new(1)};}
fn first_step()->f32 {
    FALL_CRT.with(|state| {
        let next=state.get().wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        state.set(next);(((next>>16)&0x7fff)%500) as f32/10.+50.
    })
}

#[derive(Debug,Default)]
pub(super) struct FallSparkConfig {duration:i32,size:f32,period:u32,batch:u32,restart:bool}

fn spawn(d:&mut Drop,c:&FallSparkConfig,image:[f32;2],random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|n|n*0.5);
    d.scale=(f64::from(sample(random))*f64::from(0.01f32)+f64::from(c.size)) as f32;
    d.stretch=[1.;2];let _unused=random.next_integer();
    d.vertex_rgb=Some(std::array::from_fn(|_|0x00ff_000a|((random.next_integer()%50)<<8)));
    d.alpha=0;d.position=[(f64::from(sample(random))*1280.) as f32,-50.,
        (f64::from(sample(random))*100.) as f32];
    d.rotation_xy[1]=(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32;
    d.clock=0;d.phase=0;d.captured=d.position;d.path=Trajectory::default();d.path.append(d.position);
    let direction=normalize_nonzero([
        (f64::from(sample(random))*f64::from(0.4f32)-f64::from(0.2f32)) as f32,
        (f64::from(sample(random))*f64::from(0.8f32)+f64::from(0.2f32)) as f32,0.]);
    let mut current=add(d.position,scale(direction,first_step()));d.path.append(current);
    let initial=normalize_nonzero(sub(current,d.position));d.motion=initial;
    let radians=(15.*f64::from(std::f32::consts::PI)/180.) as f32;
    let (sn,cs)=f64::from(radians).sin_cos();let (sn,cs)=(sn as f32,cs as f32);
    let left=[cs*initial[0]+sn*initial[1],cs*initial[1]-sn*initial[0]];
    let right=[cs*initial[0]-sn*initial[1],cs*initial[1]+sn*initial[0]];
    let width=[(right[0]-left[0]).abs(),(right[1]-left[1]).abs()];
    for _ in 0..8 {
        let jitter=std::array::from_fn(|axis|if axis<2 {
            let delta=(f64::from(sample(random))*f64::from(width[axis])-f64::from(width[axis])*0.5) as f32;
            initial[axis]+delta
        } else {0.});
        let direction=normalize_nonzero(jitter);
        let step=(random.next_integer()%500) as f32/10.+50.;
        current=add(current,scale(direction,step));d.path.append(current);
    }
    //2056 consumes chord sum /7, rather than averaging the nine segments.
    d.path.finish(c.duration as f32,(f64::from(d.path.length)/7.) as f32);
}

fn reset_trail(d:&Drop,trail:&mut [Drop]) {
    for c in trail {
        c.clock=0;c.phase=0;c.phase_origin=0;
        c.position=[d.position[0]-40.,d.position[1]-40.,d.position[2]-5.];
        c.vertex_rgb=d.vertex_rgb;c.alpha=0;
    }
}

impl PathFamily {
    pub(super) fn start_fall_spark(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;
        self.fall_spark=FallSparkConfig {duration:trunc(word(p,1)),size:word(p,2)/100.,
            period:61i32.wrapping_sub(trunc(word(p,3))).clamp(1,60) as u32,
            batch:trunc(word(p,4)) as u32,restart:trunc(word(p,6))!=0};
        let prewarm=trunc(word(p,5))!=0;
        self.active_count=if prewarm {self.main_count/2} else {0};
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            d.phase_origin=0;spawn(d,&self.fall_spark,self.image,random);
            let first=index*self.child_count;let trail=&mut children[first..first+self.child_count];
            for c in trail.iter_mut() {
                c.uv=std::array::from_fn(|i|[0.5+(i&1) as f32*0.5,0.5+(i>>1) as f32*0.5]);
            }
            reset_trail(d,trail);
            if prewarm && index<self.active_count {
                d.clock=trunc((f64::from(sample(random))*f64::from(self.fall_spark.duration)) as f32) as u32;
            }
        }
    }

    pub(super) fn update_fall_spark(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        let c=&self.fall_spark;
        if random.next_integer()%c.period==0 {self.active_count=(self.active_count as u32).wrapping_add(c.batch) as usize;}
        self.active_count=self.active_count.min(self.main_count);
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let first=index*self.child_count;let trail=&mut children[first..first+self.child_count];
            let now=d.clock as i32;let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            match d.phase {
                0|1=> {
                    let point=d.path.at(d.clock);d.position[..2].copy_from_slice(&point[..2]);
                    d.alpha=if d.phase==0 {190} else {
                        //SDK74 takes start/end CLOCKS, not a duration interval.
                        trunc(super::super::lerp(190.,0.,c.duration as f32-d.phase_origin as i32 as f32,elapsed as f32)) as u8
                    };
                    d.rotation_xy[1]=(f64::from(sample(random))/10.+f64::from(d.rotation_xy[1])) as f32;
                    let threshold=if d.phase==0 {((f64::from(c.duration)*f64::from(0.9f32)) as f32+0.5).floor() as i32} else {c.duration};
                    if now>threshold {d.phase=if d.phase==0 {1} else {10};d.phase_origin=d.clock;}
                }
                2|3|4=> {
                    let duration=match d.phase {2=>5.,3=>10.,_=>20.};
                    let (from,to)=if d.phase==4 {(255.,0.)} else {(0.,255.)};
                    d.alpha=trunc(super::super::lerp(from,to,duration,elapsed as f32)) as u8;
                    if d.phase!=4 {d.position[1]=(f64::from(sample(random))+f64::from(d.position[1])) as f32;}
                    if elapsed as f32>duration {
                        let jitter=d.phase==2;d.phase+=1;d.phase_origin=d.clock;
                        if jitter {
                            d.position[0]=(f64::from(sample(random))*10.-5.+f64::from(d.position[0])) as f32;
                            d.position[1]=(f64::from(sample(random))*10.-5.+f64::from(d.position[1])) as f32;
                        }
                    }
                }
                _=>if c.restart {spawn(d,c,self.image,random);reset_trail(d,trail);},
            }
            if d.phase==0 {
                if let Some(child)=trail.iter_mut().find(|child|child.phase==0) {
                    child.position=d.position;child.rotation=d.rotation;child.rotation_xy=d.rotation_xy;
                    child.scale=d.scale;child.stretch=d.stretch;child.pivot=d.pivot;
                    child.phase=1;
                }
            }
            for child in trail.iter_mut().filter(|child|child.phase!=0) {
                child.position[1]=(f64::from(sample(random))*0.5+f64::from(child.position[1])) as f32;
                child.scale*=0.99f32;
                child.alpha=trunc(super::super::lerp(100.,0.,30.,child.clock as i32 as f32)) as u8;
                child.clock=child.clock.wrapping_add(1);
                if child.clock as i32>5 {child.clock=0;child.phase=0;child.alpha=0;}
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
