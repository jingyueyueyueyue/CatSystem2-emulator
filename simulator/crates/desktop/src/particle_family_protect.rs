//! protect1ce0/1e70/28e0 and protect21370/1440/18d0. Shared SDK
//! trigonometric motion, independently recovered geometry/sprite consumers.
use super::*;
#[path="particle_family_protect_data.rs"]
mod data;

#[derive(Debug,Default)]
pub(super) struct ProtectConfig {
    center:[f32;3],size:f32,uv_step:[f32;2],spread:[f32;2],distance:f32,
    durations:[f32;3],
}

//2ba0/1cf0 clamp the input clock to1, truncate it for the final comparison,
//then use SDK74/10/18/1c. No randomness or source-relative translation.
fn motion(from:[f32;3],to:[f32;3],clock:u32,duration:f32)->[f32;3] {
    let now=(clock as i32 as f32).max(1.);
    if (trunc(now) as u32) as f32>=duration {return to;}
    let half=std::f32::consts::FRAC_PI_2;
    let ax=super::super::lerp(half,0.,duration,now);
    let ay=super::super::lerp(std::f32::consts::PI,std::f32::consts::PI+half,duration,now);
    let x=(f64::from(ax).cos() as f32)-(f64::from(half).cos() as f32);
    let y=(f64::from(ay).sin() as f32)-(f64::from(std::f32::consts::PI).sin() as f32);
    [from[0]+x*(to[0]-from[0]),from[1]-y*(to[1]-from[1]),from[2]+x*(to[2]-from[2])]
}

