//! a_wind11e0/1380/1950: transparent path parents emit real stretched
//! children;2320 partitions the existing SDK grid, not an extra allocation.
use super::*;

#[derive(Debug,Default)]
pub(super) struct WindTrailConfig {
    baseline:Trajectory,size:f32,distance:f32,durations:[i32;3],tangent:f32,
    horizontal:[f32;2],vertical:f32,direction:i32,
}

impl PathFamily {
    pub(super) fn start_wind_trail(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;self.active_count=1;
        let c=&mut self.wind_trail;
        c.baseline=Trajectory::default();c.baseline.append([word(p,1),word(p,2),0.]);
        c.baseline.append([word(p,3),word(p,4),0.]);c.baseline.finish(100.,1.);
        c.size=word(p,5)/100.;c.distance=word(p,6);
        c.durations=[trunc(word(p,7)),trunc(word(p,8)),trunc(word(p,9))];
        c.tangent=word(p,10);c.horizontal=[word(p,11)/100.,word(p,12)/100.];
        c.direction=trunc(word(p,13));c.vertical=word(p,14)/100.;
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (i,d) in parents.iter_mut().enumerate() {
            let first=i*self.child_count;
            spawn_wind_trail(d,&mut children[first..first+self.child_count],self.image,c,random);
        }
    }

    pub(super) fn update_wind_trail(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        if random.next_integer()%10==0 {self.active_count+=1;}
        self.active_count=self.active_count.min(self.main_count);
        let c=&self.wind_trail;let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (i,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let first=i*self.child_count;let trail=&mut children[first..first+self.child_count];
            if d.phase==0 {
                d.position=d.path.at(d.clock);
                if d.atlas_frame==0 && random.next_integer()%40==0 {d.atlas_frame=1;}
                if d.path.duration<=d.clock as i32 as f32 {d.phase=1;d.phase_origin=d.clock;}
            } else if d.clock as i32 as f32>=d.path.duration+60. {
                spawn_wind_trail(d,trail,self.image,c,random);
            }
            if d.atlas_frame==1 {
                if let Some(index)=trail.iter().position(|child|child.phase==0) {
                    if index+1==trail.len() {d.atlas_frame=2;}
                    let child=&mut trail[index];let difference=sub(d.position,d.captured);
                    let distance=length(difference);
                    child.pivot=d.pivot;child.scale=d.scale;child.stretch=d.stretch;
                    if distance!=0. {
                        let normal=normalize_nonzero(difference);
                        let angle=f64::from(normal[1]).acos() as f32;
                        child.rotation=if normal[0]>0. {-angle} else {angle};
                        child.position=scale(add(d.position,d.captured),0.5);
                        child.stretch[1]=(f64::from(distance)*f64::from(0.7f32)*f64::from(child.stretch[1])) as f32;
                    } else {child.position=d.position;}
                    child.phase=1;
                }
            }
            for (index,child) in trail.iter_mut().enumerate() {
                if child.phase!=0 {
                    //SDK74 receives255,0,30,60,clock: thirty ticks hold,
                    //then a thirty-tick fade; neither period uses frame time.
                    child.alpha=trunc(super::super::lerp(255.,0.,30.,child.clock as i32 as f32-30.)) as u8;
                    child.clock=child.clock.wrapping_add(tick);
                    if child.clock as i32>=60 {
                        child.clock=0;child.phase=0;child.alpha=0;if index==0 {d.atlas_frame=0;}
                    }
                }
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
        //1380 never writes parent alpha or source translation.
    }
}

fn spawn_wind_trail(d:&mut Drop,trail:&mut [Drop],image:[f32;2],c:&WindTrailConfig,
    random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.scale=(f64::from(sample(random))*f64::from(c.size)+f64::from(c.size)) as f32;
    d.stretch=[1.;2];d.vertex_rgb=Some([0x00ff_0000;4]);d.alpha=0;
    let mut point=c.baseline.at_time((f64::from(sample(random))*100.) as f32);
    d.position=point;d.rotation=(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32;
    let initial=c.durations[0].wrapping_add(c.durations[1]);
    d.clock=trunc((f64::from(sample(random))*f64::from(initial)) as f32) as u32;
    d.phase=0;d.captured=point;d.atlas_frame=0;d.path=Trajectory::default();d.path.append(point);
    let x=(f64::from(sample(random))*f64::from(c.horizontal[1])-f64::from(c.horizontal[0])) as f32;
    let s=sample(random);
    let y=match c.direction {
        1=>-((f64::from(s)*f64::from(c.vertical)+f64::from(0.05f32)) as f32),
        2=>(f64::from(s)*f64::from(c.vertical)-f64::from(c.vertical*0.5)) as f32,
        _=>(f64::from(s)*f64::from(c.vertical)+f64::from(0.05f32)) as f32,
    };
    let mut direction=normalize_nonzero([x,y,0.]);
    let speed=(f64::from(sample(random))*f64::from(c.distance)+f64::from(c.distance)) as f32;
    let next=add(point,scale(direction,speed));d.motion=normalize_nonzero(sub(next,point));
    point=next;d.path.append(point);
    for _ in 0..4 {
        //1950 computes separate cos/sin cone spans at-50/+50degrees,
        //samples both even if zero, normalizes, then draws a new distance.
        let low=rotate_degrees(d.motion,-50.);let high=rotate_degrees(d.motion,50.);
        let span_x=f64::from(high[0]-low[0]).powf(2.).sqrt() as f32;
        let x=(f64::from(sample(random))*f64::from(span_x)-f64::from(span_x*0.5)) as f32;
        let span_y=f64::from(high[1]-low[1]).powf(2.).sqrt() as f32;
        let y=(f64::from(sample(random))*f64::from(span_y)-f64::from(span_y*0.5)) as f32;
        direction=normalize_nonzero([d.motion[0]+x,d.motion[1]+y,0.]);
        let speed=(f64::from(sample(random))*f64::from(c.distance)+f64::from(c.distance)) as f32;
        point=add(point,scale(direction,speed));d.path.append(point);
    }
    let total=c.durations.iter().fold(0i32,|sum,v|sum.wrapping_add(*v));
    d.path.finish(total as f32,c.tangent);
    d.position=d.path.at(d.clock);
    //209d samples the initial position after building the path; captured
    //remains the earlier baseline placement until the first public update.
    for child in trail {
        child.clock=0;child.phase=0;child.position=d.position;
        child.vertex_rgb=Some([0x00ff_1e1e;4]);child.alpha=0;
    }
}
