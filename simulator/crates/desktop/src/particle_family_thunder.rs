//! wc_thunder13e0/1510/1ab0: grouped flashes, reusable afterimages,
//! actual SDK04 completion (Yuki1b03d0 clears the task's active344).
use super::*;

//214d uses DLL CRT rand TLS, independently of both SDK MT19937 streams.
std::thread_local! {static THUNDER_CRT:std::cell::Cell<u32>=const {std::cell::Cell::new(1)};}
fn atlas_rand()->u32 {
    THUNDER_CRT.with(|state| {
        let next=state.get().wrapping_mul(0x343fd).wrapping_add(0x269ec3);
        state.set(next);(next>>16)&0x7fff
    })
}

#[derive(Debug,Default)]
pub(super) struct ThunderConfig {
    center:[f32;3],size:f32,fadein:i32,fadeout:i32,calls:u32,finished:u32,
}

impl PathFamily {
    pub(super) fn start_thunder(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;self.active_count=0;
        let size=word(p,3)/100.;
        self.thunder=ThunderConfig {center:[word(p,1),word(p,2),0.],
            size:if 0.001>size {0.001} else {size},fadein:trunc(word(p,5)),
            fadeout:trunc(word(p,6)),calls:0,finished:0};
        //1466 stores p4 (negative ->1); neither1510 nor1ab0 reads it.
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            let tile=atlas_rand()&3;
            d.uv=std::array::from_fn(|i|[(tile+(i&1) as u32) as f32*0.25,(i>>1) as f32]);
            d.pivot=self.image.map(|n|n*0.5);d.position=self.thunder.center;
            let random_half=sample(random)*0.5*self.thunder.size;
            d.scale=1.;d.stretch=[(f64::from(random_half)+f64::from(self.thunder.size)*0.7) as f32,
                self.thunder.size*3.];
            d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;d.clock=0;d.phase=0;d.phase_origin=0;
            let first=index*self.child_count;
            for (child_index,c) in children[first..first+self.child_count].iter_mut().enumerate() {
                c.clock=0;c.phase=0;c.phase_origin=0;c.pivot=d.pivot;c.uv=d.uv;
                c.position=d.position;c.position[2]+=(child_index+1) as f32;
                c.vertex_rgb=Some([0x00ff_ffff;4]);c.alpha=0;
            }
        }
    }

    pub(super) fn update_thunder(&mut self,tick:u32,offset:[f32;2]) {
        self.thunder.calls=self.thunder.calls.wrapping_add(1);
        if self.thunder.calls&1==0 {self.active_count=(self.active_count+1).min(self.main_count);}
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            //All jump-table branches and terminal9 reset XY only.
            d.position[..2].copy_from_slice(&self.thunder.center[..2]);
            if d.phase<=3 {
                let (from,to,duration)=match d.phase {
                    0=>(0.,255.,self.thunder.fadein),
                    1=>(255.,0.,self.thunder.fadeout),
                    _=>(200.,0.,self.thunder.fadeout),
                };
                d.alpha=trunc(super::super::lerp(from,to,duration as f32,elapsed as f32)) as u8;
                if elapsed>duration {
                    d.phase=match d.phase {0=>1,1=>9,2=>3,_=>2};d.phase_origin=d.clock;
                    if d.phase==9 {self.thunder.finished=self.thunder.finished.wrapping_add(1);}
                }
            }
            let first=index*self.child_count;
            let trail=&mut children[first..first+self.child_count];
            if d.phase<=3 {
                if let Some(c)=trail.iter_mut().find(|c|c.phase==0) {
                    c.pivot=d.pivot;c.position[..2].copy_from_slice(&d.position[..2]);
                    c.scale=d.scale;c.stretch=d.stretch;c.alpha=0;
                    c.phase=1;c.clock=0;c.phase_origin=0;
                }
            }
            for c in trail.iter_mut().filter(|c|c.phase!=0) {
                c.position[..2].copy_from_slice(&d.position[..2]);
                c.alpha=trunc(super::super::lerp(self.child_count as f32,0.,
                    self.child_count as f32,c.clock as i32 as f32)) as u8;
                //19cf advances one call, independently of the parent's tick.
                c.clock=c.clock.wrapping_add(1);
                if c.clock>=self.child_count as u32 {c.alpha=0;c.phase=0;}
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];
            d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(in super::super) fn finish_requested(&self)->bool {
        self.profile.kind==PathKind::Thunder && self.thunder.finished>=self.main_count as u32
    }
}
