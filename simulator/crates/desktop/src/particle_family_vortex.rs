//! wc_vortex26a0/29b0/32f0: continuous eleven-knot ribbons, owned
//! grouped geometry, cosine scale cycle and real cumulative UV consumption.
use super::*;

#[derive(Debug,Default)]
pub(super) struct VortexConfig {
    center:[f32;3],size:[f32;3],alpha:i32,rotation_period:f32,uv_step:f32,
    reverse:bool,base:Vec<[[f32;3];4]>,
}

fn cross(a:[f32;3],b:[f32;3])->[f32;3] {
    [a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]
}

fn breathing(phase:u32,clock:u32)->[f32;3] {
    let(from,to,duration)=if phase==0 {([1.;3],[1.05,0.95,1.05],180.)}
        else {([1.05,0.95,1.05],[1.;3],220.)};
    let now=(clock as i32 as f32).max(1.);
    if (trunc(now) as u32) as f32>=duration {return to;}
    let pi=std::f32::consts::PI;let half=std::f32::consts::FRAC_PI_2;
    let ax=super::super::lerp(pi,0.,duration,now);
    let ay=super::super::lerp(half,pi+half,duration,now);
    let x=f64::from(ax).cos() as f32-f64::from(pi).cos() as f32;
    let y=f64::from(ay).sin() as f32-f64::from(half).sin() as f32;
    std::array::from_fn(|i|from[i]+if i==1 {-y*((to[i]-from[i])*0.5)} else {x*((to[i]-from[i])*0.5)})
}

impl PathFamily {
    pub(super) fn start_vortex(&mut self,p:&[u8;128]) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;self.active_count=self.main_count;
        let c=&mut self.vortex;
        c.center=[word(p,1),word(p,2),0.];
        c.size=std::array::from_fn(|i|{let s=word(p,i+3)/100.;if 0.001>s {0.001} else {s}});
        c.alpha=trunc(word(p,6));c.rotation_period=word(p,7);c.uv_step=word(p,8)/1000.;c.reverse=word(p,9)!=0.;
        if word(p,10)!=0. {self.stream_render_mode=1;}
        let mut path=Trajectory::default();
        for point in [[0.,100.,0.],[1.,80.,4.],[3.,60.,12.],[6.,40.,24.],[10.,20.,40.],
            [15.,0.,60.],[21.,-20.,76.],[28.,-40.,88.],[36.,-60.,96.],[45.,-80.,100.],[55.,-100.,102.]] {
            path.append(point);
        }
        path.finish_average(word(p,0)-1.);
        let orientation=matrix::affine([1.;3],if c.reverse {[0.,0.,std::f32::consts::FRAC_PI_2]}
            else {[0.,std::f32::consts::PI+std::f32::consts::FRAC_PI_2,0.]},[0.;3]);
        c.base.clear();
        for i in 0..self.child_count {
            let p=path.at_time(i as f32);let q=path.at_time((i+1) as f32);
            let forward=normalize_nonzero(sub(q,p));
            let sideways=matrix::transform(orientation,forward);
            //37d0 computes a cross product into temporary storage, but its
            //caller keeps xmm2/3/4 (the transformed forward), not that result.
            let _unused=cross(sideways,forward);let side=scale(sideways,20.);
            let mut vertices=[sub(p,side),sub(q,side),add(p,side),add(q,side)];
            if let Some(previous)=c.base.last() {vertices[0]=previous[1];vertices[2]=previous[3];}
            else {let center=scale(add(vertices[0],vertices[2]),0.5);vertices[0]=center;vertices[2]=center;}
            if i+1==self.child_count {
                let center=scale(add(vertices[1],vertices[3]),0.5);vertices[1]=center;vertices[3]=center;
            }
            c.base.push(vertices);
        }
        let(parents,children)=self.drops.split_at_mut(self.main_count);
        for(index,d) in parents.iter_mut().enumerate() {
            d.alpha=0;d.vertex_rgb=Some([0x00ff_ffff;4]);d.motion=[0.,std::f32::consts::TAU/self.main_count as f32*index as f32,0.];
            d.angular_step=std::f32::consts::TAU/c.rotation_period*if c.reverse {-1.} else {1.};
            d.captured=c.size;d.phase=0;d.clock=0;d.phase_origin=0;
            for(i,child) in children[index*self.child_count..index*self.child_count+self.child_count].iter_mut().enumerate() {
                child.motion[0]=1.-(1./self.child_count as f32)*i as f32;
                child.uv=std::array::from_fn(|v|[child.motion[0]+(v&1) as f32/self.child_count as f32,(v>>1) as f32*0.5]);
                child.pivot=[0.;2];child.position=[0.;3];child.scale=1.;child.stretch=[1.;2];child.clock=0;
            }
        }
    }

    pub(super) fn update_vortex(&mut self,tick:u32) {
        let c=&self.vortex;let(parents,children)=self.drops.split_at_mut(self.main_count);
        for(index,d) in parents.iter_mut().enumerate() {
            if d.phase<=1 {
                d.motion[1]+=d.angular_step;
                let age=d.clock.wrapping_sub(d.phase_origin);
                d.captured=breathing(d.phase,age);
                let duration=if d.phase==0 {180} else {220};
                if age as i32>duration {d.phase^=1;d.phase_origin=d.clock;}
            }
            let transform=matrix::affine(d.captured,d.motion,c.center);
            let pool=&mut children[index*self.child_count..index*self.child_count+self.child_count];
            for(i,child) in pool.iter_mut().enumerate() {
                let mut alpha=[c.alpha as u8;4];
                let half=trunc(c.alpha as f32*0.5) as u8;let edge=trunc(c.alpha as f32*0.2) as u8;
                match i {0=>alpha=[0,edge,0,edge],1=>alpha=[edge,half,edge,half],2=>alpha=[half,c.alpha as u8,half,c.alpha as u8],_=>{}}
                if i==self.child_count.wrapping_sub(3) {alpha=[c.alpha as u8,half,c.alpha as u8,half];}
                if i==self.child_count.wrapping_sub(2) {alpha=[half,edge,half,edge];}
                if i==self.child_count.wrapping_sub(1) {alpha=[edge,0,edge,0];}
                child.vertex_alpha=Some(alpha);child.vertex_rgb=Some([0x00ff_ffff;4]);
                child.geometry=Some(c.base[i].map(|v|matrix::transform(transform,v)));
                child.motion[0]+=c.uv_step;if child.motion[0]>=1. {child.motion[0]=0.;}
                let u=child.motion[0];let next=u-1./self.child_count as f32;
                child.uv=[[u,0.],[next,0.],[u,1.],[next,1.]];
                child.clock=child.clock.wrapping_add(tick);
            }
            d.clock=d.clock.wrapping_add(tick);
        }
        //29b0 computes source offset but has no consumer of318/31c. There
        //is no fabricated source translation, random draw or SDK04 finish.
    }
}
