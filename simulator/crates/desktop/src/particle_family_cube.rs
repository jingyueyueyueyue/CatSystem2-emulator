//! o_1501011220/13d0/1d80: three six-face solids, real transformed
//! geometry/light/culling and SDK7c depth bias. No original code is executed.
use super::*;

const FACES:[[[f32;3];4];6]=[
    [[-100.,-100.,100.],[100.,-100.,100.],[-100.,100.,100.],[100.,100.,100.]],
    [[-100.,-100.,-100.],[-100.,-100.,100.],[-100.,100.,-100.],[-100.,100.,100.]],
    [[100.,-100.,-100.],[-100.,-100.,-100.],[100.,100.,-100.],[-100.,100.,-100.]],
    [[100.,-100.,100.],[100.,-100.,-100.],[100.,100.,100.],[100.,100.,-100.]],
    [[-100.,-100.,-100.],[100.,-100.,-100.],[-100.,-100.,100.],[100.,-100.,100.]],
    [[-100.,100.,100.],[100.,100.,100.],[-100.,100.,-100.],[100.,100.,-100.]],
];

#[derive(Debug,Default)]
struct Solid {from:[f32;3],to:[f32;3],position:[f32;3],size:f32,angles:[f32;3],spin:f32,duration:i32,clock:f32}
#[derive(Debug,Default)]
pub(super) struct CubeConfig {solids:[Solid;3]}

impl PathFamily {
    pub(super) fn start_cube(&mut self,p:&[u8;128]) {
        self.main_count=self.drops.len();self.active_count=self.main_count;self.child_count=0;
        for (index,solid) in self.cube.solids.iter_mut().enumerate() {
            let i=index*7;
            *solid=Solid {from:[word(p,i),word(p,i+1),0.],to:[word(p,i+2),word(p,i+3),0.],
                position:[word(p,i),word(p,i+1),0.],size:word(p,i+4)/1000.,
                angles:[0.62831855,std::f32::consts::FRAC_PI_2,0.],spin:word(p,i+5)/100.,
                duration:trunc(word(p,i+6)),clock:0.};
        }
        for (index,d) in self.drops.iter_mut().enumerate() {
            d.vertex_rgb=Some([0x00ff_ffff;4]);d.alpha=0;d.clock=0;d.phase=0;
            d.geometry=Some(FACES[index%6]);
            //SDK7c indexes0..5 only; the following twelve keep zero bias.
            d.depth_bias=if index<6 {[0.07,0.03,0.02,0.04,0.05,0.06][index]} else {0.};
        }
    }

    pub(super) fn update_cube(&mut self,tick:u32) {
        for (index,c) in self.cube.solids.iter_mut().enumerate() {
            let angle=super::super::lerp(std::f32::consts::FRAC_PI_2,0.,c.duration as f32,c.clock);
            let speed=(f64::from(angle).sin()) as f32;
            let direction=normalize_nonzero(sub(c.to,c.from));
            c.position[0]+=(direction[0]*12.)*speed;c.position[1]+=(direction[1]*12.)*speed;
            c.angles[1]+=c.spin*2.;c.angles[2]+=c.spin;
            let matrix=matrix::affine([c.size;3],c.angles,c.position);
            for (face,d) in self.drops.iter_mut().skip(index*6).take(6).enumerate() {
                let age=d.clock.wrapping_sub(d.phase_origin) as i32 as f32;
                match d.phase {
                    0=>{d.alpha=trunc(super::super::lerp(0.,255.,5.,d.clock as i32 as f32)) as u8;
                        if d.clock as i32>5 {d.phase=1;d.phase_origin=d.clock;}},
                    1=>{d.alpha=255;if age>c.duration as f32 {d.phase=2;d.phase_origin=d.clock;}},
                    2=>{d.alpha=255;if age>10. {d.phase=3;d.phase_origin=d.clock;}},
                    3=>{
                        //17af pushes10 as the target; terminal4 retains
                        //that alpha. A conventional fade-to-zero differs.
                        d.alpha=trunc(super::super::lerp(255.,10.,10.,age)) as u8;
                        if age>10. {d.phase=4;d.phase_origin=d.clock;}
                    }
                    _=>{}
                }
                let corners=FACES[face].map(|point|matrix::transform(matrix,point));
                d.geometry=Some(corners);
                let a=sub(corners[1],corners[0]);let b=sub(corners[2],corners[0]);
                let normal=normalize_nonzero([a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]);
                let shade=(normal[1]+normal[0])-normal[2];
                let color=trunc(super::super::lerp(255.,60.,2.,shade+1.)) as u32&255;
                d.vertex_rgb=Some([color|(color<<8)|(color<<16);4]);
                let mut light=sub([640.,360.,0.],c.position);light[2]+=970.;light=normalize_nonzero(light);
                let dot=(light[1]*normal[1]+light[0]*normal[0])+light[2]*normal[2];
                if dot<=0. {d.alpha=0;}
                d.clock=d.clock.wrapping_add(tick);
            }
            c.clock+=tick as i32 as f32;
        }
        //Original13d0 consumes eighteen faces and no source translation.
    }
}
