//! Recovered hadou ring/trail emitter; SDK partition stays inside its grid.
use super::*;

#[derive(Debug,Default)]
pub(super) struct WaveConfig {period:u32,emission_clock:u32,end_size:f32,peak:i32}

impl PathFamily {
    pub(super) fn start_wave(&mut self,p: &[u8;128]) {
        let partition=trunc(word(p,0)) as u32;
        self.main_count=if partition==0 {self.drops.len()} else {self.drops.len()/partition as usize};
        self.child_count=partition.saturating_sub(1) as usize;self.active_count=0;
        self.center=[word(p,1),word(p,2),0.];
        self.scale=word(p,3)/100.;if 0.001>self.scale {self.scale=0.001;}
        let mut end_size=word(p,4)/100.;if 0.001>end_size {end_size=0.001;}
        let period=trunc(word(p,5));
        self.wave=WaveConfig {period:if period<0 {1} else {period as u32},
            emission_clock:0,end_size,peak:trunc(word(p,6))};
        self.durations=std::array::from_fn(|i|trunc(word(p,i+7)));
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().enumerate() {
            reset_wave(d,&mut children[index*self.child_count..(index+1)*self.child_count],
                self.image,self.center,self.scale);
        }
    }

    pub(super) fn update_wave(&mut self,tick:u32,offset:[f32;2]) {
        if self.wave.emission_clock%self.wave.period==0 {
            self.active_count=self.active_count.saturating_add(1).min(self.main_count);
        }
        self.wave.emission_clock=self.wave.emission_clock.wrapping_add(1);
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32;
            if d.phase>2 {reset_wave(d,trail,self.image,self.center,self.scale);}
            else if d.phase==2 {
                if elapsed>self.durations[2] {d.phase=9;d.phase_origin=d.clock;}
            } else {
                d.position[..2].copy_from_slice(&self.center[..2]);
                let size=super::super::lerp(self.scale,self.wave.end_size,
                    self.durations[0].wrapping_add(self.durations[1]) as f32,d.clock as i32 as f32);
                d.stretch=[size*1.1,size];
                d.alpha=trunc(if d.phase==0 {super::super::lerp(0.,self.wave.peak as f32,
                    self.durations[0] as f32,elapsed as f32)} else {super::super::lerp(self.wave.peak as f32,0.,
                    self.durations[1] as f32,elapsed as f32)}) as u8;
                if elapsed>self.durations[d.phase as usize] {d.phase+=1;d.phase_origin=d.clock;}
            }
            if d.phase<=3 {
                if let Some(child)=trail.iter_mut().find(|c|c.phase==0) {
                    child.pivot=d.pivot;child.position[..2].copy_from_slice(&d.position[..2]);
                    child.scale=d.scale;child.stretch=d.stretch;child.alpha=0;
                    child.phase=1;child.clock=0;child.phase_origin=0;
                }
            }
            for child in trail.iter_mut().filter(|c|c.phase!=0) {
                child.position[..2].copy_from_slice(&d.position[..2]);
                child.alpha=trunc(super::super::lerp(10.,0.,40.,child.clock as i32 as f32)) as u8;
                child.clock=child.clock.wrapping_add(1);
                // 19d6 only clears alpha, not the occupied phase. The pool
                // becomes reusable on parent respawn, not after forty calls.
                if child.clock>=40 {child.alpha=0;}
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn reset_wave(d:&mut Drop,trail:&mut [Drop],image:[f32;2],center:[f32;3],size:f32) {
    d.pivot=image.map(|n|n*0.5);d.position=center;d.scale=1.;d.stretch=[size;2];
    d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;d.clock=0;d.phase_origin=0;d.phase=0;
    for (index,c) in trail.iter_mut().enumerate() {
        c.pivot=d.pivot;c.position=center;c.position[2]+=index as f32+1.;
        c.vertex_rgb=Some([0x00ff_ffff;4]);c.alpha=0;c.clock=0;c.phase=0;
    }
}
