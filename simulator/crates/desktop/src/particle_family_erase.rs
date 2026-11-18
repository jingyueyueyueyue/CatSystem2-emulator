//! erase/s_0401011230/1560/1a20 (erase spawn1a20, shifted helpers):
//! two three-knot guides feed an independent, at most six-knot fading path.
//! The atlas variant alone consumes SDK integer RNG before the float stream.
use super::*;

#[derive(Debug, Default)]
pub(super) struct EraseConfig {
    guides: [Trajectory;2],
    size: f32,
    distance: f32,
    tangent: f32,
    durations: [i32;3],
}

impl PathFamily {
    pub(super) fn start_erase(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let durations=std::array::from_fn(|i|trunc(word(p,15+i)));
        let mut guides: [Trajectory;2]=std::array::from_fn(|_|Trajectory::default());
        for (guide_index,guide) in guides.iter_mut().enumerate() {
            for knot in 0..3 {
                let parameter=1+guide_index*6+knot*2;
                guide.append([word(p,parameter),word(p,parameter+1),0.]);
            }
            guide.finish(100.,if guide_index==0 {word(p,18)} else {100.});
        }
        self.erase=EraseConfig {guides,size:(f64::from(word(p,13))/100.) as f32,
            distance:word(p,14),tangent:word(p,18),durations};
        self.main_count=self.drops.len();self.child_count=0;self.active_count=self.main_count;
        let randomize=trunc(word(p,19))!=0;
        let half=if randomize {self.main_count/2} else {0};
        let total=durations[0].wrapping_add(durations[1]).wrapping_add(durations[2]);
        for (index,d) in self.drops.iter_mut().enumerate() {
            //2580 clears private state on every start; no child pool is reachable.
            d.clock=0;d.phase_origin=0;d.phase=0;d.path=Trajectory::default();
            d.pivot=self.image.map(|v|v*0.5);
            if self.profile.kind==PathKind::EraseAtlas {
                let tile=random.next_integer()&3;
                let origin=[(tile&1) as f32*0.5,(tile>>1) as f32*0.5];
                d.uv=std::array::from_fn(|i|[origin[0]+(i&1) as f32*0.5,origin[1]+(i>>1) as f32*0.5]);
            }
            d.scale=(f64::from(self.erase.size)*f64::from(sample(random))+f64::from(self.erase.size)) as f32;
            d.stretch=[1.;2];d.alpha=0;
            let at=(f64::from(sample(random))*100.) as f32;
            let a=self.erase.guides[0].at_time(at);let b=self.erase.guides[1].at_time(at);
            for axis in 0..2 {
                let now=(f64::from(sample(random))*100.) as f32;
                d.position[axis]=super::super::lerp(a[axis],b[axis],100.,now);
            }
            d.position[2]=(f64::from(sample(random))*50.) as f32;
            d.rotation=(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32;
            d.path.append(d.position);d.captured=d.position;
            let direction=normalize_nonzero([
                (f64::from(sample(random))*30.-15.) as f32,
                (f64::from(sample(random))*30.-15.) as f32,0.]);
            let distance=self.erase.distance;
            let step=(f64::from(distance)*f64::from(sample(random))+f64::from(distance)) as f32;
            let mut current=add(d.position,scale(direction,step));d.path.append(current);
            let direction=normalize_nonzero(sub(current,d.position));
            let radians=(20.*f64::from(std::f32::consts::PI)/180.) as f32;
            let (s,c)=f64::from(radians).sin_cos();let (s,c)=(s as f32,c as f32);
            let [x,y,_]=direction;
            let left=[c*x+s*y,c*y-s*x];let right=[c*x-s*y,c*y+s*x];
            let width=[(right[0]-left[0]).abs(),(right[1]-left[1]).abs()];
            for _ in 0..4 {
                let jitter=std::array::from_fn(|axis|if axis<2 {
                    let delta=(f64::from(width[axis])*f64::from(sample(random))-f64::from(width[axis])*0.5) as f32;
                    direction[axis]+delta
                } else {0.});
                let direction=normalize_nonzero(jitter);
                let step=(f64::from(distance)*f64::from(sample(random))+f64::from(distance)) as f32;
                current=add(current,scale(direction,step));d.path.append(current);
                if random.next_integer()%15==0 {break;}
            }
            d.path.finish(total as f32,self.erase.tangent);
            //1473 randomizes only the first half, AFTER that particle's spawn.
            if index<half {d.clock=(f64::from(sample(random))*f64::from(total)).trunc() as i32 as u32;}
        }
    }

    pub(super) fn update_erase(&mut self,tick: u32) {
        for d in &mut self.drops {
            if d.phase<=2 {
                d.position=d.path.at(d.clock);
                let elapsed=if d.phase==0 {d.clock as i32} else {d.clock.wrapping_sub(d.phase_origin) as i32};
                let duration=self.erase.durations[d.phase as usize];
                if d.phase!=1 {
                    let (from,to)=if d.phase==0 {(0.,128.)} else {(128.,0.)};
                    d.alpha=trunc(super::super::lerp(from,to,duration as f32,elapsed as f32)) as u8;
                }
                if elapsed>duration {d.phase+=1;d.phase_origin=d.clock;}
            }
            //Phase3 holds the actual final transform; there is no respawn or offset.
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}
