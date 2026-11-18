//! star/o_150102: invisible seven-direction path emitters with independently
//! timed, textured child paths. Fixed PE differences remain explicit.
use super::*;

#[derive(Debug,Default)]
pub(super) struct StarConfig {duration:i32,emission_limit:i32,child_duration:i32,shrink:f32}

//12d30: actual three-DWORD byte colors, copied to SDK vertices unchanged.
const STAR_COLORS:[u32;7]=[0xf0f0f0,0xffff0a,0xff0aff,0x0affff,0xff0a0a,0x0aff0a,0x0a0aff];

fn rotated(direction:[f32;3],angle:f32)->[f32;3] {
    //26f0/2790 quantizes the matrix angle, unlike SDK scalar trig callbacks.
    let angle=trunc((f64::from(angle)*1000.) as f32) as f32/1000.;
    let (s,c)=f64::from(angle).sin_cos();let (s,c)=(s as f32,c as f32);
    [direction[0]*c-direction[1]*s,direction[0]*s+direction[1]*c,direction[2]]
}

impl PathFamily {
    pub(super) fn start_star(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let modern=self.profile.kind==PathKind::Star;
        //10d0 computes the grouping divisor as total/7, then2360 divides
        //TOTAL by that divisor. It is not seven quads per emitter.
        let divisor=self.drops.len()/7;
        self.main_count=if divisor==0 {self.drops.len()} else {self.drops.len()/divisor};
        self.child_count=divisor.saturating_sub(1);
        self.star=StarConfig {duration:trunc(word(p,5)),
            emission_limit:if modern {trunc(word(p,7))} else {16},
            child_duration:if modern {200} else {120},shrink:if modern {0.99} else {0.94}};
        let origin=[word(p,0),word(p,1),0.];let target=[word(p,2),word(p,3),0.];
        let spread=if modern {word(p,6)} else {140.};
        let spread=(f64::from(spread)*f64::from(std::f32::consts::PI)/180.) as f32;
        let base_size=(f64::from(word(p,4))/100.) as f32;
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            d.alpha=0;d.position=origin;d.captured=origin;d.clock=0;d.phase_origin=0;d.phase=0;
            d.path=Trajectory::default();d.path.append(origin);d.motion_limit=base_size;
            let now=(index as f64*f64::from(16.666666f32)) as f32;
            let half=(f64::from(spread)*0.5) as f32;
            let angle=super::super::lerp(-half,half,100.,now);
            let vector=rotated(sub(target,origin),angle);
            let mut distance=length(vector);let direction=normalize_nonzero(vector);
            d.path.append(add(origin,scale(direction,distance)));
            for _ in 0..2 {
                distance=(f64::from(distance)*f64::from(1.01f32)) as f32;
                d.path.append(add(origin,scale(direction,distance)));
            }
            d.path.finish(self.star.duration as f32,d.path.length);
            for child in children.iter_mut().skip(index*self.child_count).take(self.child_count) {
                child.clock=0;child.phase=0;child.phase_origin=0;child.path=Trajectory::default();
                child.angular_step=(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32;
                let tile=((random.next_integer()&3)+2).min(3);
                let origin=[(tile&1) as f32*0.5,(tile>>1) as f32*0.5];
                child.uv=std::array::from_fn(|i|[origin[0]+(i&1) as f32*0.5,origin[1]+(i>>1) as f32*0.5]);
                child.pivot=self.image.map(|n|n*0.5);child.vertex_rgb=Some([STAR_COLORS[index];4]);child.alpha=0;
            }
        }
    }

    pub(super) fn update_star(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        let modern=self.profile.kind==PathKind::Star;
        let config=&self.star;let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            if d.phase==0 {
                d.position=d.path.at(d.clock);
                if d.clock as i32>config.duration {d.phase=1;d.phase_origin=d.clock;}
            } else if d.phase==1 {
                let elapsed=if modern {d.clock.wrapping_sub(d.phase_origin)} else {d.clock};
                if elapsed as i32>60 {d.phase=2;d.phase_origin=d.clock;}
            }
            d.motion_limit=(f64::from(d.motion_limit)*f64::from(config.shrink)) as f32;
            let first=index*self.child_count;
            let pool=&mut children[first..first+self.child_count];
            if d.phase==0 {
                let mut emitted=0i32;
                for child in pool.iter_mut().filter(|c|c.phase==0) {
                    let x=super::super::lerp(d.captured[0],d.position[0],1.,sample(random));
                    let y=super::super::lerp(d.captured[1],d.position[1],1.,sample(random));
                    let size=f64::from(d.motion_limit)*2.;
                    child.position=[((f64::from(sample(random))*100.-50.)*size+f64::from(x)) as f32,
                        ((f64::from(sample(random))*100.-50.)*size+f64::from(y)) as f32,
                        (f64::from(sample(random))*60.*size+f64::from(d.position[2])) as f32];
                    child.rotation=d.rotation;child.path=Trajectory::default();child.path.append(child.position);
                    let mut direction=sub(child.position,d.captured);
                    if length(direction)<0.001 {
                        direction=sub(d.path.at(child.clock.wrapping_add(1)),d.path.at(child.clock));
                    }
                    direction=normalize_nonzero(direction);
                    let spread=(30.*f64::from(std::f32::consts::PI)/180.) as f32;
                    let now=(f64::from(sample(random))*100.) as f32;
                    direction=normalize_nonzero(rotated(direction,super::super::lerp(-spread,spread,100.,now)));
                    let distance=(f64::from(sample(random))*50.+50.) as f32;
                    child.path.append(add(child.position,scale(direction,distance)));
                    child.path.finish(config.child_duration as f32,child.path.length/10.);
                    child.scale=if child.uv[0][0]>0.4 {(f64::from(d.motion_limit)*f64::from(0.7f32)) as f32} else {d.motion_limit};
                    child.stretch=[1.;2];
                    child.angular_step=(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32;
                    child.saved_alpha=(random.next_integer()%200) as f32;
                    child.phase=1;emitted=emitted.wrapping_add(1);
                    //1a14 breaks AFTER emission when count > limit, i.e. limit+1.
                    if emitted>config.emission_limit {break;}
                }
            }
            for child in pool.iter_mut().filter(|c|c.phase!=0) {
                child.position=child.path.at(child.clock);
                let angular=if modern {f64::from(0.04f32)} else {0.04};
                child.angular_step=(f64::from(sample(random))*angular+f64::from(child.angular_step)) as f32;
                child.rotation=child.angular_step;
                child.scale=(f64::from(child.scale)*f64::from(0.99f32)) as f32;
                let alpha=trunc(super::super::lerp(child.saved_alpha,0.,config.child_duration as f32,child.clock as i32 as f32));
                child.alpha=((f64::from(sample(random))*f64::from(0.6f32)+f64::from(0.4f32))*f64::from(alpha)).trunc() as i32 as u8;
                child.clock=child.clock.wrapping_add(tick);
                if child.clock as i32>=config.child_duration {child.clock=0;child.phase=0;child.alpha=0;}
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
