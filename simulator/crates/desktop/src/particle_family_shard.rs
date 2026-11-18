//! Scene-referenced swordspark1070/1110/1860. Shared real matrix/face
//! consumer; independent radial initialization and three-stage integrator.
use super::*;
static SQUARE_ACTIVE:AtomicU32=AtomicU32::new(1);
//s_080102 has its own module11dd0, also initialized to1 and retained at start.
static EARLY_SQUARE_ACTIVE:AtomicU32=AtomicU32::new(1);

#[derive(Debug,Default)]
pub(super) struct ShardConfig {pieces:Vec<Shard>}
#[derive(Debug)]
struct Shard {position:[f32;3],velocity:[f32;3],angles:[f32;3],spin:[f32;3],size:f32}

// The source rotates, adds translation, then prepends XY scale. Keeping
// this order matters when X/Y scale differs from Z. SDK pivot stays zero.
pub(super) fn face(image:[f32;2],size:f32,angles:[f32;3],position:[f32;3])->[[f32;3];4] {
    let rotation=matrix::affine([1.;3],angles,position);
    let mut scale=matrix::identity();scale[0]=size;scale[5]=size;
    let combined=matrix::product(scale,rotation);
    std::array::from_fn(|i|matrix::transform(combined,
        [((i&1) as f32-0.5)*image[0],((i>>1) as f32-0.5)*image[1],0.]))
}

fn light(vertices:[[f32;3];4])->u32 {
    let a=sub(vertices[1],vertices[0]);let b=sub(vertices[2],vertices[0]);
    let n=normalize_nonzero([a[1]*b[2]-a[2]*b[1],a[2]*b[0]-a[0]*b[2],a[0]*b[1]-a[1]*b[0]]);
    let value=trunc(super::super::lerp(255.,128.,2.,(n[0]+n[1]-n[2])+1.)) as u8 as u32;
    value|(value<<8)|(value<<16)
}

impl PathFamily {
    pub(super) fn start_early_square(&mut self,p:&[u8;128]) {
        self.start_square(p);
        //1070 saves p3/4/5, but1120 uses fixed rotations and80/20/120.
        for s in &mut self.shard.pieces {s.angles=[0.15,-0.75,0.];}
    }

    pub(super) fn update_early_square(&mut self,tick:u32) {
        self.update_square_values(tick,80.,20.,120.,&EARLY_SQUARE_ACTIVE);
    }

    pub(super) fn start_square(&mut self,p:&[u8;128]) {
        self.main_count=self.drops.len();self.child_count=0;self.stream_render_mode=1;
        self.shard.pieces.clear();
        for d in &mut self.drops {
            let position=[word(p,0),word(p,1),0.];
            self.shard.pieces.push(Shard {position,velocity:[0.;3],
                angles:[word(p,3)/100.,word(p,4)/100.,0.],spin:[0.;3],size:word(p,2)/100.});
            d.clock=0;d.phase=0;d.phase_origin=0;d.alpha=0;
            d.uv=[[0.5,0.5],[0.75,0.5],[0.5,0.75],[0.75,0.75]];
            d.position=[0.;3];d.pivot=[0.;2];d.scale=1.;d.stretch=[1.;2];d.rotation=0.;d.rotation_xy=[0.;2];
            d.vertex_rgb=None;d.vertex_alpha=None;
            d.geometry=Some(face(self.image,1.,[0.;3],position));
        }
    }

    pub(super) fn update_square(&mut self,tick:u32,p:&[u8;128]) {
        self.update_square_values(tick,word(p,5),word(p,6),word(p,6),&SQUARE_ACTIVE);
    }

    fn update_square_values(&mut self,tick:u32,fadein:f32,fadeout:f32,end:f32,active:&AtomicU32) {
        //11dd0 is module data initialized to1;1070/1920 never resets it.
        let count=active.load(Ordering::Relaxed).wrapping_add(1).min(self.main_count as u32);
        active.store(count,Ordering::Relaxed);self.active_count=count as usize;
        for (d,s) in self.drops.iter_mut().zip(&self.shard.pieces).take(self.active_count) {
            let now=d.clock as i32 as f32;
            match d.phase {
                0=> {
                    d.alpha=trunc(super::super::lerp(0.,255.,fadein,now)) as u8;
                    d.geometry=Some(face(self.image,s.size,s.angles,s.position));
                    if now>fadein {d.phase=1;}
                },
                1=> {
                    //18c6 does not stamp phase origin;1d00 zeroed it.
                    d.alpha=trunc(super::super::lerp(255.,0.,fadeout,now)) as u8;
                    d.geometry=Some(face(self.image,s.size,s.angles,s.position));
                    if now>end {d.phase=2;}
                },
                _=>{},
            }
            d.captured=d.position;d.clock=d.clock.wrapping_add(tick);
        }
    }

