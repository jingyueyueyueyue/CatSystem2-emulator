//! rolling1090/1160/18c0: a distributed three-stage travelling sprite
//! with independent four-knot shrinking trails. No executable is loaded.
use super::*;

impl PathFamily {
    pub(super) fn start_rolling(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;
        self.active_count=self.main_count;self.stream_render_mode=1;
        self.durations=std::array::from_fn(|i|trunc(word(p,i+6)));
        self.scale=word(p,5)/100.;
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let spread=[word(p,9)/self.main_count as f32*index as f32,
                word(p,10)/self.main_count as f32*index as f32];
            d.motion_origin=[word(p,1)+spread[0],word(p,2)+spread[1],0.];
            d.motion=[word(p,3)+spread[0],word(p,4)+spread[1],0.];
            d.position=d.motion_origin;d.pivot=self.image.map(|v|v*0.5);
            d.uv=[[0.5,0.],[1.,0.],[0.5,0.5],[1.,0.5]];
            d.scale=self.scale;d.stretch=[1.;2];d.alpha=128;
            d.clock=0;d.phase_origin=0;d.phase=0;
            d.angular_step=rolling_spin(random);
            //18c0 resets private records but leaves SDK rotation intact.
            for child in &mut children[index*self.child_count..(index+1)*self.child_count] {
                child.clock=0;child.phase_origin=0;child.phase=0;
                child.angular_step=0.;child.position=d.position;child.alpha=0;
                child.pivot=d.pivot;child.uv=[[0.5,0.5],[1.,0.5],[0.5,1.],[1.,1.]];
                child.vertex_rgb=Some([0x00ff_ffff;4]);
            }
        }
    }

    pub(super) fn update_rolling(&mut self,tick:u32,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let now=d.clock as i32 as f32;
            let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            match d.phase {
                0=>{
                    d.alpha=trunc(super::super::lerp(0.,128.,word(p,6),now)) as u8;
                    if now>word(p,6) {d.phase=1;d.phase_origin=d.clock;}
                },
                1=>{
                    for axis in 0..2 {d.position[axis]=super::super::lerp(d.motion_origin[axis],d.motion[axis],word(p,7),age);}
                    d.rotation+=d.angular_step;
                    if age>word(p,7) {d.phase=2;d.phase_origin=d.clock;}
                },
                2=>{
                    d.rotation+=d.angular_step;
                    d.alpha=trunc(super::super::lerp(128.,0.,word(p,8),age)) as u8;
                    if age>word(p,8) {d.phase=3;d.phase_origin=d.clock;d.alpha=0;}
                },
                _=>{},
            }
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            if d.phase==1 {
                if let Some(child)=trail.iter_mut().find(|c|c.phase==0) {
                    child.position=d.position;child.rotation=d.rotation;
                    child.pivot=d.pivot;child.scale=d.scale*f32::from_bits(0x3ecc_cccd);
                    child.stretch=d.stretch;child.path=Trajectory::default();child.path.append(d.position);
                    let mut point=[(f64::from(sample(random))*60.+f64::from(d.position[0])-30.) as f32,
                        (f64::from(sample(random))*20.+f64::from(d.position[1])) as f32,0.];
                    child.path.append(point);
                    for _ in 0..2 {
                        point=[(f64::from(sample(random))*80.-40.+f64::from(point[0])) as f32,
                            (f64::from(sample(random))*40.-20.+f64::from(point[1])) as f32,0.];
                        child.path.append(point);
                    }
                    child.path.finish(self.child_count as f32,20.);
                    //16a8 writes the PARENT private29c. Child29c remains
                    //zero from1d00;17xx reads that child's own zero step.
                    d.angular_step=rolling_spin(random);child.phase=1;
                }
            }
            for child in trail.iter_mut().filter(|c|c.phase!=0) {
                child.scale*=f32::from_bits(0x3f7f_be77);
                child.position=child.path.at(child.clock);child.rotation+=child.angular_step;
                child.alpha=trunc(super::super::lerp(128.,0.,self.child_count as f32,child.clock as i32 as f32)) as u8;
                child.clock=child.clock.wrapping_add(tick);
                if child.clock>=self.child_count as u32 {child.clock=0;child.phase=0;child.alpha=0;}
            }
            d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn rolling_spin(random:&mut impl ParticleRandomSource)->f32 {
    (f64::from(sample(random))*f64::from(std::f32::consts::PI)-f64::from(std::f32::consts::FRAC_PI_2)) as f32
}
