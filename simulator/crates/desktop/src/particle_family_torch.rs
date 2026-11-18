//! wc_torch2050/21c0/2720/3150, SDK2.1. Uses the existing distance-timed
//! trajectory and real texture/Canvas consumers; never loads original code.
use super::*;

#[derive(Debug,Default)]
pub(super) struct TorchConfig {center:[f32;3],size:f32,step:f32,durations:[i32;3]}

fn spawn(d:&mut Drop,c:&TorchConfig,image:[f32;2],random:&mut impl ParticleRandomSource) {
    d.pivot=image.map(|v|v*0.5);
    let tile=random.next_integer()&3;
    d.uv=std::array::from_fn(|i|[((tile&1)+(i&1) as u32) as f32*0.5,
        ((tile>>1)+(i>>1) as u32) as f32*0.5]);
    d.scale=sample(random)*c.size+c.size;d.stretch=[1.;2];
    d.alpha=0;d.vertex_rgb=None;d.position=c.center;
    d.rotation=sample(random)*std::f32::consts::PI;
    d.clock=0;d.phase=0;d.captured=d.position;
    //294d..2982 reads object64/68, but neither1f60 nor2050 initializes
    //them and no public setter writes them. The independent instance gives
    //those unspecified fields zero storage, while retaining its RNG read.
    //This boundary is recorded; no claim of allocator-dependent equivalence.
    let _uninitialized_spin_sample=sample(random);d.angular_step=0.;
    let _unused_direction_sample=sample(random);
    let radians=(50.*f64::from(std::f32::consts::PI)/180.) as f32;
    let (sine,cosine)=f64::from(radians).sin_cos();
    let width=[((sine as f32)-(-(sine as f32))).abs(),
        ((-(cosine as f32))-(-(cosine as f32))).abs()];
    let dx=sample(random)*width[0]-width[0]*0.5;
    let dy=(sample(random)*width[1]-width[1]*0.5)-1.;
    let direction=normalize_nonzero([dx,dy,0.]);
    let distance=sample(random)*c.step+c.step;
    let delta=scale(direction,distance);
    //2d00..2ee0: advance, reverse X, then two .3-sized reversed steps.
    let mut point=c.center;d.path=Trajectory::default();d.path.append(point);
    for step in [delta,[-delta[0],delta[1],delta[2]],
        [delta[0]*0.3,delta[1]*0.3,delta[2]],[-delta[0]*0.3,delta[1]*0.3,delta[2]]] {
        point=add(point,step);d.path.append(point);
    }
    let duration=c.durations[0].wrapping_add(c.durations[1]).wrapping_add(c.durations[2]);
    d.path.finish(duration as f32,30.);
}

impl PathFamily {
    pub(super) fn start_torch(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let size=word(p,2)/100.;
        self.torch=TorchConfig {center:[word(p,0),word(p,1),0.],
            size:if 0.01>size {0.01} else {size},step:word(p,3),
            durations:std::array::from_fn(|i|trunc(word(p,4+i)).max(0))};
        self.main_count=self.drops.len();self.child_count=0;self.active_count=self.main_count/2;
        let c=&self.torch;
        for (index,d) in self.drops.iter_mut().enumerate() {
            d.phase_origin=0;spawn(d,c,self.image,random);
            if index<self.active_count {
                let total=c.durations[0].wrapping_add(c.durations[1]).wrapping_add(c.durations[2]);
                d.clock=trunc(sample(random)*total as f32) as u32;
                if d.clock as i32>c.durations[0] {
                    d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(c.durations[0] as u32);d.alpha=70;
                }
                //31b9 compares the absolute clock against hold alone.
                if d.clock as i32>c.durations[1] {
                    d.phase+=1;d.phase_origin=d.phase_origin.wrapping_add(c.durations[1] as u32);
                }
            }
        }
    }

    pub(super) fn update_torch(&mut self,tick:u32,offset:[f32;2],random:&mut impl ParticleRandomSource) {
        //2201 still requests an integer even though divisor54 is one.
        let _activation=random.next_integer();
        self.active_count=self.active_count.saturating_add(1).min(self.main_count);
        let c=&self.torch;
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase>2 {spawn(d,c,self.image,random);}
            else {
                let now=d.clock as i32;let age=d.clock.wrapping_sub(d.phase_origin) as i32;
                d.position=d.path.at(d.clock);d.rotation+=d.angular_step;
                match d.phase {
                    0=> {
                        d.alpha=trunc(super::super::lerp(0.,70.,c.durations[0] as f32,now as f32)) as u8;
                        if now>c.durations[0] {d.phase=1;d.phase_origin=d.clock;}
                    },
                    1=>if age>c.durations[1] {d.phase=2;d.phase_origin=d.clock;},
                    _=> {
                        d.alpha=trunc(super::super::lerp(70.,0.,c.durations[2] as f32,age as f32)) as u8;
                        if age>c.durations[2] {d.phase=3;d.phase_origin=d.clock;}
                    },
                }
            }
            //1120 sets every childCount to zero:24a7..26a5 is unreachable.
            d.position[0]+=offset[0];d.position[1]+=offset[1];
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
