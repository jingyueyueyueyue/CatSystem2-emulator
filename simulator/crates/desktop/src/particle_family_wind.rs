//! wc_windball2480/2a10: two SDK pools with a moving Hermite center.
//! Independently recovered; the PE is read as evidence, never loaded.
use super::*;

#[derive(Debug,Default)]
pub(super) struct WindConfig {
    path: Trajectory,
    current: [f32;3],
    clock: u32,
    counters: [i32;2],
    secondary_active: usize,
    size: f32,
    peak: i32,
    render_mode: u32,
}

impl PathFamily {
    pub(super) fn start_wind(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let start=std::array::from_fn(|i|word(p,i));
        let end=std::array::from_fn(|i|word(p,i+3));
        self.wind.path=volt::volt_path(start,end,word(p,8),word(p,9),word(p,6));
        self.wind.current=start;self.wind.clock=0;self.wind.counters=[0;2];
        self.wind.secondary_active=0;self.wind.size=word(p,10)/100.;
        self.wind.peak=trunc(word(p,7));
        if word(p,11)!=0. {self.wind.render_mode=1;}
        self.main_count=self.drops.len()/2;self.active_count=1;
        for (i,d) in self.drops.iter_mut().enumerate() {
            // 1120 clears private memory, not SDK transform storage.
            spawn_wind(d,self.image,self.wind.size,i>=self.main_count,random);
        }
    }

    pub(super) fn update_wind(&mut self,tick: u32,offset: [f32;2],random: &mut impl ParticleRandomSource) {
        self.wind.current=self.wind.path.at(self.wind.clock);
        if self.wind.counters[0]>5 {
            self.active_count=(self.active_count+1).min(self.main_count);
            self.wind.counters[0]=0;
        }
        // 2fbb reads33c, not348: the secondary activation counter is
        // maintained by the PE but does NOT bound its actual update loop.
        if self.wind.counters[1]>10 {
            self.wind.secondary_active=(self.wind.secondary_active+1).min(self.drops.len().saturating_sub(1));
            self.wind.counters[1]=0;
        }
        let boundary=self.active_count.min(self.drops.len());
        let (first,second)=self.drops.split_at_mut(boundary);
        advance_wind_pool(first,self.image,&self.wind,false,tick,offset,random);
        advance_wind_pool(second,self.image,&self.wind,true,tick,offset,random);
        for counter in &mut self.wind.counters {*counter=counter.wrapping_add(1);}
        self.wind.clock=self.wind.clock.wrapping_add(tick);
    }

    pub(super) fn wind_render_mode(&self) -> u32 {self.wind.render_mode}
}

fn advance_wind_pool(pool: &mut [Drop],image: [f32;2],c: &WindConfig,second: bool,
    tick: u32,offset: [f32;2],random: &mut impl ParticleRandomSource) {
    for d in pool {
        if d.phase>2 {
            // Respawn sets the SDK XY center before3450/3780, which only
            // reset private offsets. Z remains the previously submitted Z.
            d.position[..2].copy_from_slice(&c.current[..2]);
            spawn_wind(d,image,c.size,second,random);
        } else {
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            for axis in 0..2 {d.motion_origin[axis]+=d.motion[axis];}
            d.position=add(c.current,d.motion_origin);
            let total=d.durations.iter().fold(0i32,|n,v|n.wrapping_add(*v));
            d.scale=1.;d.stretch=[super::super::lerp(c.size,c.size*if second {1.5} else {2.5},
                total as f32,d.clock as i32 as f32);2];
            d.rotation+=d.angular_step;
            if d.phase==0 {d.alpha=trunc(super::super::lerp(0.,c.peak as f32,
                d.durations[0] as f32,elapsed as f32)) as u8;}
            if d.phase==2 {d.alpha=trunc(super::super::lerp(c.peak as f32,0.,
                d.durations[2] as f32,elapsed as f32)) as u8;}
            if elapsed>d.durations[d.phase as usize] {d.phase+=1;d.phase_origin=d.clock;}
        }
        d.position[0]+=offset[0];d.position[1]+=offset[1];
        d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
    }
}

fn spawn_wind(d: &mut Drop,image: [f32;2],size: f32,second: bool,
    random: &mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);
    let x=if second {0.5} else {0.};
    d.uv=std::array::from_fn(|i|[x+(i&1) as f32*0.5,(i>>1) as f32*0.5]);
    d.scale=1.;d.stretch=[size;2];
    // Second spawn overwrites this angle but consumes the same MT sample.
    d.rotation=sample(random)*std::f32::consts::TAU;
    d.angular_step=if second {0.005} else {0.04};
    let x=(sample(random)*2.-1.)*sample(random);
    let y=(sample(random)*2.-1.)*sample(random);
    let direction=normalize_nonzero([x,y,0.]);
    if second {
        let angle=f64::from(-direction[1]).acos() as f32;
        d.rotation=(if -direction[0]>0. {-angle} else {angle})+std::f32::consts::FRAC_PI_2;
        d.motion_origin=direction.map(|v|(v*200.)*size);
    } else {d.motion_origin=[0.;3];}
    d.motion=scale(direction,sample(random)*0.1+if second {0.1} else {0.2});
    d.clock=0;d.phase_origin=0;d.phase=0;
    let ranges=if second {[[20.,40.],[40.,80.]]} else {[[15.,30.],[20.,60.]]};
    d.durations=[trunc((sample(random)*2.-1.)*ranges[0][0]+ranges[0][1]),
        0,trunc((sample(random)*2.-1.)*ranges[1][0]+ranges[1][1])];
    d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;
}
