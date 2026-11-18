//! spark/spark_ex10b0/1140/1390: independently recovered six-knot
//! branching paths; the EX PE changes the tangent producer, not just its name.
use super::*;

static SPARK_ACTIVE: AtomicU32=AtomicU32::new(1);

impl PathFamily {
    pub(super) fn start_spark(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        self.main_count=self.drops.len();
        let extended=self.profile.kind==PathKind::SparkEx;
        for d in &mut self.drops {
            d.clock=0;d.phase=0;d.phase_origin=0;d.path=Trajectory::default();
            d.pivot=self.image.map(|v|v*0.5);
            let atlas=random.next_integer()%3;
            let origin=[(atlas&1) as f32*0.5,(atlas>>1) as f32*0.5];
            d.uv=std::array::from_fn(|i|[origin[0]+(i&1) as f32*0.5,origin[1]+(i>>1) as f32*0.5]);
            let size=(f64::from(word(p,4))/100.) as f32;
            d.scale=(f64::from(size)*f64::from(sample(random))+f64::from(size)) as f32;
            d.stretch=[1.;2];d.alpha=0;
            d.position=[(f64::from(sample(random))*200.-100.+f64::from(word(p,1))) as f32,word(p,2),0.];
            d.path.append(d.position);
            let x=(f64::from(sample(random))*f64::from(0.3f32)-f64::from(0.15f32)) as f32;
            let y=(-(f64::from(sample(random))+f64::from(0.05f32))) as f32;
            let direction=normalize_nonzero([x,y,0.]);
            let distance=word(p,3);
            let step=(f64::from(distance)*f64::from(sample(random))+f64::from(distance)) as f32;
            let mut current=add(d.position,scale(direction,step));d.path.append(current);
            d.motion=normalize_nonzero(sub(current,d.position));
            //1742..18eb obtains two +/-50-degree bounds from the INITIAL
            //segment direction. Each later segment samples the same bounds.
            let radians=(50.*f64::from(std::f32::consts::PI)/180.) as f32;
            let (s,c)=f64::from(radians).sin_cos();let (s,c)=(s as f32,c as f32);
            let [x,y,_]=d.motion;
            let left=[c*x+s*y,c*y-s*x];let right=[c*x-s*y,c*y+s*x];
            let width=[(right[0]-left[0]).abs(),(right[1]-left[1]).abs()];
            for _ in 0..4 {
                let jitter=std::array::from_fn(|axis|if axis<2 {
                    (f64::from(width[axis])*f64::from(sample(random))-f64::from(width[axis])*0.5+f64::from(d.motion[axis])) as f32
                } else {0.});
                let direction=normalize_nonzero(jitter);
                let step=(f64::from(distance)*f64::from(sample(random))+f64::from(distance)) as f32;
                current=add(current,scale(direction,step));d.path.append(current);
                if random.next_integer()%15==0 {break;}
            }
            let tangent=if extended {(f64::from(word(p,0))*f64::from(0.55f32)) as f32} else {400.};
            d.path.finish(word(p,0),tangent);d.captured=d.position;
        }
    }

    pub(super) fn update_spark(&mut self,tick: u32,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        //1140 increments a module counter, but the actual loop uses TOTAL
        //count. Consume the real RNG call without inventing staggered drawing.
        let increment=u32::from(random.next_integer()&1==0);
        SPARK_ACTIVE.store(SPARK_ACTIVE.load(Ordering::Relaxed).wrapping_add(increment).min(self.main_count as u32),Ordering::Relaxed);
        for d in &mut self.drops {
            if d.phase==0 {
                d.position=d.path.at(d.clock);d.rotation+=sample(random);d.alpha=255;
                let limit=(f64::from(word(p,0))*f64::from(0.7f32)) as f32;
                let limit=trunc((limit+0.5).floor());
                if d.clock as i32>limit {d.phase=1;d.phase_origin=d.clock;}
            } else if d.phase==1 {
                d.position=d.path.at(d.clock);
                let origin=d.phase_origin as i32 as f32;
                d.alpha=trunc(super::super::lerp(255.,0.,word(p,0)-origin,d.clock as i32 as f32-origin)) as u8;
                if d.clock as i32 as f32>word(p,0) {d.phase=2;d.phase_origin=d.clock;}
            }
            // Phase2 retains the last actual SDK transform, without respawn.
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
