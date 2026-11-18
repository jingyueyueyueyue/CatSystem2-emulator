//! Recovered SDK2 atlas emitters; exact identities are gated by profiles.
//! wc_invisi13d0/1540/17f0 keeps private placement separate from SDK scale.
use super::*;

#[derive(Debug,Default)]
pub(super) struct AtlasEmitterConfig {
    radius:[f32;2],
    minimum:[f32;2],
    from:[f32;3],
    to:[f32;3],
    clock:f32,
    travel:i32,
    speed:[f32;2],
    size_range:f32,
    lifetime:i32,
    alpha:i32,
    calls:u32,
}

impl PathFamily {
    pub(super) fn start_radial_flare(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.center=[word(p,0),word(p,1),0.];self.scale=word(p,2)/100.;
        self.atlas_emitter.speed[0]=word(p,3)/10.;
        self.main_count=self.drops.len();self.active_count=1;self.child_count=0;
        for d in &mut self.drops {spawn_radial_flare(d,self.image,self.center,self.scale,self.main_count,random);}
    }

    pub(super) fn update_radial_flare(&mut self,tick:u32,random:&mut impl ParticleRandomSource) {
        self.active_count=(self.active_count+2).min(self.main_count);
        for d in self.drops.iter_mut().take(self.active_count) {
            let now=d.clock as i32 as f32;let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            if d.phase<=1 {
                let fading=d.phase==1;let factor=if fading {0.95f32} else {0.985f32};
                d.alpha=trunc(if fading {super::super::lerp(255.,0.,30.,age)} else {super::super::lerp(0.,255.,10.,now)}) as u8;
                if let Some(colors)=&mut d.vertex_rgb {
                    for color in colors {
                        let mut value=0;
                        for shift in [0,8,16] {value|=(trunc(((*color>>shift)&255) as f32*factor) as u8 as u32)<<shift;}
                        *color=value;
                    }
                }
                d.position[0]+=d.motion[0]*self.atlas_emitter.speed[0];
                d.position[1]+=d.motion[1]*self.atlas_emitter.speed[0];
                if fading {
                    d.stretch[0]=super::super::lerp(self.scale*0.3f32,self.scale*0.01f32,30.,age);
                    if age>30. {d.phase=2;d.phase_origin=d.clock;}
                } else {
                    d.stretch=[super::super::lerp(self.scale*0.5,self.scale*0.3f32,60.,now),
                        super::super::lerp(self.scale*0.4f32,self.scale*0.2f32,60.,now)];
                    if now>60. {d.phase=1;d.phase_origin=d.clock;}
                }
            } else {spawn_radial_flare(d,self.image,self.center,self.scale,self.main_count,random);}
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
        //2020 explicitly sets every child count to0. No offset is added.
    }

    pub(super) fn start_wc_fall(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.atlas_emitter.radius=[word(p,0),word(p,2)];
        self.atlas_emitter.minimum=[word(p,3)/100.,word(p,4)/100.];
        self.atlas_emitter.travel=trunc(word(p,5));self.atlas_emitter.calls=0;
        self.main_count=self.drops.len();self.active_count=1;
        for d in &mut self.drops {d.phase_origin=0;spawn_wc_fall(d,self.image,&self.atlas_emitter,random);}
        //p1 is stored but not consumed. Object64 is the fixed threshold2.
    }

    pub(super) fn update_wc_fall(&mut self,tick:u32,offset:[f32;2],random:&mut impl ParticleRandomSource) {
        let c=&mut self.atlas_emitter;
        if c.calls>2 {self.active_count=(self.active_count+1).min(self.main_count);c.calls=0;}
        for d in self.drops.iter_mut().take(self.active_count) {
            let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            match d.phase {
                0=>{
                    d.position=fall_position(d.captured,d.motion_origin,age,c.travel as f32);d.alpha=255;
                    //16d4 writes5, not1. The next call respawns the slot.
                    if age>c.travel as f32 {d.phase=5;d.phase_origin=d.clock;}
                },
                1=>if age>0. {d.phase=2;d.phase_origin=d.clock;},
                2=>{
                    //Private2c/30 are cleared by1120 and never assigned by
                    //start/spawn. This branch is not reached from phase0.
                    d.position=emitter_position(d.captured,d.motion_origin,age,0.);
                    d.alpha=trunc(super::super::lerp(255.,0.,0.,age)) as u8;
                    if age>0. {d.phase=3;d.phase_origin=d.clock;}
                },
                _=>spawn_wc_fall(d,self.image,c,random),
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
        c.calls=c.calls.wrapping_add(1);
    }

    pub(super) fn start_heart(&mut self,p:&[u8;128]) {
        self.center=[word(p,0),word(p,1),0.];self.stream_render_mode=1;
        self.durations=[trunc(word(p,2)),trunc(word(p,3)),trunc(word(p,4))];
        const HORIZONTAL:[f32;15]=[-5.,-3.,1.,5.,-3.,5.,4.,2.,0.,-2.,-1.,-4.,0.,-1.,-2.];
        for (i,d) in self.drops.iter_mut().enumerate() {
            d.pivot=self.image.map(|v|v*0.5);let frame=if i<15 {i+1} else {0};
            let x=(frame%4) as f32*0.25;let y=(frame/4) as f32*0.25;
            d.uv=[[x,y],[x+0.25,y],[x,y+0.25],[x+0.25,y+0.25]];
            d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;d.scale=0.3;d.stretch=[1.;2];
            d.captured=[self.center[0],self.center[1],i as f32*0.1f32];d.position=d.captured;
            let horizontal=HORIZONTAL.get(i).copied().unwrap_or(0.);
            d.motion_origin=[d.captured[0]-horizontal*0.5,d.captured[1]+30.+(i as i32).wrapping_mul(2) as f32,d.captured[2]];
            d.durations=[self.durations[0].wrapping_sub(i as i32).wrapping_add(15),self.durations[1],self.durations[2]];
            d.clock=0;d.phase_origin=0;d.phase=0;
        }
        //p5 is saved at object58, but update never consumes it.
    }

    pub(super) fn update_heart(&mut self,tick:u32,offset:[f32;2]) {
        for (i,d) in self.drops.iter_mut().enumerate() {
            let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            match d.phase {
                0=>{
                    d.scale=super::super::lerp(0.3,0.25,d.durations[0] as f32,age);
                    d.alpha=trunc(super::super::lerp(0.,255.,d.durations[0] as f32,age)) as u8;
                    if age>d.durations[0] as f32 {d.phase=1;d.phase_origin=d.clock;}
                },
                1=>if age>d.durations[1] as f32 {d.phase=2;d.phase_origin=d.clock;},
                2=>{
                    let duration=if i<15 {d.durations[2] as f32} else {30.};
                    if i<15 {d.position=emitter_position(d.captured,d.motion_origin,age,duration);}
                    else {d.scale=super::super::lerp(0.25,0.3,duration,age);}
                    d.alpha=trunc(super::super::lerp(255.,0.,duration,age)) as u8;
                    if age>duration {d.phase=3;d.phase_origin=d.clock;}
                },
                _=>{},
            }
            //Only the first15 phase2 branch rewrites SDK position. Preserve
            //the literal additive source-offset behavior in hold/terminal.
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn start_bubble(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.center=[word(p,0),word(p,1),0.];self.atlas_emitter.from=self.center;
        self.atlas_emitter.to=[word(p,2),word(p,3),0.];self.atlas_emitter.travel=trunc(word(p,4));
        self.atlas_emitter.clock=0.;self.atlas_emitter.speed=[word(p,5)/100.,word(p,6)/100.];
        self.scale=word(p,7)/1000.;self.atlas_emitter.size_range=word(p,8)/1000.;
        self.atlas_emitter.radius=[word(p,9),word(p,10)];
        //start stores p11 at98 (spawn lifetime), p12 at88 (spawn alpha).
        self.atlas_emitter.lifetime=trunc(word(p,11));self.atlas_emitter.alpha=trunc(word(p,12));
        if word(p,13)!=0. {self.stream_render_mode=1;}
        self.main_count=self.drops.len();self.active_count=0;
        for d in &mut self.drops {spawn_bubble(d,self.image,self.center,self.scale,&self.atlas_emitter,random);}
    }

    pub(super) fn update_bubble(&mut self,delta:f32,tick:u32,offset:[f32;2],random:&mut impl ParticleRandomSource) {
        let c=&mut self.atlas_emitter;
        self.center=emitter_position(c.from,c.to,c.clock,c.travel as f32);
        self.active_count=(self.active_count+4).min(self.main_count);
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase==0 {
                spawn_bubble(d,self.image,self.center,self.scale,c,random);d.phase=1;
            } else if d.phase!=1 && c.clock<c.travel as f32 {
                spawn_bubble(d,self.image,self.center,self.scale,c,random);
                //1663 returns directly to1741; phase0 runs on the NEXT call.
                d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
                continue;
            }
            if d.phase==1 {
                let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
                d.captured[0]+=d.motion[0];d.captured[1]+=(d.motion[1]-age*0.02f32);
                d.position[0]=d.captured[0];d.position[1]=d.captured[1];
                d.alpha=trunc(super::super::lerp(d.saved_alpha,0.,d.motion_limit,age)) as u8;
                if age>d.motion_limit {d.phase=3;d.phase_origin=d.clock;}
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
        c.clock+=delta;
    }

    pub(super) fn start_number(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.center=[word(p,0),word(p,1),0.];
        self.durations=[trunc(word(p,4)),trunc(word(p,5)),0];self.atlas_emitter.alpha=trunc(word(p,6));
        if word(p,7)!=0. {self.stream_render_mode=1;}
        self.main_count=self.drops.len();
        for (i,d) in self.drops.iter_mut().enumerate() {
            d.pivot=self.image.map(|v|v*0.5);set_number_atlas(d,random);d.scale=1.;d.stretch=[1.,1./16.];
            set_bright_color(d,random);d.alpha=100;d.captured=[self.center[0],self.center[1]+i as f32*26.,0.];
            //1880 samples a direction and private velocity/alpha even though
            //1510 never integrates that velocity or reads the private alpha.
            let _=signed_direction(random);let _=sample(random);let _=sample(random);
            d.clock=0;d.phase_origin=0;d.phase=0;
        }
        //p2/3 size are stored but update never applies them. Object80,48/4c
        //are not initialized by12b0/start; their unused arithmetic is not
        //invented as a moving emitter or allocator-dependent velocity.
    }

    pub(super) fn update_number(&mut self,tick:u32,offset:[f32;2],random:&mut impl ParticleRandomSource) {
        for d in &mut self.drops {
            let age=d.clock.wrapping_sub(d.phase_origin) as i32;
            d.position=d.captured;
            if d.phase==0 {
                set_number_atlas(d,random);set_bright_color(d,random);d.alpha=self.atlas_emitter.alpha as u8;
                if age>self.durations[0] {d.phase=1;d.phase_origin=d.clock;}
            } else if d.phase==1 && age>self.durations[1] {d.phase=0;d.phase_origin=d.clock;}
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn start_invisi(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        self.center=[word(p,0),word(p,1),0.];self.scale=word(p,2)/100.;
        self.atlas_emitter.radius=[word(p,3),word(p,4)];
        self.atlas_emitter.minimum=[word(p,5)/100.,word(p,6)/100.];
        self.durations=[trunc(word(p,7)),trunc(word(p,8)),0];
        self.main_count=self.drops.len();self.active_count=1;
        for d in &mut self.drops {spawn_invisi(d,self.image,self.center,self.scale,&self.atlas_emitter,random);}
        //p9..12 are stored in the PE but neither1540 nor17f0 consumes them.
    }

    pub(super) fn update_invisi(&mut self,tick:u32,offset:[f32;2],random:&mut impl ParticleRandomSource) {
        //The period is1, but1540 still consumes the host integer callback.
        let _=random.next_integer();self.active_count=(self.active_count+1).min(self.main_count);
        for d in self.drops.iter_mut().take(self.active_count) {
            let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            if d.phase<=1 {
                d.position=d.captured;d.scale*=1.001f32;
                if d.phase==0 {
                    d.alpha=trunc(super::super::lerp(0.,255.,self.durations[0] as f32,age)) as u8;
                    //176d compares the absolute private clock, not age.
                    if (d.clock as i32)>self.durations[0] {d.phase=1;d.phase_origin=d.clock;}
                } else {
                    d.alpha=trunc(super::super::lerp(255.,0.,self.durations[1] as f32,age)) as u8;
                    if age>self.durations[1] as f32 {d.phase=2;d.phase_origin=d.clock;}
                }
            } else {spawn_invisi(d,self.image,self.center,self.scale,&self.atlas_emitter,random);}
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn spawn_radial_flare(d:&mut Drop,image:[f32;2],center:[f32;3],size:f32,count:usize,
    random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);let frame=random.next_integer()%3+1;
    let x=(frame&1) as f32*0.5;let y=(frame>>1) as f32*0.5;
    d.uv=[[x,y],[x+0.5,y],[x,y+0.5],[x+0.5,y+0.5]];
    d.scale=1.;d.stretch=[size*0.5,size*0.4f32];d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;
    let index=random.next_integer()%(count as u32);
    let step=(360./count as f64) as f32;let degrees=index as f32*step;
    let radians=(f64::from(degrees)*f64::from(std::f32::consts::PI)/180.) as f32;
    let radians=trunc(radians*1000.) as f32/1000.;
    //1c44 overwrites the earlier integer at esp38 after push; both sin and
    //cos consume quantized FLOAT radians, rather than integer bits.
    d.position=[(f64::from(radians).sin()*40.+f64::from(center[0])) as f32,
        (f64::from(radians).cos()*40.+f64::from(center[1])) as f32,
        (f64::from(sample(random))*50.) as f32];
    d.motion=normalize_nonzero([d.position[0]-center[0],d.position[1]-center[1],0.]);
    let mut angle=f64::from(-d.motion[1]).acos() as f32;
    if -d.motion[0]>0. {angle = -angle;}d.rotation=angle;
    d.clock=0;d.phase=0;d.captured=d.position;
    //1af0 clears clock/phase, but leaves the previous phase origin intact.
}

fn set_bright_color(d:&mut Drop,random:&mut impl ParticleRandomSource) {
    let b=random.next_integer()%31+225;let g=random.next_integer()%31+225;let r=random.next_integer()%31+225;
    d.vertex_rgb=Some([b|(g<<8)|(r<<16);4]);
}

fn spawn_wc_fall(d:&mut Drop,image:[f32;2],c:&AtlasEmitterConfig,random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);let frame=random.next_integer()&3;
    let x=(frame&1) as f32*0.5;let y=(frame>>1) as f32*0.5;
    d.uv=[[x,y],[x+0.5,y],[x,y+0.5],[x+0.5,y+0.5]];
    d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;
    d.captured=[(f64::from(sample(random))*f64::from(c.radius[0])) as f32,-50.,
        (f64::from(sample(random))*f64::from(c.radius[1])) as f32];
    d.position=d.captured;d.motion_origin=[d.captured[0],d.captured[1]+850.,d.captured[2]];
    d.scale=1.;d.stretch=c.minimum;d.clock=0;d.phase=0;
    //1740 leaves private phase_origin untouched on reuse.
}

fn fall_position(from:[f32;3],to:[f32;3],clock:f32,duration:f32)->[f32;3] {
    let clock=if clock<1. {1.} else {clock};if (clock as u32) as f32>=duration {return to;}
    let radians=|degrees:f32|((f64::from(degrees)*f64::from(std::f32::consts::PI))/180.) as f32;
    let start=radians(180.);let angle=super::super::lerp(start,radians(90.),duration,clock);
    let x=(f64::from(angle).cos() as f32)-(f64::from(start).cos() as f32);
    let start=radians(90.);let angle=super::super::lerp(start,radians(180.),duration,clock);
    let y=(f64::from(angle).sin() as f32)-(f64::from(start).sin() as f32);
    [from[0]+x*(to[0]-from[0]),from[1]-y*(to[1]-from[1]),from[2]+x*(to[2]-from[2])]
}

fn set_number_atlas(d:&mut Drop,random:&mut impl ParticleRandomSource) {
    let y=(random.next_integer()&15) as f32/16.;
    d.uv=[[0.,y],[1.,y],[0.,y+1./16.],[1.,y+1./16.]];
}

fn signed_direction(random:&mut impl ParticleRandomSource)->[f32;3] {
    let mut x=sample(random);if random.next_integer()&1==0 {x=-x;}
    let mut y=sample(random);if random.next_integer()&1==0 {y=-y;}
    normalize_nonzero([x,y,0.])
}

fn spawn_bubble(d:&mut Drop,image:[f32;2],center:[f32;3],size:f32,c:&AtlasEmitterConfig,
    random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);let row=random.next_integer()&3;
    let x=(row&1) as f32*0.5;let y=(row>>1) as f32*0.5;
    d.uv=[[x,y],[x+0.5,y],[x,y+0.5],[x+0.5,y+0.5]];
    d.scale=(f64::from(sample(random))*f64::from(c.size_range)+f64::from(size)) as f32;d.stretch=[1.;2];
    set_bright_color(d,random);d.alpha=0;
    d.captured=[(sample(random)*2.-1.)*c.radius[0]+center[0],(sample(random)*2.-1.)*c.radius[1]+center[1],
        (f64::from(sample(random))*5.) as f32];
    let direction=signed_direction(random);let speed=(sample(random)*2.-1.)*c.speed[1]+c.speed[0];
    d.motion=scale(direction,speed);
    let alpha=(sample(random)*2.-1.)*(c.alpha as f32)+c.alpha as f32;
    d.saved_alpha=trunc(alpha.clamp(0.,255.)) as f32;
    d.clock=0;d.phase_origin=0;d.phase=0;
    d.motion_limit=(sample(random)*2.-1.)*(c.lifetime as f32)+c.lifetime as f32;
    //17c0 does not copy private position/Z into the SDK transform.
}

//bubble1c10 is the same recovered trigonometric endpoint helper as the
//sprite scatter model: X uses cos90->0; Y uses sin180->270.
fn emitter_position(from:[f32;3],to:[f32;3],clock:f32,duration:f32)->[f32;3] {
    let clock=if clock<1. {1.} else {clock};
    if (clock as u32) as f32>=duration {return to;}
    let radians=|degrees:f32|((f64::from(degrees)*f64::from(std::f32::consts::PI))/180.) as f32;
    let start=radians(90.);let angle=super::super::lerp(start,radians(0.),duration,clock);
    let x=(f64::from(angle).cos() as f32)-(f64::from(start).cos() as f32);
    let start=radians(180.);let angle=super::super::lerp(start,radians(270.),duration,clock);
    let y=(f64::from(angle).sin() as f32)-(f64::from(start).sin() as f32);
    [from[0]+x*(to[0]-from[0]),from[1]-y*(to[1]-from[1]),from[2]+x*(to[2]-from[2])]
}

fn spawn_invisi(d:&mut Drop,image:[f32;2],center:[f32;3],size:f32,c:&AtlasEmitterConfig,
    random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.uv=[[0.,0.],[0.5,0.],[0.,0.5],[0.5,0.5]];
    d.scale=(sample(random)*size)*0.2f32+size*0.8f32;d.stretch=[1.;2];
    d.rotation=(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32;
    d.rotation_xy=[0.;2];d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;
    let direction=normalize_nonzero([sample(random)-0.5,sample(random)-0.5,0.]);
    d.captured=[
        (sample(random)*(1.-c.minimum[0])+c.minimum[0])*(c.radius[0]*direction[0])+center[0],
        (sample(random)*(1.-c.minimum[1])+c.minimum[1])*(c.radius[1]*direction[1])+center[1],
        d.scale,
    ];
    d.position=d.captured;d.clock=0;d.phase_origin=0;d.phase=0;
}
