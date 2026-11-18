//! ice/ice2 10b0/11f0/1940: falling folded paths and attached atlas
//! children. The second SHA supplies alpha and skips the flash stages.
use super::*;

std::thread_local! {
    static ICE_CRT:std::cell::Cell<u32>=const {std::cell::Cell::new(1)};
    static ICE2_CRT:std::cell::Cell<u32>=const {std::cell::Cell::new(1)};
}
fn crt_random(revised:bool)->u32 {
    let draw=|seed:&std::cell::Cell<u32>| {
        let next=seed.get().wrapping_mul(214013).wrapping_add(2531011);
        seed.set(next);(next>>16)&0x7fff
    };
    if revised {ICE2_CRT.with(draw)} else {ICE_CRT.with(draw)}
}

impl PathFamily {
    pub(super) fn start_ice(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;
        let revised=self.profile.kind==PathKind::Ice2;
        // ice1111 is sticky blend1; ice2 instead saves signed p6 in44.
        if !revised && trunc(word(p,6))==0 {self.stream_render_mode=1;}
        self.active_count=if trunc(word(p,3))!=0 {self.main_count/2} else {0};
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (i,d) in parents.iter_mut().enumerate() {
            spawn(d,&mut children[i*self.child_count..(i+1)*self.child_count],self.image,p,revised,random);
            if i<self.active_count {d.clock=trunc(sample(random)*word(p,1)) as u32;}
        }
    }

    pub(super) fn update_ice(&mut self,tick:u32,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        if random.next_integer()%10==0 {self.active_count=(self.active_count+2).min(self.main_count);}
        let revised=self.profile.kind==PathKind::Ice2;
        let peak=if revised {trunc(word(p,6)) as f32} else {64.};
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (i,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let trail=&mut children[i*self.child_count..(i+1)*self.child_count];
            let now=d.clock as i32 as f32;
            let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
            match d.phase {
                0=>{
                    d.position[..2].copy_from_slice(&d.path.at(d.clock)[..2]);d.alpha=trunc(peak) as u8;
                    d.rotation_xy[1]=(f64::from(d.rotation_xy[1])+f64::from(sample(random))/10.) as f32;
                    let threshold=trunc(((f64::from(word(p,1))*f64::from(0.9f32)) as f32+0.5).floor());
                    if d.clock as i32>threshold {d.phase=1;d.phase_origin=d.clock;}
                },
                1=>{
                    d.position[..2].copy_from_slice(&d.path.at(d.clock)[..2]);
                    d.alpha=fade(peak,0.,d.phase_origin as i32 as f32,word(p,1),now);
                    d.rotation_xy[1]=(f64::from(d.rotation_xy[1])+f64::from(sample(random))/10.) as f32;
                    if now>word(p,1) {
                        //144a's esp20 is the phase-origin address saved at
                        //13b9 under the five-argument interpolation block.
                        d.phase_origin=d.clock;d.phase=if revised {5} else {2};
                        d.scale*=1.5;d.rotation_xy[1]=0.;d.rotation=sample(random)*std::f32::consts::TAU;
                        d.uv=atlas([0.5,0.5]);jitter(d,random);
                    }
                },
                2=>{
                    d.alpha=fade(0.,255.,0.,5.,age);d.position[1]+=sample(random);
                    if age>5. {d.phase=3;d.phase_origin=d.clock;jitter(d,random);}
                },
                3=>{
                    d.alpha=fade(0.,255.,0.,10.,age);d.position[1]+=sample(random);
                    if age>10. {d.phase=4;d.phase_origin=d.clock;}
                },
                4=>{
                    d.alpha=fade(255.,0.,0.,20.,age);
                    if age>20. {d.phase=5;d.phase_origin=d.clock;}
                },
                _=>if trunc(word(p,4))!=0 {spawn(d,trail,self.image,p,revised,random);},
            }
            if d.phase==0 {
                if let Some(child)=trail.iter_mut().find(|c|c.phase==0) {
                    child.position=d.position;child.pivot=d.pivot;child.scale=d.scale*1.5;
                    child.stretch=d.stretch;child.phase=1;
                }
            }
            for child in trail.iter_mut().filter(|c|c.phase!=0) {
                //180e/1822 deliberately follow XY only. Z and rotations
                //retain their initialized SDK values, no source translation.
                child.position[..2].copy_from_slice(&d.position[..2]);child.alpha=32;
                child.clock=child.clock.wrapping_add(1);
                if d.phase>1 {child.clock=0;child.phase=0;child.alpha=0;}
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn atlas(origin:[f32;2])->[[f32;2];4] {
    std::array::from_fn(|i|[origin[0]+(i&1) as f32*0.5,origin[1]+(i>>1) as f32*0.5])
}
fn fade(from:f32,to:f32,start:f32,end:f32,now:f32)->u8 {
    trunc(super::super::lerp(from,to,end-start,now-start)) as u8
}
fn jitter(d:&mut Drop,random:&mut impl ParticleRandomSource) {
    for v in &mut d.position[..2] {*v=(f64::from(*v)+f64::from(sample(random))*10.-5.) as f32;}
}
fn spawn(d:&mut Drop,trail:&mut [Drop],image:[f32;2],p:&[u8;128],revised:bool,
    random:&mut impl ParticleRandomSource) {
    d.uv=atlas([((random.next_integer()&1) as f32)*0.5,0.]);d.pivot=image.map(|v|v*0.5);
    d.scale=(f64::from(sample(random))*f64::from(0.01f32)+f64::from(word(p,2))/100.) as f32;
    d.stretch=[1.;2];let _unused_color_draw=random.next_integer();d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;
    d.position=[(f64::from(sample(random))*1280.) as f32,-50.,
        ((f64::from(sample(random))*100.) as f32+word(p,5)/100.)];
    d.rotation_xy=[0.,(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32];d.rotation=0.;
    d.clock=0;d.phase=0;d.captured=d.position;d.path=Trajectory::default();d.path.append(d.position);
    let direction=normalize_nonzero([sample(random)-0.5,sample(random),0.]);
    let distance=((crt_random(revised)%500) as f64/10.+50.) as f32;
    let mut point=add(d.position,scale(direction,distance));d.path.append(point);
    //1c6d retains the FIRST normalized segment as the cone axis for all
    //eight folds; each fold independently samples both coordinate spans.
    let forward=normalize_nonzero(sub(point,d.position));
    for _ in 0..8 {
        let a=rotate_degrees(forward,-10.);let b=rotate_degrees(forward,10.);
        let widths=[f64::from(b[0]-a[0]).powf(2.).sqrt() as f32,
            f64::from(b[1]-a[1]).powf(2.).sqrt() as f32];
        let x=(f64::from(sample(random))*f64::from(widths[0])-f64::from(widths[0])*0.5) as f32;
        let y=(f64::from(sample(random))*f64::from(widths[1])-f64::from(widths[1])*0.5) as f32;
        let direction=normalize_nonzero([forward[0]+x,forward[1]+y,0.]);
        let distance=((random.next_integer()%500) as f64/10.+50.) as f32;
        point=add(point,scale(direction,distance));d.path.append(point);
    }
    d.path.finish(word(p,1),50.);
    for child in trail {
        child.clock=0;child.phase=0;child.position=[d.position[0]-40.,d.position[1]-40.,d.position[2]-word(p,5)/100.];
        child.uv=atlas([0.,0.5]);child.vertex_rgb=d.vertex_rgb;child.alpha=d.alpha;
    }
}
