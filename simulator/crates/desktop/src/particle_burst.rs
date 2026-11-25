//! Independent bang2 SDK2.1 state machine. Original PE is read-only evidence,
//! never loaded. Start1070/1360, update1110, finish1350,28-byte private record.
use cs2_script::Result;
use super::particle_vm::{ParticleCanvas,ParticleQuad,ParticleRandomSource,ParticleRenderSnapshot,ParticleVertex};

#[derive(Debug,Clone,Default)]
struct Drop {
    clock:u32,phase:u32,fade_alpha:u32,
    position:[f32;3],velocity:[f32;3],rotation:f32,angular_velocity:f32,
    scale:f32,alpha:u8,uv_origin:[f32;2],uv_size:f32,
}

#[derive(Debug)]
pub(super) struct ParticleBurst { drops:Vec<Drop>,image:[f32;2],render_mode:u32 }

impl ParticleBurst {
    pub(super) fn new(count:u32,image:[f32;2])->Self {
        Self {drops:vec![Drop::default();count as usize],image,render_mode:0}
    }
    pub(super) fn reset_extent(&mut self,image:[f32;2]) {self.image=image;}
    pub(super) fn start(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource)->Result<()> {
        let parameter=|i| word(p,i);
        let atlas=trunc(f64::from(parameter(5))) as u32;
        if atlas>4 {return Err(super::native_state_error("bang2 UV selector exceeds its recovered four-entry atlas; original out-of-bounds read is not modeled"));}
        if parameter(4)<1. {self.render_mode=1;}
        let count=self.drops.len() as u32;
        for (i,d) in self.drops.iter_mut().enumerate() {
            *d=Drop::default();
            let uv=if atlas==0 {0} else {random.next_integer()%atlas};
            d.uv_origin=[[0.,0.],[0.5,0.],[0.,0.5],[0.5,0.5]][uv as usize];
            d.uv_size=if atlas==0 {1.} else {0.5};
            d.position=[parameter(0),parameter(1),0.];
            let size=f64::from(parameter(2))/100.;
            d.scale=(size*(1.+sample(random))) as f32;
            d.rotation=(sample(random)*f64::from(std::f32::consts::PI)) as f32;
            let step=(360./f64::from(count)) as f32;
            let degrees=(f64::from(step)*i as f64) as f32;
            let radians=(f64::from(degrees)*f64::from(std::f32::consts::PI)/180.) as f32;
            //1531 truncates at a HUNDREDTH radian; rain's local helper uses a
            //different thousandth. Callback+18=sin,+1c=cos,+70=normalize.
            let angle=(f64::from(trunc(f64::from(radians)*100.))/100.) as f32;
            let (sin,cos)=f64::from(angle).sin_cos();let [x,y]=[-(sin as f32),cos as f32];
            let length=(x*x+y*y).sqrt();
            let unit=if length==0. {[0.;2]} else {[x/length,y/length]};
            let speed=(sample(random)*f64::from(parameter(3))+f64::from(parameter(3))*0.5) as f32;
            d.velocity=[(f64::from(unit[0])*f64::from(speed)) as f32,(f64::from(unit[1])*f64::from(speed)) as f32,0.];
            d.angular_velocity=(sample(random)/5.) as f32;
            if random.next_integer()&1!=0 {d.angular_velocity=-d.angular_velocity;}
            for axis in 0..2 {d.position[axis]=(f64::from(d.velocity[axis])*10.+f64::from(d.position[axis])) as f32;}
        }
        Ok(())
    }

    pub(super) fn update(&mut self,delta:f32,p:&[u8;128]) {
        let tick=trunc(f64::from(delta)) as u32;
        for d in &mut self.drops {
            let scale=match d.phase {0=>1.,1|2=>0.30000001192092896,_=>0.};
            if d.phase<=2 {
                for axis in 0..3 {d.position[axis]=(f64::from(d.velocity[axis])*scale+f64::from(d.position[axis])) as f32;}
                let divisor=match d.phase {0=>1.,1=>5.,_=>10.};
                d.rotation=(f64::from(d.angular_velocity)/divisor+f64::from(d.rotation)) as f32;
            }
            match d.phase {
                0|1=>{
                    d.alpha=trunc(f64::from(word(p,4))*64.+191.) as u8;
                    if f64::from(d.clock as i32)>f64::from(word(p,if d.phase==0 {6}else{7})) {
                        d.phase+=1;d.fade_alpha=u32::from(d.alpha);
                    }
                }
                2=>{
                    let start=word(p,7);let end=word(p,8);let now=d.clock as i32 as f32;
                    let from=d.fade_alpha as f32;
                    let alpha=if start>=now {from} else if now>=end {0.}
                        else {(now-start)/(end-start)*(0.-from)+from};
                    d.alpha=trunc(f64::from(alpha)) as u8;
                    if f64::from(d.clock as i32)>f64::from(end) {d.phase=3;d.fade_alpha=d.clock;d.alpha=0;}
                }
                _=>{}
            }
            d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn snapshot(&self,canvas:ParticleCanvas,active:bool,submitted:bool,
        elapsed:u32,previous_elapsed:u32,opacity:u32)->ParticleRenderSnapshot {
        let [w,h]=self.image;let positions=[[0.,0.],[w,0.],[0.,h],[w,h]];
        let base_uv=[[0.,0.],[1.,0.],[0.,1.],[1.,1.]];
        ParticleRenderSnapshot {active,frame_complete:true,elapsed,previous_elapsed,
            current_quad:self.drops.len() as u32,render_mode:self.render_mode,opacity,canvas,
            quads:self.drops.iter().enumerate().map(|(i,d)|{
                let (s,c)=f64::from(d.rotation).sin_cos();let (s,c)=(s as f32,c as f32);
                let vertices=std::array::from_fn(|v|{
                    let [mut x,mut y]=positions[v];let mut uv=base_uv[v];
                    if submitted {
                        x=(x-w*0.5)*d.scale;y=(y-h*0.5)*d.scale;
                        let [a,b]=[x,y];x=a*c-b*s+d.position[0];y=a*s+b*c+d.position[1];
                        uv=[uv[0]*d.uv_size+d.uv_origin[0],uv[1]*d.uv_size+d.uv_origin[1]];
                    }
                    ParticleVertex {position:[x,y,if submitted {d.position[2]} else {0.}],uv,
                        color:if submitted {0x00ff_ffff|u32::from(d.alpha)<<24} else {u32::MAX},..Default::default()}
                });
                ParticleQuad {index:i as u32,vertices,offset:[0.;4],depth_bias:0.,flag:1}
            }).collect()}
    }
}
fn sample(random:&mut impl ParticleRandomSource)->f64 {f64::from(f32::from_bits(random.next_float_bits()))}
fn word(p:&[u8;128],i:usize)->f32 {f32::from_le_bytes(p[i*4..i*4+4].try_into().unwrap())}
fn trunc(v:f64)->i32 {if !v.is_finite()||v>=2147483648.||v< -2147483648. {i32::MIN} else {v.trunc() as i32}}
