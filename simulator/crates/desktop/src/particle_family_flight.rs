//! hotaru_move's explicit path and reusable segment-trail consumer.
//! This identity has one parent and count-1 children, not the hotaru random
//! emitter. Parent and child clocks below follow their distinct PE operands.
use super::*;

#[derive(Debug,Default)]
pub(super) struct FlightConfig {departure:Trajectory,observed_trigger:i32}

impl PathFamily {
    pub(super) fn start_flight(&mut self,p:&[u8;128]) {
        let stay=self.profile.kind==PathKind::HotaruStay;
        self.main_count=usize::from(!self.drops.is_empty());
        self.child_count=self.drops.len()-self.main_count;self.active_count=self.main_count;
        self.scale=word(p,2)/100.;if self.scale<0. {self.scale=0.01;}
        self.factor=word(p,3)/1000.;if self.factor<0. {self.factor=0.001;}
        self.radius=trunc(word(p,4)).clamp(0,255) as f32;
        self.flight.observed_trigger=if stay {trunc(word(p,5))} else {0};
        let (parents,trail)=self.drops.split_at_mut(self.main_count);
        let Some(d)=parents.first_mut() else {return;};
        d.pivot=self.image.map(|n|n*0.5);d.scale=self.scale;d.stretch=[1.;2];
        d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;d.clock=0;d.phase=0;d.phase_origin=0;
        d.path=Trajectory::default();
        let list=if stay {7} else {6};
        for index in 0..trunc(word(p,list-1)).max(0) as usize {
            d.path.append([word(p,list+index*2),word(p,list+1+index*2),0.]);
        }
        if d.path.count!=0 {d.position=d.path.points[0];}
        d.path.finish(word(p,0),word(p,1));d.captured=d.position;
        for c in trail {
            c.position=d.position;c.pivot=d.pivot;c.vertex_rgb=d.vertex_rgb;c.alpha=d.alpha;
            c.clock=0;c.phase=0;
        }
    }

    pub(super) fn update_flight(&mut self,tick:u32,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let stay=self.profile.kind==PathKind::HotaruStay;
        let (parents,trail)=self.drops.split_at_mut(self.main_count);
        let Some(d)=parents.first_mut() else {return;};
        if stay && !matches!(d.phase,0|1|3) {
            d.pivot=self.image.map(|n|n*0.5);d.scale=self.scale;d.stretch=[1.;2];
            d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;d.clock=0;d.phase=0;
            d.position=d.path.points[0];d.captured=d.position;
            for c in trail.iter_mut() {
                c.position=d.position;c.pivot=d.pivot;c.vertex_rgb=d.vertex_rgb;c.alpha=0;c.clock=0;c.phase=0;
            }
        } else if stay && d.phase==3 {
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            d.alpha=trunc(super::super::sprites::triangle(elapsed,180.)*30.+225.) as u8;
            d.position=self.flight.departure.at_time(elapsed);
        } else if d.phase<=1 {
            d.position=d.path.at(d.clock);
            if d.phase==0 {
                d.alpha=trunc(super::super::lerp(0.,225.,120.,d.clock as i32 as f32)) as u8;
                if d.clock as i32>=120 {d.phase=1;d.phase_origin=d.clock;}
            } else {
                let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
                d.alpha=trunc(super::super::sprites::triangle(elapsed,180.)*30.+225.) as u8;
                if d.clock as i32 as f32>=d.path.duration {
                    if stay {d.clock=0;d.phase_origin=0;} else {d.phase=4;}
                }
            }
        }
        if let Some(c)=trail.iter_mut().find(|c|c.phase==0 && (!stay || d.phase!=0)) {
            let v=sub(d.position,d.captured);let distance=length(v);let direction=normalize_nonzero(v);
            let angle=f64::from(direction[1]).acos() as f32;
            if !stay || distance!=0. {c.rotation=if v[0]>0. {-angle} else {angle};}
            // 24cb writes midpoint, then2529 overwrites it with the current
            // position. 25b9 consumes already-written X scale for Y.
            c.position=if stay && distance!=0. {scale(add(d.position,d.captured),0.5)} else {d.position};
            c.pivot=d.pivot;c.scale=1.;let size_x=d.scale*d.stretch[0]*self.factor;
            c.stretch=if stay && distance==0. {[d.scale*d.stretch[0],d.scale*d.stretch[1]]}
                else {[size_x,if stay {size_x*distance} else {(size_x*(d.scale*d.stretch[1]))*distance}]};
            if stay && distance!=0. {c.alpha=0;}
            c.phase=1;
        }
        for c in trail.iter_mut().filter(|c|c.phase!=0) {
            c.stretch=c.stretch.map(|v|v*0.999);
            // The private child pointer saved at260b/289c is read after
            // sub esp,14 at2662/28f3: both clocks here belong to the child.
            c.alpha=trunc(super::super::lerp(self.radius,0.,self.child_count as f32,c.clock as i32 as f32)) as u8;
            c.clock=c.clock.wrapping_add(tick);
            if c.clock as i32>=self.child_count as i32 {c.clock=0;c.phase=0;c.alpha=0;}
        }
        if stay && self.flight.observed_trigger as f32!=word(p,5) {
            // 29f7 compares the stored INTEGER against live p5 FLOAT.
            // This is an external parameter-change producer, not a timer.
            self.flight.departure=departure_path(d.position,d.captured,word(p,6),random);
            self.flight.observed_trigger=trunc(word(p,5));d.phase=3;d.phase_origin=d.clock;
        }
        d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        // Phase4 stays at its final transform/alpha. No respawn or owning
        // source translation is produced by21b0 for this exact module.
    }
}

// 2a14..3109: current point, forward100, reflected/halved direction100,
// four random ±30 degree bounded steps, then fixed exit(-100,100,0).
fn departure_path(start:[f32;3],previous:[f32;3],duration:f32,
    random:&mut impl ParticleRandomSource) -> Trajectory {
    let mut path=Trajectory::default();path.append(start);
    let mut direction=normalize_nonzero(sub(start,previous));
    let mut point=add(start,scale(direction,100.));path.append(point);
    if direction[0]>0. && direction[1]!=0. {direction[0]=-direction[0];}
    else {direction[1]*=0.5;}
    point=add(point,scale(direction,100.));path.append(point);
    let end=[-100.,100.,0.];direction=normalize_nonzero(sub(end,point));
    for _ in 0..4 {
        let minus=flight_rotate(direction,-30.);let plus=flight_rotate(direction,30.);
        let span_x=(plus[0]-minus[0]).abs();let span_y=(plus[1]-minus[1]).abs();
        let jitter_x=sample(random)*span_x-span_x*0.5+direction[0];
        let jitter_y=sample(random)*span_y-span_y*0.5+direction[1];
        direction=normalize_nonzero([jitter_x,jitter_y,0.]);
        point=add(point,scale(direction,sample(random)*100.+100.));path.append(point);
    }
    path.append(end);path.finish(duration,100.);path
}

fn flight_rotate(v:[f32;3],degrees:f32) -> [f32;3] {
    let angle=((f64::from(degrees)*f64::from(std::f32::consts::PI))/180.) as f32;
    let s=f64::from(angle).sin() as f32;let c=f64::from(angle).cos() as f32;
    [v[0]*c-v[1]*s,v[1]*c+v[0]*s,0.]
}
