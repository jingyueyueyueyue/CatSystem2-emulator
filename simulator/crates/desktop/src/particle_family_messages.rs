//! wc_message / wc_message2: shared atlas, affine rectangle submission and
//! private/SDK transforms, with distinct production and terminal lifecycles.
//! Fixed atlas180b0 and actual1d60/1f30/2490 versus1d60/1f00/24c0.
use super::*;

#[derive(Debug,Default)]
pub(super) struct MessageConfig {
    size:[f32;3],angles:[f32;3],initial:[f32;2],speed:f32,speed_range:f32,
    exit_x:f32,atlas:u32,
}

impl PathFamily {
    pub(super) fn start_message(&mut self,p:&[u8;128],viewport:[u32;2],random:&mut impl ParticleRandomSource) {
        let rain=self.profile.kind==PathKind::MessageRandom;
        self.messages.size=[word(p,2)/100.,word(p,3)/100.,1.];
        self.messages.angles=std::array::from_fn(|i|
            (f64::from(word(p,i+4))*f64::from(std::f32::consts::PI)/180.) as f32);
        if rain {
            //1bf0 explicitly initializes object a8 to0, and this version's
            //start never writes it. Do not guess a parameter for its base X.
            self.messages.initial=[0.;2];self.messages.speed=word(p,7)/100.;
            self.messages.speed_range=word(p,8)/100.;
        } else {
            self.messages.initial=[word(p,7),word(p,8)];self.messages.speed=word(p,9)/100.;
            self.messages.atlas=trunc(word(p,10)) as u32;
            self.messages.exit_x=-(viewport[0] as f32+self.image[0]);
        }
        if trunc(word(p,if rain {9} else {11}))!=0 {self.stream_render_mode=1;}
        self.main_count=self.drops.len();self.active_count=if rain {0} else {self.main_count};
        for (i,d) in self.drops.iter_mut().enumerate() {
            //1120 clears private matrix, angle and state storage on start;
            //it leaves SDK data separate. Spawn writes the fields below.
            d.phase_origin=0;d.motion_origin=[0.;3];d.captured=[0.;3];
            spawn(d,i,self.image,&self.messages,rain,random);
        }
    }

    pub(super) fn update_message(&mut self,tick:u32,p:&[u8;128],offset:[f32;2],random:&mut impl ParticleRandomSource) {
        let rain=self.profile.kind==PathKind::MessageRandom;
        if rain {self.active_count=(self.active_count+1).min(self.main_count);}
        let parent=matrix::affine(self.messages.size,self.messages.angles,[word(p,0)+offset[0],word(p,1)+offset[1],0.]);
        for (i,d) in self.drops.iter_mut().take(self.active_count).enumerate() {
            if rain && d.phase!=0 {spawn(d,i,self.image,&self.messages,true,random);}
            else if d.phase==0 {
                if rain {d.alpha=255;}
                d.motion_origin[0]+=d.motion[0];
                //message2 writes the actual SDK X in addition to the private
                //matrix position; both must reach snapshot/Canvas.
                if rain {d.position[0]=d.motion_origin[0];}
                let exit=if rain {-self.image[0]} else {self.messages.exit_x};
                if exit>d.motion_origin[0] {d.phase=3;d.phase_origin=d.clock;}
            }
            let local=matrix::affine(d.captured,[0.;3],d.motion_origin);
            let combined=matrix::product(local,parent);
            d.geometry=Some(std::array::from_fn(|j|matrix::transform(combined,d.path.points[j])));
            d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn spawn(d:&mut Drop,index:usize,image:[f32;2],c:&MessageConfig,rain:bool,
    random:&mut impl ParticleRandomSource) {
    let row=if rain {random.next_integer()%9} else {c.atlas};
    d.uv=std::array::from_fn(|i|[(i&1) as f32,(row+(i>>1) as u32) as f32/32.]);
    let height=if rain {image[1]} else {image[1]*0.03125};
    d.path=Trajectory::default();
    for point in [[0.,0.,0.],[image[0],0.,0.],[0.,height,0.],[image[0],height,0.]] {d.path.append(point);}
    if rain {
        d.alpha=0;
        d.motion_origin=[(f64::from(sample(random))*100.+f64::from(c.initial[0])) as f32,
            (f64::from(sample(random))*140.+15.) as f32,0.];
        d.position=[d.motion_origin[0],d.motion_origin[1],index as f32*0.];
        d.captured=[1.,0.03125,1.];d.scale=1.;d.stretch=[1.,0.03125];
        d.motion=[-(f64::from(sample(random))*f64::from(c.speed_range)+f64::from(c.speed)) as f32,0.,0.];
    } else {
        d.motion_origin=[c.initial[0],c.initial[1],0.];d.position=[0.;3];d.captured=[1.;3];
        d.motion=[-c.speed,0.,0.];
        //Not consumed by1f30, but2490 consumes these two actual SDK draws.
        d.durations=[trunc((f64::from(sample(random))*10.+10.) as f32),0,
            trunc((f64::from(sample(random))*20.+20.) as f32)];
    }
    d.clock=0;d.phase_origin=0;d.phase=0;
}
