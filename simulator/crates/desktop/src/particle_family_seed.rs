//! wc_seed / wc_seedrot: shared eased transforms, distinct PE state tables.
//! Static evidence: priority2_bae261fba6 / daacc29cb3 and seed_tables.json.
use super::*;

#[derive(Clone,Debug,Default)]
struct SeedParticle {
    from_position:[f32;3], to_position:[f32;3],
    from_rotation:[f32;3], to_rotation:[f32;3],
    from_scale:[f32;3], to_scale:[f32;3],
    duration:f32, started:bool, inside:bool,
}

#[derive(Debug,Default)]
pub(super) struct SeedConfig {
    center:[f32;3], size:f32, fadein:f32, hold:f32, mode:i32, frozen:bool,
    particles:Vec<SeedParticle>,
}

fn seed_uv(d:&mut Drop,tile:u32,span:f32) {
    let origin=if span==0.25 {[(tile&1) as f32*0.25,0.5+(tile>>1) as f32*0.25]}
        else {[(tile&1) as f32*0.5,(tile>>1) as f32*0.5]};
    d.uv=std::array::from_fn(|i|[origin[0]+(i&1) as f32*span,origin[1]+(i>>1) as f32*span]);
}

fn seed_sign(random:&mut impl ParticleRandomSource)->f32 {
    if random.next_integer()&1!=0 {1.} else {-1.}
}

fn seed_angle(random:&mut impl ParticleRandomSource)->f32 {
    let signed=sample(random)*2.-1.;
    let angle=f64::from(signed)*1.570796326794897+1.570796326794897;
    (f64::from(seed_sign(random))*angle) as f32
}

fn spawn_seed(d:&mut Drop,s:&mut SeedParticle,center:[f32;3],size:f32,
    durations:[f32;2],rotated:bool,image:[f32;2],random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);
    let tile=random.next_integer()&if rotated {3} else {1};
    seed_uv(d,tile,0.5);d.alpha=0;d.vertex_rgb=None;
    let x=sample(random)*2.-1.;
    let x=if rotated {(f64::from(sample(random))*1400.*f64::from(x)) as f32} else {x*700.};
    let y=sample(random)*2.-1.;
    let y=if rotated {(f64::from(sample(random))*800.*f64::from(y)) as f32} else {y*400.};
    let distance=f64::from(x*x+y*y).sqrt() as f32;
    let divisor=if rotated {10.} else {4.};
    s.from_position=[center[0]+x/divisor,center[1]+y/divisor,0.];
    s.to_position=[center[0]+x,center[1]+y,if rotated {800.} else {-(distance/1000.)}];
    s.inside=rotated && s.to_position[0]>300. && s.to_position[0]<980.
        && s.to_position[1]>100. && s.to_position[1]<620.;
    if rotated {
        s.from_rotation=std::array::from_fn(|_|seed_angle(random));
        s.to_rotation=std::array::from_fn(|axis| {
            let base=s.from_rotation[axis]+std::f32::consts::PI;
            let angle=(f64::from(sample(random))*f64::from(std::f32::consts::PI)+f64::from(base)) as f32;
            seed_sign(random)*angle
        });
    } else {
        s.from_rotation=[0.,0.,seed_angle(random)];
        let direction=seed_sign(random);
        s.to_rotation=[0.,0.,s.from_rotation[2]+direction*(distance/200.)];
    }
    let scale=if rotated {size} else {(distance/100.)*size};
    s.from_scale=[scale,scale,1.];s.to_scale=s.from_scale.map(|v|v*1.01);
    s.duration=durations[1]+durations[0];s.started=false;
    d.clock=0;d.phase=0;
    // The PE spawn leaves phase_origin and SDK transforms intact. Start
    // clears private storage; a later respawn must not clear SDK storage.
}

//1cb0 /1e70 clamp time to at least1 and use different trigonometric
// interpolation for Y. The c510/c6d0 call is float-to-integer, not a clock.
fn seed_ease(from:[f32;3],to:[f32;3],duration:f32,now:f32)->[f32;3] {
    let now=if 1.>now {1.} else {now};
    if (now as u32) as f32>=duration {return to;}
    let pi=std::f32::consts::PI;
    let half=pi*0.5;
    let a=super::super::lerp(pi,half,duration,now);
    let b=super::super::lerp(half,pi,duration,now);
    let horizontal=(f64::from(a).cos() as f32)-(f64::from(pi).cos() as f32);
    let vertical=(f64::from(b).sin() as f32)-(f64::from(half).sin() as f32);
    [from[0]+horizontal*(to[0]-from[0]),from[1]-vertical*(to[1]-from[1]),
        from[2]+horizontal*(to[2]-from[2])]
}

