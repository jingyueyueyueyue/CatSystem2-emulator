//! kemuridama10b0/1160/1630: three emitter groups, real four-knot
//! smoke trajectories and FLOAT private clocks (including delta/10).
use super::*;

#[derive(Debug,Default,Clone,Copy)]
struct Clock {now:f32,origin:f32}
#[derive(Debug,Default)]
pub(super) struct SmokeConfig {clocks:Vec<Clock>}

std::thread_local! {static SMOKE_CRT:std::cell::Cell<u32>=const {std::cell::Cell::new(1)};}
fn crt_random()->u32 {
    //2c8f has its own statically linked CRT per-thread seed; no SDK MT draw.
    SMOKE_CRT.with(|seed|{let next=seed.get().wrapping_mul(214013).wrapping_add(2531011);
        seed.set(next);(next>>16)&0x7fff})
}

impl PathFamily {
    pub(super) fn start_smoke(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=self.drops.len()/3;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition};
        self.child_count=partition.saturating_sub(1);self.active_count=self.main_count;
        if 1.>word(p,3) {self.stream_render_mode=1;}
        self.smoke.clocks=vec![Clock::default();self.drops.len()];
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        let (parent_clocks,child_clocks)=self.smoke.clocks.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let range=index*self.child_count..(index+1)*self.child_count;
            spawn(d,&mut children[range.clone()],&mut parent_clocks[index],&mut child_clocks[range],index,self.image,p,random);
        }
    }

    pub(super) fn update_smoke(&mut self,tick:u32,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        let (parent_clocks,child_clocks)=self.smoke.clocks.split_at_mut(self.main_count);
        for (index,(d,time)) in parents.iter_mut().zip(parent_clocks).enumerate() {
            let range=index*self.child_count..(index+1)*self.child_count;
            let trail=&mut children[range.clone()];let clocks=&mut child_clocks[range];
            match d.phase {
                //1160 really reuses header18 (p1) for this threshold,
                //although1630 also uses p1 as X. Do not invent a new slot.
                0=>if time.now>trunc((word(p,1)*f32::from_bits(0x3f66_6666)+0.5).floor()) as f32 {
                    d.phase=1;time.origin=time.now;
                },
                1=>if time.now>word(p,1) {d.phase=2;time.origin=time.now;},
                _=>spawn(d,trail,time,clocks,index,self.image,p,random),
            }
            if let Some(child)=trail.iter_mut().find(|c|c.phase==0) {child.phase=1;}
            for (child,clock) in trail.iter_mut().zip(clocks.iter_mut()) {
                if child.phase==0 {continue;}
                match child.phase {
                    1=>{
                        child.position=child.path.at_time(clock.now);clock.now+=tick as f32;
                        if clock.now>(f64::from(child.path.duration)*f64::from(0.01f32)) as f32 {
                            child.phase=2;clock.origin=time.now;
                        }
                    },
                    2=>{
                        child.position=child.path.at_time(clock.now);child.rotation+=child.angular_step;
                        child.alpha=alpha(0.,255.,clock.origin,(f64::from(child.path.duration)*f64::from(0.6f32)) as f32,clock.now);
                        clock.now+=tick as f32;
                        if clock.now>(f64::from(child.path.duration)*f64::from(0.6f32)) as f32 {
                            child.phase=3;clock.origin=time.now;
                        }
                    },
                    3=>{
                        child.position=child.path.at_time(clock.now);child.rotation+=child.angular_step;
                        child.scale*=f32::from_bits(0x3f83_d70a);
                        //14fe's esp3c is the saved delta under the14-byte
                        //argument block, not a path coordinate or frame count.
                        clock.now=(f64::from(clock.now)+f64::from(tick)/10.) as f32;
                        child.alpha=alpha(255.,0.,clock.origin,child.path.duration,clock.now);
                        if clock.now>child.path.duration {child.phase=5;clock.origin=clock.now;}
                    },
                    4=>{
                        //Retain the recovered table entry; no public producer
                        //of phase4 was found in this SHA's start/update chain.
                        child.position[1]=(f64::from(child.position[1])-f64::from(0.7f32)) as f32;
                        let value=super::super::lerp(255.,0.,60.,clock.now-clock.origin);
                        child.alpha=trunc(value) as u8;clock.now+=tick as f32;
                        if trunc(value)==0 {child.phase=5;clock.origin=clock.now;}
                    },
                    _=>{},
                }
                child.rotation=(f64::from(child.rotation)+f64::from(0.05f32)) as f32;
            }
            time.now+=tick as f32;
        }
    }
}

fn alpha(from:f32,to:f32,start:f32,end:f32,now:f32)->u8 {
    trunc(super::super::lerp(from,to,end-start,now-start)) as u8
}

fn spawn(parent:&mut Drop,trail:&mut [Drop],clock:&mut Clock,clocks:&mut [Clock],index:usize,
    image:[f32;2],p:&[u8;128],random:&mut impl ParticleRandomSource) {
    let center=[word(p,1),word(p,2),0.];parent.position=[center[0],center[1]-350.,0.];
    parent.alpha=0;parent.flag=0;parent.phase=0;clock.now=0.;
    for (d,time) in trail.iter_mut().zip(clocks) {
        time.now=0.;d.phase=0;
        let sign=if random.next_integer()&1!=0 {1.} else {-1.};
        d.angular_step=((f64::from(sample(random))*f64::from(0.1f32)+f64::from(0.05f32))*sign) as f32;
        d.position=parent.position;d.pivot=image.map(|v|v*0.5);d.alpha=0;
        d.scale=(f64::from(sample(random))*0.5+0.5) as f32;d.stretch=[1.;2];
        d.path=Trajectory::default();d.path.append(d.position);
        let second=[d.position[0]+(crt_random()%50) as f32-25.,d.position[1]+350.,0.];d.path.append(second);
        let third=match index {
            0=>[center[0]+(crt_random()%150) as f32,center[1]-(crt_random()%250) as f32,0.],
            1=>[center[0]-(crt_random()%150) as f32,center[1]-(crt_random()%250) as f32,0.],
            _=>[center[0]+(crt_random()%100) as f32-50.,center[1]-250.,0.],
        };
        d.path.append(third);
        let mut direction=sub(third,second);direction[1]-=50.;direction=normalize_nonzero(direction);
        direction=scale(direction,(random.next_integer()%100+50) as f32);
        let a=rotate_degrees(direction,-40.);let b=rotate_degrees(direction,40.);
        let width=f64::from(b[0]-a[0]).powf(2.).sqrt() as f32;
        let height=f64::from(b[1]-a[1]).powf(2.).sqrt() as f32;
        let x=(f64::from(sample(random))*f64::from(width)-f64::from(width)*0.5) as f32;
        let y=(f64::from(sample(random))*f64::from(height)-f64::from(height)*0.5) as f32;
        direction=normalize_nonzero([direction[0]+x,direction[1]+y,0.]);
        let distance=((random.next_integer()%500) as f64/10.+50.) as f32;
        d.path.append(add(third,scale(direction,distance)));d.path.finish(word(p,0),50.);
    }
}
