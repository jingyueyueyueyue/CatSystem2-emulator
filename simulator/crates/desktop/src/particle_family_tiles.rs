//! s_0801031070/1210/16b0: four atlas pieces, affine XYZ spin and
//! SDK+7c depth bias. Uses the shared true quad transform/snapshot pipeline.
use super::*;

#[derive(Debug,Default)]
pub(super) struct TilesConfig {
    center: [f32;3],
    rotation_xy: [f32;2],
    speed: f32,
    peak: f32,
    duration: f32,
    render_mode: u32,
}

impl PathFamily {
    pub(super) fn start_tiles(&mut self,p: &[u8;128]) {
        self.tiles.center=[word(p,0),word(p,1),0.];
        self.tiles.rotation_xy=[word(p,4),word(p,5)].map(tile_radians);
        self.tiles.speed=word(p,6)/100.;self.tiles.peak=word(p,3);
        self.tiles.duration=word(p,7);self.tiles.render_mode=u32::from(trunc(word(p,8))!=0);
        let layers=if self.tiles.render_mode!=0 {[0.,0.01,0.02,0.03]} else {[0.001,0.003,0.,0.002]};
        let [w,h]=self.image;
        for (i,d) in self.drops.iter_mut().enumerate() {
            d.uv=std::array::from_fn(|v|[(i&1) as f32*0.5+(v&1) as f32*0.5,
                (i>>1) as f32*0.5+(v>>1) as f32*0.5]);
            d.geometry=Some([[-w*0.5,-h*0.5,0.],[w*0.5,-h*0.5,0.],
                [-w*0.5,h*0.5,0.],[w*0.5,h*0.5,0.]]);
            d.position=self.tiles.center;d.pivot=[0.;2];d.scale=1.;d.stretch=[1.;2];
            d.rotation=0.;d.rotation_xy=[0.;2];d.motion=[0.;3];d.motion_limit=word(p,2)/100.;
            d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;d.clock=0;d.phase=0;
            d.depth_bias=layers[i];
        }
    }

    pub(super) fn update_tiles(&mut self,tick: u32) {
        let rates=[2.,1.,-2.,-1.];
        for (i,d) in self.drops.iter_mut().enumerate() {
            // Private phase8 is never advanced by1210. There is no implied
            // one-shot completion or reset at the end of the alpha ramp.
            d.alpha=trunc(super::super::lerp(0.,self.tiles.peak,self.tiles.duration,d.clock as i32 as f32)) as u8;
            d.motion[0]+=rates[i]*self.tiles.speed;
            d.rotation=trunc(d.motion[0]*1000.) as f32/1000.;
            d.rotation_xy=self.tiles.rotation_xy;d.scale=d.motion_limit;
            d.position=self.tiles.center;d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn tiles_render_mode(&self) -> u32 {self.tiles.render_mode}
}

fn tile_radians(degrees: f32) -> f32 {
    let radians=((f64::from(degrees)*f64::from(std::f32::consts::PI))/180.) as f32;
    trunc(radians*1000.) as f32/1000.
}
