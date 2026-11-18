//! s_0401031210/14a0/2380: two real baselines produce a ribbon of
//! parent paths; the existing SDK child pool consumes four-knot trails.
use super::*;

#[derive(Debug,Default)]
pub(super) struct RibbonConfig {baselines:[Trajectory;2],size:f32,durations:[f32;3]}

fn ribbon_point(c:&RibbonConfig,time:f32,index:usize,count:usize)->[f32;3] {
    let a=c.baselines[0].at_time(time);let b=c.baselines[1].at_time(time);
    std::array::from_fn(|axis|a[axis]+((b[axis]-a[axis])/count as f32)*index as f32)
}

fn spawn_ribbon(d:&mut Drop,trail:&mut [Drop],image:[f32;2],c:&RibbonConfig,
    index:usize,count:usize,random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.scale=c.size;d.stretch=[1.;2];
    d.uv=[[0.5,0.],[1.,0.],[0.5,0.5],[1.,0.5]];d.alpha=128;
    d.path=Trajectory::default();
    for time in [0.,50.,100.] {d.path.append(ribbon_point(c,time,index,count));}
    let tangent=d.path.length*0.5;d.path.finish(c.durations[1],tangent);
    //2529 reads the second baseline's scratch result, before the mixed
    //first knot. Phase0 holds here until the public path phase starts.
    d.position=c.baselines[1].at_time(0.);d.clock=0;d.phase=0;d.captured=d.position;
    d.angular_step=(f64::from(sample(random))*f64::from(std::f32::consts::PI)
        -f64::from(std::f32::consts::FRAC_PI_2)) as f32;
    for child in trail {
        child.clock=0;child.phase=0;child.position=d.position;child.pivot=d.pivot;
        child.vertex_rgb=d.vertex_rgb;child.alpha=0;
        child.uv=[[0.5,0.5],[1.,0.5],[0.5,1.],[1.,1.]];
    }
}

fn spawn_ribbon_child(child:&mut Drop,parent:&mut Drop,child_count:usize,
    random:&mut impl ParticleRandomSource) {
    child.position=parent.position;child.rotation=parent.rotation;
    child.path=Trajectory::default();child.path.append(parent.position);
    let mut direction=sub(parent.captured,parent.position);
    if length(direction)<=0.001 {
        direction=sub(parent.path.at(child.clock.wrapping_add(1)),parent.path.at(child.clock));
    }
    direction=normalize_nonzero(direction);
    //Three directions all rotate the original segment. Each distance is
    //relative to the parent, not a cumulative walk through previous knots.
    for (degrees,range) in [(50.,10.),(80.,20.),(90.,20.)] {
        let limit=(f64::from(degrees)*f64::from(std::f32::consts::PI)/180.) as f32;
        let now=(f64::from(sample(random))*100.) as f32;
        let angle=super::super::lerp(-limit,limit,100.,now);
        let (sin,cos)=f64::from(angle).sin_cos();let (sin,cos)=(sin as f32,cos as f32);
        let rotated=normalize_nonzero([direction[0]*cos-direction[1]*sin,
            direction[0]*sin+direction[1]*cos,direction[2]]);
        let distance=(f64::from(sample(random))*f64::from(range)+f64::from(range)) as f32;
        child.path.append(add(parent.position,scale(rotated,distance)));
    }
    let tangent=child.path.length/10.;child.path.finish(child_count as f32,tangent);
    child.pivot=parent.pivot;child.scale=parent.scale*0.4;child.stretch=parent.stretch;
    //211c writes a newly sampled spin to the PARENT private record.
    //The child's own private spin remains its existing value (zero on load).
    parent.angular_step=(f64::from(sample(random))*f64::from(std::f32::consts::PI)
        -f64::from(std::f32::consts::FRAC_PI_2)) as f32;
    child.phase=1;
}

impl PathFamily {
    pub(super) fn start_ribbon(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;self.active_count=self.main_count;
        let c=&mut self.ribbon;
        for (index,path) in c.baselines.iter_mut().enumerate() {
            *path=Trajectory::default();
            for knot in 0..3 {let i=1+index*6+knot*2;path.append([word(p,i),word(p,i+1),0.]);}
            let tangent=path.length*1.5;path.finish(100.,tangent);
        }
        c.size=word(p,13)/100.;c.durations=[word(p,14),word(p,15),word(p,16)];
        //Start stores p17/18; reachable14a0 never consumes either.
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let base=index*self.child_count;
            spawn_ribbon(d,&mut children[base..base+self.child_count],self.image,c,index,self.main_count,random);
        }
    }

    pub(super) fn update_ribbon(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        let c=&self.ribbon;let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            match d.phase {
                0=>{
                    d.alpha=trunc(super::super::lerp(0.,128.,c.durations[0],d.clock as i32 as f32)) as u8;
                    if d.clock as i32 as f32>c.durations[0] {d.phase=1;d.phase_origin=d.clock;}
                }
                1=>{
                    d.position=d.path.at_time(elapsed);d.rotation+=d.angular_step;
                    if elapsed>c.durations[1] {d.phase=2;d.phase_origin=d.clock;}
                }
                2=>{
                    d.rotation+=d.angular_step;
                    d.alpha=trunc(super::super::lerp(128.,0.,c.durations[2],elapsed)) as u8;
                    if elapsed>c.durations[2] {d.phase=3;d.phase_origin=d.clock;d.alpha=0;}
                }
                _=>{}
            }
            let base=index*self.child_count;let trail=&mut children[base..base+self.child_count];
            if d.phase==1 {
                if let Some(child)=trail.iter_mut().find(|child|child.phase==0) {
                    spawn_ribbon_child(child,d,self.child_count,random);
                }
            }
            for child in trail.iter_mut().filter(|child|child.phase!=0) {
                child.scale=(f64::from(child.scale)*f64::from(0.999f32)) as f32;
                child.position=child.path.at(child.clock);child.rotation+=child.angular_step;
                child.alpha=trunc(super::super::lerp(90.,0.,self.child_count as f32,child.clock as i32 as f32)) as u8;
                child.clock=child.clock.wrapping_add(tick);
                if child.clock as i32>=self.child_count as i32 {child.clock=0;child.phase=0;child.alpha=0;}
            }
            d.clock=d.clock.wrapping_add(tick);d.captured=d.position;
        }
    }
}
