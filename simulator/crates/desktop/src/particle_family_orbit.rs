//! wc_mirror13a0/1530/1b20: colored radial mirrors, three age stages.
use super::*;

#[derive(Debug,Default)]
pub(super) struct OrbitConfig {
    center: [f32;3],
    tilt: f32,
    size: [f32;2],
    radius: f32,
    peak: i32,
    durations: [i32;3],
    render_mode: u32,
}

impl PathFamily {
    pub(super) fn start_orbit(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        self.orbit.center=[word(p,0),word(p,1),0.];
        let tilt=((f64::from(word(p,3))*f64::from(std::f32::consts::PI))/180.) as f32;
        self.orbit.tilt=if tilt<0. {0.} else if tilt<std::f32::consts::FRAC_PI_2 {tilt} else {std::f32::consts::FRAC_PI_2};
        self.orbit.size=[word(p,6)/1000.,word(p,7)/1000.];
        self.orbit.radius=word(p,8);self.orbit.peak=trunc(word(p,9));
        self.orbit.durations=std::array::from_fn(|i|trunc(word(p,i+10)));
        if trunc(word(p,13))!=0 {self.orbit.render_mode=1;}
        // p2, p4 and p5 are retained by13a0 but its producer/consumer do
        // not read those fields. No invented movement from those values.
        self.main_count=self.drops.len();self.active_count=0;
        for d in &mut self.drops {spawn_orbit(d,self.image,&self.orbit,random);}
    }

    pub(super) fn update_orbit(&mut self,tick: u32,offset: [f32;2],random: &mut impl ParticleRandomSource) {
        self.active_count=self.active_count.saturating_add(1).min(self.main_count);
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase>2 {spawn_orbit(d,self.image,&self.orbit,random);}
            else {
                let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
                d.motion[0]+=d.angular_step;
                let (sin,cos)=f64::from(d.motion[0]).sin_cos();
                d.position=[self.orbit.center[0]-(cos as f32*d.motion_limit),
                    self.orbit.center[1],sin as f32*d.motion_limit];
                let inward=normalize_nonzero(sub(self.orbit.center,d.position));
                let yaw=f64::from(-inward[2]).acos() as f32;
                d.rotation_xy=[-self.orbit.tilt,if inward[0]>0. {-yaw} else {yaw}];
                if d.phase==0 {d.alpha=trunc(super::super::lerp(0.,d.saved_alpha,
                    d.durations[0] as f32,elapsed as f32)) as u8;}
                if d.phase==2 {d.alpha=trunc(super::super::lerp(d.saved_alpha,0.,
                    d.durations[2] as f32,elapsed as f32)) as u8;}
                if elapsed>d.durations[d.phase as usize] {d.phase+=1;d.phase_origin=d.clock;}
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn orbit_render_mode(&self) -> u32 {self.orbit.render_mode}
}

fn spawn_orbit(d: &mut Drop,image: [f32;2],c: &OrbitConfig,random: &mut impl ParticleRandomSource) {
    d.pivot=[image[0]*0.5,image[1]];
    let size=(sample(random)*2.-1.)*c.size[1]+c.size[0];
    d.scale=1.;d.stretch=[size,size*6.];
    let mut rgb=d.vertex_rgb.unwrap_or([0x00ff_ffff;4]);
    for vertex in &mut rgb[..2] {
        let bright=random.next_integer()%3;
        let channels: [u32;3]=std::array::from_fn(|i| {
            let (divisor,base)=if i as u32==bright {(31,225)} else {(30,0)};
            random.next_integer()%divisor+base
        });
        *vertex=channels[0]|(channels[1]<<8)|(channels[2]<<16);
    }
    d.vertex_rgb=Some(rgb);d.alpha=0;
    let angle=((f64::from(sample(random)*360.)*f64::from(std::f32::consts::PI))/180.) as f32;
    d.motion[0]=trunc(angle*1000.) as f32/1000.;
    d.angular_step=(sample(random)*2.-1.)*(sample(random)*0.01);
    d.motion_limit=((sample(random)*2.-1.)*c.radius)*0.5+c.radius;
    let peak=(sample(random)*2.-1.)*c.peak as f32+c.peak as f32;
    d.saved_alpha=if peak<0. {0.} else if peak<255. {trunc(peak) as f32} else {255.};
    d.clock=0;d.phase_origin=0;d.phase=0;
    d.durations=std::array::from_fn(|i| {
        let n=c.durations[i] as f32;trunc(((sample(random)*2.-1.)*n)*0.5+n)
    });
}
