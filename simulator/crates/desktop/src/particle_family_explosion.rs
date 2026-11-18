//! exprode1070/1160/1660: animated 5x5 atlas, not the bang2 emitter.
use super::*;

// fe18 starts at1 and neither start nor finish resets the PE module global.
static EXPLOSION_ACTIVE: AtomicU32=AtomicU32::new(1);

#[derive(Debug,Default)]
pub(super) struct ExplosionConfig {
    center: [f32;2],
    spread: f32,
    size: f32,
    durations: [f32;3],
    total: f32,
}

impl PathFamily {
    pub(super) fn start_explosion(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        self.explosion.center=[word(p,0),word(p,1)];
        self.explosion.spread=word(p,2);self.explosion.size=word(p,3)/100.;
        self.explosion.durations=std::array::from_fn(|i| {
            let n=word(p,i+4);if n<1. {1.} else {n}
        });
        self.explosion.total=self.explosion.durations.iter().sum();
        self.main_count=self.drops.len();
        for d in &mut self.drops {
            d.pivot=self.image.map(|v|v*0.5);explosion_atlas(d,0);
            d.saved_alpha=sample(random)*self.explosion.size+self.explosion.size;
            d.scale=d.saved_alpha;d.stretch=[1.;2];d.alpha=0;
            d.position=[sample(random)*self.explosion.spread-self.explosion.spread*0.5+self.explosion.center[0],
                sample(random)*self.explosion.spread-self.explosion.spread*0.5+self.explosion.center[1],0.];
            d.phase=0;d.clock=0;d.phase_origin=0;
        }
    }

    pub(super) fn update_explosion(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        let count=EXPLOSION_ACTIVE.load(Ordering::Relaxed).wrapping_add(10).min(self.main_count as u32);
        EXPLOSION_ACTIVE.store(count,Ordering::Relaxed);self.active_count=count as usize;
        for d in self.drops.iter_mut().take(self.active_count) {
            if d.phase<=2 {
                let clock=d.clock as i32 as f32;
                let elapsed=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
                let phase=d.phase as usize;let duration=self.explosion.durations[phase];
                let ascent=if phase==2 {300.} else {5.};
                d.position[1]-=(sample(random)*ascent)/duration;
                if phase==0 {d.alpha=trunc(super::super::lerp(0.,255.,duration,clock)) as u8;}
                if phase==2 {
                    d.rotation+=sample(random)*0.3;
                    d.scale=super::super::lerp(d.saved_alpha,d.saved_alpha/10.,duration,elapsed);
                    d.alpha=trunc(super::super::lerp(255.,0.,duration,elapsed)) as u8;
                }
                let tile=super::super::lerp(0.,24.,self.explosion.total,clock);
                // SDK+50= floor(float(value+.5)), confirmed1b0b50/460670.
                explosion_atlas(d,trunc((tile+0.5).floor()) as usize);
                let age=if phase==0 {clock} else {elapsed};
                if age>duration {
                    d.phase=if phase==2 {10} else {d.phase+1};d.phase_origin=d.clock;
                    if phase==2 {d.alpha=0;}
                }
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }
}

fn explosion_atlas(d: &mut Drop,tile: usize) {
    let origin=[(tile%5) as f32*0.2,(tile/5) as f32*0.2];
    d.uv=std::array::from_fn(|i|[origin[0]+(i&1) as f32*0.2,origin[1]+(i>>1) as f32*0.2]);
}
