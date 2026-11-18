//! roundy/roundz1ce0/1e30/2780: five-record groups, four actual
//! textured faces, local/global affine matrices and per-face normal shading.
//! Fixed tables are PE data, independently interpreted; no original code runs.
use super::*;

#[derive(Clone,Debug,Default)]
struct RoundParticle {
    clock: f32, origin: f32, phase: u32, angle: f32, speed: f32,
    position: [f32;3], size: [f32;3], alpha: u8, durations: [i32;3],
}
#[derive(Debug,Default)]
pub(super) struct RoundConfig {
    center: [f32;3], scale: f32, particles: Vec<RoundParticle>,
}

//18290 /181a0: four quads, not a texture-sized replacement rectangle.
const ROUND_Y: [[[f32;3];4];4]=[
    [[-100.,45.,-51.],[100.,45.,-51.],[-100.,50.,-33.],[100.,50.,-33.]],
    [[-100.,50.,-33.],[100.,50.,-33.],[-100.,52.,0.],[100.,52.,0.]],
    [[-100.,52.,0.],[100.,52.,0.],[-100.,50.,33.],[100.,50.,33.]],
    [[-100.,50.,33.],[100.,50.,33.],[-100.,45.,51.],[100.,45.,51.]],
];
const ROUND_Z: [[[f32;3];4];4]=[
    [[-125.,5.,0.],[-75.,-15.,0.],[-115.,55.,0.],[-75.,35.,0.]],
    [[-75.,-15.,0.],[-25.,-25.,0.],[-75.,35.,0.],[-25.,25.,0.]],
    [[-25.,-25.,0.],[25.,-25.,0.],[-25.,25.,0.],[25.,25.,0.]],
    [[25.,-25.,0.],[75.,-15.,0.],[25.,25.,0.],[75.,35.,0.]],
];

impl PathFamily {
    pub(super) fn start_round(&mut self,p: &[u8;128],random: &mut impl ParticleRandomSource) {
        let z=self.profile.kind==PathKind::RoundZ;
        self.round.center=[word(p,0),word(p,1),0.];
        let size=word(p,2)/100.;self.round.scale=if 0.001>size {0.001} else {size};
        self.main_count=self.drops.len()/5;
        self.round.particles=vec![RoundParticle::default();self.main_count];
        for i in 0..self.main_count {
            round_spawn(&mut self.round.particles[i],z,random);
            self.drops[i].alpha=0;
            for face in 0..4 {
                let d=&mut self.drops[self.main_count+i*4+face];
                //2780 copies the fixed face through its freshly reset
                //identity matrix before the first update; child alpha survives.
                d.geometry=Some(if z {ROUND_Z[face]} else {ROUND_Y[face]});d.pivot=[0.;2];d.scale=1.;
                d.stretch=[1.;2];d.position=[0.;3];d.rotation=0.;d.rotation_xy=[0.;2];
                d.uv=round_uv(face,z);
            }
        }
    }