impl PathFamily {
    pub(super) fn start_protect(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let sprite=self.profile.kind==PathKind::Protect2;
        let size=word(p,2)/100.;
        self.protect=ProtectConfig {center:[word(p,0),word(p,1),0.],
            size:if !sprite && 0.001>size {0.001} else {size},
            uv_step:[word(p,3)/10000.,word(p,4)/10000.],
            spread:[word(p,3),word(p,4)],distance:word(p,5),
            durations:if sprite {[word(p,6),word(p,7),word(p,8)]} else {[60.,5.,60.]}};
        self.main_count=if sprite {self.drops.len()} else {self.drops.len()/26};
        self.child_count=if sprite {0} else {25};self.active_count=self.main_count;
        if sprite || trunc(word(p,5))!=0 {self.stream_render_mode=1;}
        let c=&self.protect;
        if sprite {
            for d in &mut self.drops {
                let tile=random.next_integer()&3;
                d.uv=std::array::from_fn(|i|[((tile&1)+(i&1) as u32) as f32*0.5,
                    ((tile>>1)+(i>>1) as u32) as f32*0.5]);
                d.pivot=self.image.map(|v|v*0.5);
                d.scale=sample(random)*c.size+c.size*0.5;d.stretch=[1.;2];
                let sign=if random.next_integer()&1==0 {-1.} else {1.};
                d.rotation=(sample(random)*0.02+0.02)*sign;
                d.angular_step=d.rotation*3.;
                let dy=sample(random)-0.5;let dx=sample(random)-0.5;
                let direction=normalize_nonzero([dx,dy,0.]);
                d.position=[c.center[0]+sample(random)*(c.spread[0]*direction[0]),
                    c.center[1]+sample(random)*(c.spread[1]*direction[1]),d.scale];
                d.motion_origin=d.position;
                let away=normalize_nonzero(sub(d.position,c.center));
                let distance=c.distance*(sample(random)+0.05);
                d.motion=add(d.position,scale(away,distance));
                d.motion_limit=c.durations.iter().sum();
                d.clock=0;d.phase_origin=0;d.phase=0;d.alpha=0;
                d.vertex_rgb=Some([0x00ff_ffff;4]);
            }
            return;
        }
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            d.clock=0;d.phase_origin=0;d.phase=0;d.alpha=0;d.saved_alpha=0.;
            for (tile,child) in children[index*25..index*25+25].iter_mut().enumerate() {
                child.geometry=Some(data::VERTICES[tile]);child.position=[0.;3];
                child.scale=1.;child.stretch=[1.;2];child.pivot=[0.;2];
                child.uv=data::UV_ALPHA[tile].map(|v|[v[0],v[1]]);
                child.vertex_alpha=Some(data::UV_ALPHA[tile].map(|v|trunc(v[2]) as u8));
                child.vertex_rgb=Some([0x00ff_ffff;4]);child.alpha=255;
                child.clock=0;child.motion=[data::UV_ALPHA[tile][0][0],data::UV_ALPHA[tile][0][1],0.];
            }
        }
    }

    pub(super) fn update_protect(&mut self,tick:u32) {
        let c=&self.protect;
        if self.profile.kind==PathKind::Protect2 {
            for d in &mut self.drops {
                if d.phase<=2 {
                    d.position=motion(d.motion_origin,d.motion,d.clock,d.motion_limit);
                    //1440 adds absolute clock*.002 to the target Y AFTER use.
                    d.motion[1]+=(d.clock as i32 as f32)*0.002;
                    let spin=motion([0.;3],[0.,0.,d.angular_step],d.clock,d.motion_limit);
                    d.rotation+=spin[2];
                    let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
                    let stage=d.phase as usize;
                    if stage!=1 {
                        d.alpha=trunc(super::super::lerp(if stage==0 {0.} else {255.},
                            if stage==0 {255.} else {0.},c.durations[stage],
                            if stage==0 {d.clock as i32 as f32} else {age})) as u8;
                    }
                    if age>c.durations[stage] {d.phase=if stage==2 {9} else {d.phase+1};d.phase_origin=d.clock;}
                }
                d.clock=d.clock.wrapping_add(tick);
            }
            return;
        }
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            if d.phase<=2 {
                let stage=d.phase as usize;
                if stage!=1 {
                    d.saved_alpha=trunc(super::super::lerp(if stage==0 {0.} else {255.},
                        if stage==0 {255.} else {0.},c.durations[stage],age)) as f32;
                }
                if stage==2 {self.stream_render_mode=1;}
                if age>c.durations[stage] {
                    d.phase=if stage==2 {9} else {d.phase+1};d.phase_origin=d.clock;
                    if stage==1 {self.stream_render_mode=1;}
                }
            }
            for (tile,child) in children[index*25..index*25+25].iter_mut().enumerate() {
                //The local and main rotations/scales are initialized to
                //identity and have no producer writes in this fixed module.
                let geometry=data::VERTICES[tile].map(|v|[v[0]*c.size+c.center[0],v[1]*c.size+c.center[1],v[2]]);
                child.geometry=Some(geometry);
                let a=sub(geometry[1],geometry[0]);let b=sub(geometry[2],geometry[0]);
                let n=normalize_nonzero([a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]);
                let shade=trunc(super::super::lerp(255.,60.,2.,n[0]+n[1]-n[2]+1.)) as u8;
                let rgb=u32::from(shade)*0x010101;child.vertex_rgb=Some([rgb;4]);
                child.vertex_alpha=Some(data::UV_ALPHA[tile].map(|v|trunc(d.saved_alpha/255.*v[2]) as u8));
                child.motion[0]+=motion([0.;3],[c.uv_step[0],0.,0.],d.clock,65.)[0];
                child.motion[1]+=motion([0.;3],[c.uv_step[1],0.,0.],d.clock,65.)[0];
                child.uv=std::array::from_fn(|i|[child.motion[0]+(i&1) as f32*0.2,
                    child.motion[1]+(i>>1) as f32*0.2]);
                child.clock=child.clock.wrapping_add(tick);
            }
            d.clock=d.clock.wrapping_add(tick);
        }
        //Terminal9 keeps the last submitted geometry; no SDK04/respawn.
    }
}
