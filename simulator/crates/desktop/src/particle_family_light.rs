//! wc_light1540/1660/1710/1bb0/2560/2710. Helper1120/1171
//! explicitly creates zero children: embedded segment-pool code is unreachable.
use super::*;

#[derive(Debug,Default)]
pub(super) struct LightConfig {center:[f32;2],size:f32,speed:f32,durations:[i32;4]}

fn spawn_light(d:&mut Drop,main:bool,c:&LightConfig,image:[f32;2],count:usize,
    random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);d.scale=1.;d.alpha=0;d.vertex_rgb=Some([0x00ff_ffff;4]);
    if main {
        d.uv=[[0.,0.],[0.5,0.],[0.,0.5],[0.5,0.5]];d.stretch=[c.size;2];
        //2646 consumes the integer angle, then discards SDK10's result.
        let _=random.next_integer()%count as u32;
        d.motion_origin=[c.center[0],c.center[1],0.];
    } else {
        let tile=random.next_integer()%3;
        let uv=[((tile+1)&1) as f32*0.5,((tile+1)>>1) as f32*0.5];
        d.uv=std::array::from_fn(|i|[uv[0]+(i&1) as f32*0.5,uv[1]+(i>>1) as f32*0.5]);
        d.stretch=[c.size*0.5,c.size*0.4];
        let angle=(random.next_integer()%count as u32) as f32*(360./count as f32);
        let angle=(f64::from(angle)*f64::from(std::f32::consts::PI)/180.) as f32;
        let angle=trunc(angle*1000.) as f32/1000.;
        let (sin,cos)=f64::from(angle).sin_cos();
        d.motion_origin=[(sin as f32*40.)*c.speed+c.center[0],
            (cos as f32*40.)*c.speed+c.center[1],sample(random)*50.];
        d.motion=normalize_nonzero([d.motion_origin[0]-c.center[0],d.motion_origin[1]-c.center[1],0.]);
        let angle=f64::from(-d.motion[1]).acos() as f32;
        d.rotation=if -d.motion[0]>0. {-angle} else {angle};
    }
    d.position=d.motion_origin;d.atlas_frame=0;d.clock=0;d.phase=0;d.captured=d.position;
    //Neither spawn resets phase_origin. Parameter changes capture the
    //current alpha/size but keep that original relative clock origin too.
}

fn light_toggle(d:&mut Drop,mode:u32) {
    if d.atlas_frame!=mode {d.phase=3;d.saved_alpha=f32::from(d.alpha);
        d.motion_limit=d.stretch[0];d.atlas_frame=mode;}
}

fn light_main(d:&mut Drop,c:&LightConfig) {
    let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
    let phase=d.phase;
    let (from,to,start_size,end_size,duration)=match phase {
        0=>(0.,230.,c.size,c.size*1.2,c.durations[0]),
        1=>(230.,160.,c.size*1.2,c.size*1.1,c.durations[1]),
        2=>(160.,230.,c.size*1.1,c.size*1.2,c.durations[2]),
        3=>(d.saved_alpha,0.,d.motion_limit,c.size*0.1,c.durations[3]),
        _=>return,
    };
    d.alpha=trunc(super::super::lerp(from,to,duration as f32,elapsed as f32)) as u8;
    d.position[..2].copy_from_slice(&c.center);
    let size=super::super::lerp(start_size,end_size,duration as f32,elapsed as f32);
    d.stretch=[size;2];
    if phase<3 && elapsed>duration {d.phase=if phase==1 {2} else {1};d.phase_origin=d.clock;}
}

impl PathFamily {
    pub(super) fn start_light(&mut self,p:&[u8;128],viewport:[u32;2],random:&mut impl ParticleRandomSource) {
        self.light=LightConfig {center:[word(p,0),word(p,1)],size:word(p,2)/100.,speed:word(p,3)/100.,
            durations:std::array::from_fn(|i|trunc(word(p,4+i)))};
        self.main_count=self.drops.len();self.child_count=0;self.active_count=1;
        self.viewport_half=viewport.map(|v|v as f32*0.5);
        for (index,d) in self.drops.iter_mut().enumerate() {
            spawn_light(d,index==0,&self.light,self.image,self.main_count,random);
        }
    }

    pub(super) fn update_light(&mut self,tick:u32,p:&[u8;128],offset:[f32;2],random:&mut impl ParticleRandomSource) {
        self.active_count=self.active_count.wrapping_add(2).min(self.main_count);
        let c=&self.light;let mode=trunc(word(p,8)) as u32;
        for (index,d) in self.drops.iter_mut().take(self.active_count).enumerate() {
            light_toggle(d,mode);
            if index==0 {
                if d.phase>3 {spawn_light(d,true,c,self.image,self.main_count,random);}
                else {light_main(d,c);}
            } else {
                let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
                if matches!(d.phase,0|1|3) {
                    let now=if d.phase==0 {d.clock as i32} else {elapsed};
                    let (from,to,duration)=match d.phase {0=>(0.,64.,10),1=>(64.,0.,60),_=>(d.saved_alpha,0.,c.durations[3])};
                    d.alpha=trunc(super::super::lerp(from,to,duration as f32,now as f32)) as u8;
                    if d.phase==0 {
                        if let Some(colors)=&mut d.vertex_rgb {for color in colors {
                            *color=(0..3).fold(0,|rgb,axis|rgb|((trunc(((*color>>(axis*8))&255) as f32*0.985) as u32&255)<<(axis*8)));
                        }}
                    }
                    for axis in 0..2 {d.motion_origin[axis]+=d.motion[axis]*c.speed;}
                    d.position[..2].copy_from_slice(&d.motion_origin[..2]);
                    d.stretch=match d.phase {
                        0=>[super::super::lerp(c.size*0.5,c.size*0.3,30.,now as f32),
                            super::super::lerp(c.size*0.4,c.size*0.2,30.,now as f32)],
                        1=>[super::super::lerp(c.size*0.3,c.size*0.1,60.,now as f32),
                            super::super::lerp(c.size*0.2,c.size*0.1,60.,now as f32)],
                        _=>[super::super::lerp(d.motion_limit,c.size*0.1,c.durations[3] as f32,now as f32);2],
                    };
                    if d.phase==0 && now>30 {d.phase=1;d.phase_origin=d.clock;}
                    else if d.phase==1 && now>60 {d.phase=2;d.phase_origin=d.clock;}
                } else {spawn_light(d,false,c,self.image,self.main_count,random);}
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