fn submit_seed_transform(d:&mut Drop,s:&SeedParticle,rotated:bool) {
    let now=d.clock as i32 as f32;
    let position=seed_ease(s.from_position,s.to_position,s.duration,now);
    d.position[0]=position[0];d.position[1]=position[1];
    if rotated {d.position[2]=position[2];}
    let angle=seed_ease(s.from_rotation,s.to_rotation,s.duration,now)[2];
    d.rotation=angle;
    // All three SDK rotation stores take eased Z in seedrot, even though
    // its initializer independently generates all three private components.
    if rotated {d.rotation_xy=[angle;2];}
    let scale=seed_ease(s.from_scale,s.to_scale,s.duration,now);
    d.scale=1.;d.stretch=[scale[0],scale[1]];
}

impl PathFamily {
    pub(super) fn start_seed(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let rotated=self.profile.kind==PathKind::SeedRot;
        self.seed=SeedConfig {center:[word(p,0),word(p,1),0.],size:word(p,2)/1000.,
            fadein:word(p,3),hold:word(p,4),mode:0,frozen:false,
            particles:vec![SeedParticle::default();self.drops.len()]};
        self.main_count=self.drops.len();self.child_count=0;self.active_count=0;
        for (d,s) in self.drops.iter_mut().zip(&mut self.seed.particles) {
            d.phase_origin=0;d.motion=[0.;3];
            spawn_seed(d,s,self.seed.center,self.seed.size,[self.seed.fadein,self.seed.hold],rotated,self.image,random);
        }
    }

    pub(super) fn update_seed(&mut self,tick:u32,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let rotated=self.profile.kind==PathKind::SeedRot;
        self.active_count=self.active_count.wrapping_add(4).min(self.main_count);
        let c=&mut self.seed;
        if self.active_count==self.main_count && word(p,5)!=c.mode as f32 {
            c.mode=trunc(word(p,5));c.frozen=c.mode==1;
        }
        if c.frozen {return;}
        for (d,s) in self.drops.iter_mut().zip(&mut c.particles).take(self.active_count) {
            if c.mode==2 && (d.phase as i32)<3 && s.started {d.phase=3;}
            match d.phase {
                0=> {
                    s.started=true;submit_seed_transform(d,s,rotated);
                    d.alpha=trunc(super::super::lerp(0.,255.,c.fadein,d.clock as i32 as f32)) as u8;
                    if (d.clock as i32 as f32)>c.fadein {
                        d.phase=if s.inside {2} else {1};d.phase_origin=d.clock;
                    }
                }
                1|2 if d.phase==1 || rotated=> {
                    submit_seed_transform(d,s,rotated);
                    let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
                    if d.phase==2 {d.alpha=trunc(super::super::lerp(255.,0.,c.hold,elapsed as f32)) as u8;}
                    if (elapsed as f32)>c.hold {d.phase=99;d.phase_origin=d.clock;}
                }
                3 if !rotated=> {
                    seed_uv(d,random.next_integer()&3,0.25);
                    let horizontal=(f64::from(sample(random))*2.-1.) as f32;
                    d.motion[0]=(f64::from(sample(random))*2.*f64::from(horizontal)) as f32;
                    d.motion[1]=-(sample(random)*5.+0.5);
                    d.phase_origin=d.clock;d.phase=4;
                    advance_seed_scatter(d);
                }
                4=>advance_seed_scatter(d),
                5=>{}, // Both actual switch tables retain the invisible terminal particle.
                _=>spawn_seed(d,s,c.center,c.size,[c.fadein,c.hold],rotated,self.image,random),
            }
            d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn advance_seed_scatter(d:&mut Drop) {
    let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
    d.position[0]+=d.motion[0];d.position[1]+=d.motion[1];
    d.alpha=trunc(super::super::lerp(180.,0.,40.,elapsed as f32)) as u8;
    if elapsed>40 {d.phase=5;d.phase_origin=d.clock;}
}