    pub(super) fn start_shard(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        let count=self.drops.len();self.main_count=count;self.active_count=count;self.child_count=0;
        //10b3 compares1 against parameter4, with an ordered strict test.
        if 1.>word(p,4) {self.stream_render_mode=1;}
        self.shard.pieces.clear();
        for (index,d) in self.drops.iter_mut().enumerate() {
            let tiles=trunc(word(p,5)) as u32;
            d.uv=if tiles==0 {[[0.,0.],[1.,0.],[0.,1.],[1.,1.]]} else {
                let tile=random.next_integer()%tiles;
                std::array::from_fn(|i|[((tile&1)+(i&1) as u32) as f32*0.5,
                    ((tile>>1)+(i>>1) as u32) as f32*0.5])
            };
            let base=word(p,2)/1000.;let size=(f64::from(sample(random))*f64::from(base)+f64::from(base)) as f32;
            let position=[word(p,0),word(p,1),0.];
            let degrees=(360./count as f32)*index as f32;
            let radians=(f64::from(degrees)*f64::from(std::f32::consts::PI)/180.) as f32;
            //19e4 quantizes SDK10's result to hundredths before sin/cos.
            let radians=trunc(radians*100.) as f32/100.;
            let direction=unit([-(f64::from(radians).sin() as f32),f64::from(radians).cos() as f32,0.5]);
            let speed=(f64::from(sample(random))*f64::from(word(p,3))+f64::from(word(p,3))*0.5) as f32;
            let velocity=[direction[0]*speed,direction[1]*speed,direction[2]*(speed*3.)];
            let mut angles=std::array::from_fn(|_|(f64::from(sample(random))*f64::from(std::f32::consts::PI)) as f32);
            let spin=std::array::from_fn(|_|(f64::from(sample(random))*f64::from(0.3f32)+f64::from(0.3f32)) as f32);
            //1b24..1b65 changes initial angles, not the positive spin steps.
            for angle in &mut angles {if random.next_integer()&1!=0 {*angle = -*angle;}}
            self.shard.pieces.push(Shard {position,velocity,angles,spin,size});
            d.clock=0;d.phase=0;d.saved_alpha=0.;d.alpha=0;
            d.position=[0.;3];d.pivot=[0.;2];d.scale=1.;d.stretch=[1.;2];d.rotation=0.;d.rotation_xy=[0.;2];
            d.vertex_alpha=None;d.vertex_rgb=None;
            d.geometry=Some(face(self.image,1.,[0.;3],position));
        }
    }

    pub(super) fn update_shard(&mut self,tick:u32,p:&[u8;128]) {
        for (d,s) in self.drops.iter_mut().zip(&mut self.shard.pieces) {
            let now=d.clock as i32 as f32;
            if d.phase<=2 {
                let move_scale=if d.phase==0 {1.} else {0.3};
                let spin_scale=if d.phase==0 {1.} else {0.5};
                s.position=add(s.position,scale(s.velocity,move_scale));
                s.angles=add(s.angles,scale(s.spin,spin_scale));
                if d.phase==2 {
                    d.alpha=trunc(super::super::lerp(d.saved_alpha,0.,word(p,8)-word(p,7),now-word(p,7))) as u8;
                    if now>word(p,8) {d.phase=3;d.alpha=0;}
                } else {
                    d.alpha=trunc(64.*word(p,4)+191.) as u8;
                    let limit=word(p,if d.phase==0 {6} else {7});
                    if now>limit {d.phase+=1;d.saved_alpha=d.alpha as f32;}
                }
            }
            let vertices=face(self.image,s.size,s.angles,s.position);
            d.geometry=Some(vertices);d.vertex_rgb=Some([light(vertices);4]);
            d.clock=d.clock.wrapping_add(tick);
        }
    }
}