    pub(super) fn update_round(&mut self,tick: u32,random: &mut impl ParticleRandomSource) {
        let z=self.profile.kind==PathKind::RoundZ;
        for (i,state) in self.round.particles.iter_mut().enumerate() {
            if state.phase>2 {round_spawn(state,z,random);} else {
                let age=state.clock-state.origin;
                let phase=state.phase as usize;
                let (gx,gy)=if phase==2 {(1.005,1.001)} else {(1.001,1.005)};
                state.position[0]*=gx;state.position[1]*=gy;
                if phase==2 {state.speed*=0.98;}
                state.angle+=state.speed;
                if phase!=1 {
                    let (from,to)=if phase==0 {(0.,100.)} else {(100.,0.)};
                    state.alpha=trunc(super::super::lerp(from,to,state.durations[phase] as f32,age)) as u8;
                }
                if age>state.durations[phase] as f32 {state.phase=if phase==2 {9} else {state.phase+1};state.origin=state.clock;}
            }
            let local_angles=if z {[-std::f32::consts::FRAC_PI_2,0.,0.]} else {[-0.1,std::f32::consts::PI,f32::from_bits(0x3f89_0fdb)]};
            let local=round_matrix(state.size,local_angles,state.position);
            let global=round_matrix([self.round.scale,self.round.scale,1.],[0.,state.angle,0.],self.round.center);
            let combined=round_multiply(global,local);
            for face in 0..4 {
                let raw=if z {ROUND_Z[face]} else {ROUND_Y[face]};
                let vertices=raw.map(|point|std::array::from_fn(|axis|
                    ((combined[axis][0]*point[0]+combined[axis][1]*point[1])+combined[axis][2]*point[2])+combined[axis][3]));
                let a=sub(vertices[1],vertices[0]);let b=sub(vertices[2],vertices[0]);
                let normal=normalize_nonzero([a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]);
                let light=(normal[0]+normal[1])-normal[2];
                let level=trunc(super::super::lerp(255.,60.,2.,light+1.)) as u8;
                let rgb=u32::from(level)*0x0001_0101;
                let d=&mut self.drops[self.main_count+i*4+face];
                d.geometry=Some(vertices);d.vertex_rgb=Some([rgb;4]);d.alpha=state.alpha;
                // Geometry is already the composed world result, so shared
                //snapshot submits it once without a second SDK transform.
                d.position=[0.;3];d.scale=1.;d.pivot=[0.;2];d.rotation=0.;d.rotation_xy=[0.;2];
            }
            //1e30 consumes integer delta as float, not frame*1000/60.
            state.clock+=tick as i32 as f32;
        }
    }
}

fn round_spawn(d: &mut RoundParticle,z: bool,random: &mut impl ParticleRandomSource) {
    d.clock=0.;d.origin=0.;d.phase=0;d.alpha=0;
    d.angle=(f64::from(sample(random))*f64::from(std::f32::consts::PI)*2.) as f32;
    let radius=(f64::from(sample(random))*100.) as f32;
    let ratio=radius/100.;
    d.position=if z {[0.,-radius,ratio*100.]} else {[ratio*100.,-radius,0.]};
    let size=ratio*0.3+0.7;d.size=if z {[0.6,1.,1.]} else {[size;3]};
    d.speed=(sample(random)*if z {0.05} else {0.1}+0.01)*(ratio*0.5+0.5);
    d.durations=std::array::from_fn(|phase| {
        let base=[10.,5.,40.][phase];trunc((sample(random)-0.5)*base+base)
    });
}

fn round_uv(face: usize,z: bool)->[[f32;2];4] {
    if z {
        let fifths=[0.,0.2,0.4,0.6,0.8];
        std::array::from_fn(|i|[fifths[face+(i&1)],(i>>1) as f32])
    } else {
        let rows=[0.05,0.25,0.5,0.75,0.95];
        std::array::from_fn(|i|[if i&1==0 {0.2} else {0.8},rows[face+(i>>1)]])
    }
}

type RoundMatrix=[[f32;4];4];
fn round_multiply(a: RoundMatrix,b: RoundMatrix)->RoundMatrix {
    std::array::from_fn(|row|std::array::from_fn(|col|
        ((a[row][0]*b[0][col]+a[row][1]*b[1][col])+a[row][2]*b[2][col])+a[row][3]*b[3][col]))
}
fn round_matrix(size: [f32;3],angles: [f32;3],position: [f32;3])->RoundMatrix {
    let mut m=[[size[0],0.,0.,0.],[0.,size[1],0.,0.],[0.,0.,size[2],0.],[0.,0.,0.,1.]];
    //16c0/17e0/1900 append each rotation to the existing matrix, with
    //original .001-radian quantization; local scale precedes the rotations.
    for (axis,angle) in angles.into_iter().enumerate() {
        let angle=trunc(angle*1000.) as f32/1000.;
        let (s,c)=f64::from(angle).sin_cos();let (s,c)=(s as f32,c as f32);
        let r=match axis {
            0=>[[1.,0.,0.,0.],[0.,c,-s,0.],[0.,s,c,0.],[0.,0.,0.,1.]],
            1=>[[c,0.,s,0.],[0.,1.,0.,0.],[-s,0.,c,0.],[0.,0.,0.,1.]],
            _=>[[c,-s,0.,0.],[s,c,0.,0.],[0.,0.,1.,0.],[0.,0.,0.,1.]],
        };
        m=round_multiply(m,r);
    }
    for axis in 0..3 {m[axis][3]+=position[axis];}m
}
