//! Independent cmnptcl/wc_cmnptcl SDK2 interpreter. Selected static
//! evidence: config_afe0138dfd/config_137c565ce1 and common_compare.
//! Uses the resource router and the owner's RNG/clock, never a PE loader.
use super::{ParticleCanvas,ParticleQuad,ParticleRandomSource,ParticleRenderSnapshot,ParticleVertex};
use super::super::{native_state_error,ResourceIndex};
use cs2_runtime::resource_router::ResourceMode;
use cs2_script::Result;

pub(super) const COMMON_SHA:&str="afe0138dfde4b7c558b87e00e2518c34e55cb0866cb27ef0ba9623bdd73d1c05";
pub(super) const COMMON_LEGACY_SHA:&str="137c565ce19826c1d34b3d4e8e04f80fe17a89747b840e083a7c66fe852b5fbc";
const KEYS:[&str;50]=[
    "angle","angleVariance","blendFuncDestination","blendFuncSource","duration","emitterType",
    "finishColorAlpha","finishColorBlue","finishColorGreen","finishColorRed",
    "finishColorVarianceAlpha","finishColorVarianceBlue","finishColorVarianceGreen","finishColorVarianceRed",
    "finishParticleSize","finishParticleSizeVariance","gravityx","gravityy","maxParticles",
    "maxRadius","maxRadiusVariance","minRadius","particleLifespan","particleLifespanVariance",
    "radialAccelVariance","radialAcceleration","rotatePerSecond","rotatePerSecondVariance",
    "rotationEnd","rotationEndVariance","rotationStart","rotationStartVariance",
    "sourcePositionVariancex","sourcePositionVariancey","sourcePositionx","sourcePositiony","speed","speedVariance",
    "startColorAlpha","startColorBlue","startColorGreen","startColorRed",
    "startColorVarianceAlpha","startColorVarianceBlue","startColorVarianceGreen","startColorVarianceRed",
    "startParticleSize","startParticleSizeVariance","tangentialAccelVariance","tangentialAcceleration",
];

#[derive(Clone,Debug,Default)]
struct Sprite {
    position:[f32;3],pivot:[f32;2],scale:f32,rotation:f32,color:u32,flag:u32,
}
#[derive(Clone,Debug,Default)]
struct Record {
    position:[f32;3],direction:[f32;3],velocity:[f32;3],
    life:f32,color:[u8;4],color_step:[u8;4],size:f32,size_step:f32,
    rotation:f32,rotation_step:f32,radial:f32,tangential:f32,
    angle:f32,angle_step:f32,radius:f32,radius_step:f32,retired:bool,
    sprite:Sprite,
}

#[derive(Debug)]
pub(super) struct CommonParticles {
    records:Vec<Record>,config:[f32;50],image:[f32;2],viewport_half:[f32;2],
    blend:u32,live:usize,finished:usize,accumulator:f32,time:f32,
    running:bool,emission_stopped:bool,burst:bool,
}

impl CommonParticles {
    pub(super) fn new(count:u32,image:[f32;2])->Self {
        Self {records:vec![Record::default();count as usize],config:[0.;50],image,
            viewport_half:[0.;2],blend:0,live:0,finished:0,accumulator:0.,time:0.,
            running:false,emission_stopped:false,burst:false}
    }
    pub(super) fn reset_extent(&mut self,image:[f32;2]) {self.image=image;}

    pub(super) fn start(&mut self,p:&[u8;128],viewport:[u32;2],resources:&ResourceIndex,
        random:&mut impl ParticleRandomSource)->Result<()> {
        //2b86/2a86 SDK80 receives this formatted name and reads actual
        //plist bytes. The task's texture is independent of plist texture keys.
        let name=format!("ptcl{}{:03}.plist",int(word(p,0)),int(word(p,1)));
        let bytes=resources.read_mode(&name,ResourceMode::LooseFirst)
            .map_err(|_|native_state_error("particle plist resource read failed"))?
            .ok_or_else(||native_state_error("particle plist resource is unavailable"))?;
        self.config=parse_config(&bytes,viewport)?;
        self.viewport_half=viewport.map(|v|v as f32*0.5);
        self.burst=int(word(p,2))!=0;self.live=0;self.finished=0;self.accumulator=0.;self.time=0.;
        //361b only writes blend1 for destination1; other starts retain it.
        if int(self.config[2])==1 {self.blend=1;}
        //35c4 deliberately does not clear emission_stopped (168). Preserve
        //the original stop/restart state instead of silently resetting it.
        for d in &mut self.records {spawn(d,&self.config,self.image,random);}
        self.running=true;
        Ok(())
    }

    pub(super) fn update(&mut self,delta:f32,source:[f32;3],random:&mut impl ParticleRandomSource) {
        if !self.running {return;}
        let c=&self.config;let offset=[source[0]-self.viewport_half[0],source[1]-self.viewport_half[1]];
        if !self.emission_stopped {
            let period=1./(self.records.len() as f32/c[22]);
            if self.live<self.records.len() {
                self.accumulator+=delta;
                while self.accumulator>period && self.live<self.records.len() {
                    if self.burst {self.live=self.records.len();} else {self.live+=1;}
                    self.accumulator-=period;
                }
            }
            self.time+=delta;
            if c[4]!=-1. && self.time>c[4] {self.emission_stopped=true;}
        }
        let mut index=0;
        while index<self.live {
            let slot=index;
            let d=&mut self.records[index];d.life-=delta;
            if d.life>0. {
                d.sprite.flag=1;
                if int(c[5])==1 {
                    d.angle+=d.angle_step*delta;d.radius+=d.radius_step*delta;
                    d.position=[-((f64::from(d.angle).cos()*f64::from(d.radius)) as f32)+c[34],
                        -((f64::from(d.angle).sin()*f64::from(d.radius)) as f32)+c[35],d.position[2]];
                } else {
                    //3ac8..3ee1: actual positional radial term and signed
                    //velocity-relative tangent construction. No guessed
                    //ParticleDesigner normalization of the radial vector.
                    let radial=scale([d.position[0]-c[34],d.position[1]-c[35],0.],d.radial);
                    let tangent=perp(d.direction);
                    let tangent=if d.tangential>0. {
                        sub(d.velocity,perp(sub(d.velocity,tangent)))
                    } else {sub(perp(sub(tangent,d.velocity)),d.velocity)};
                    let tangent=scale(unit(tangent),d.tangential);
                    let acceleration=add(add(radial,tangent),[c[16],c[17],0.]);
                    d.velocity=add(d.velocity,scale(acceleration,delta));
                    d.position=add(d.position,scale(scale(d.velocity,delta),1.));
                }
                for i in 0..4 {
                    //3ee4 uses unsigned byte deltas and wraps each channel.
                    d.color[i]=int(d.color_step[i] as f32*delta+d.color[i] as f32) as u8;
                }
                d.size=nonnegative(d.size+d.size_step*delta);d.rotation+=d.rotation_step*delta;
                d.sprite.position=d.position;d.sprite.rotation=d.rotation;d.sprite.scale=d.size;
                d.sprite.color=u32::from_le_bytes(d.color);index+=1;
            } else if self.emission_stopped {
                if !d.retired {d.retired=true;d.sprite.flag=0;self.finished+=1;}
                if self.finished==self.live {
                    //418b clears this emitter, not the owner's task active
                    //bit. The final flagged-off snapshot is still submitted.
                    self.running=false;self.time=c[4];self.accumulator=0.;return;
                }
                index+=1;
            } else {
                //40a4 first initializes the dying slot (observable RNG),
                //then swaps in the tail SDK/private record and initializes
                //that tail again. The moved live particle is revisited.
                spawn(d,c,self.image,random);
                let last=self.live-1;
                if index!=last {
                    self.records[index]=self.records[last].clone();
                    spawn(&mut self.records[last],c,self.image,random);
                }
                self.live-=1;
            }
            //414d applies translation even to dead/replaced SDK records.
            self.records[slot].sprite.position[0]+=offset[0];
            self.records[slot].sprite.position[1]+=offset[1];
        }
    }

    pub(super) fn snapshot(&self,canvas:ParticleCanvas,active:bool,submitted:bool,
        elapsed:u32,previous_elapsed:u32,opacity:u32)->ParticleRenderSnapshot {
        let [w,h]=self.image;
        ParticleRenderSnapshot {active,frame_complete:true,elapsed,previous_elapsed,
            current_quad:self.records.len() as u32,render_mode:self.blend,opacity,canvas,
            quads:self.records.iter().enumerate().map(|(index,d)| {
                let s=&d.sprite;let (sin,cos)=f64::from(s.rotation).sin_cos();let (sin,cos)=(sin as f32,cos as f32);
                let vertices=std::array::from_fn(|i| {
                    let uv=[(i&1) as f32,(i>>1) as f32];
                    let mut x=uv[0]*w;let mut y=uv[1]*h;
                    if submitted {
                        x=(x-s.pivot[0])*s.scale;y=(y-s.pivot[1])*s.scale;
                        let [a,b]=[x,y];x=a*cos-b*sin+s.position[0];y=a*sin+b*cos+s.position[1];
                    }
                    ParticleVertex {position:[x,y,if submitted {s.position[2]} else {0.}],uv,
                        color:if submitted {s.color} else {u32::MAX},..Default::default()}
                });
                ParticleQuad {index:index as u32,vertices,offset:[0.;4],depth_bias:0.,flag:if submitted {s.flag} else {1}}
            }).collect()}
    }
}

fn spawn(d:&mut Record,c:&[f32;50],image:[f32;2],random:&mut impl ParticleRandomSource) {
    d.sprite.flag=0;d.sprite.pivot=image.map(|v|v*0.5);d.retired=false;
    d.life=nonnegative(varied(c[22],c[23],random));
    d.position=[varied(c[34],c[32],random),varied(c[35],c[33],random),d.position[2]];
    d.direction=unit([d.position[0]-c[34],d.position[1]-c[35],0.]);
    //451f..4842 draws start RGBA then finish RGBA, but SDK color bytes
    //are BGRA. Delta storage itself is BYTE, including negative quotients.
    let start=[varied(c[41],c[45],random).clamp(0.,255.),varied(c[40],c[44],random).clamp(0.,255.),
        varied(c[39],c[43],random).clamp(0.,255.),varied(c[38],c[42],random).clamp(0.,255.)];
    let finish=[varied(c[9],c[13],random).clamp(0.,255.),varied(c[8],c[12],random).clamp(0.,255.),
        varied(c[7],c[11],random).clamp(0.,255.),varied(c[6],c[10],random).clamp(0.,255.)];
    for (byte,channel) in [2,1,0,3].into_iter().enumerate() {
        d.color[channel]=int(start[byte]) as u8;d.color_step[channel]=int((finish[byte]-start[byte])/d.life) as u8;
    }
    d.size=nonnegative(varied(c[46],c[47],random));
    d.size_step=(nonnegative(varied(c[14],c[15],random))-d.size)/d.life;
    d.rotation=radians(varied(c[30],c[31],random));
    d.rotation_step=(radians(varied(c[28],c[29],random))-d.rotation)/d.life;
    let angle=radians(varied(c[0],c[1],random));
    if int(c[5])==1 {
        d.radius=varied(c[19],c[20],random);
        d.radius_step=(varied(c[21],0.,random)-d.radius)/d.life;
        d.angle=angle;d.angle_step=radians(varied(c[26],c[27],random));
    } else {
        let speed=varied(c[36],c[37],random);
        //SDK1c is cos,18 is sin; both receive the already converted angle.
        d.velocity=scale([f64::from(angle).cos() as f32,f64::from(angle).sin() as f32,0.],speed);
        d.radial=varied(c[25],c[24],random);d.tangential=varied(c[49],c[48],random);
        //The public start writes the optional velocity-aligned rotation
        //flag to zero; no extra atan2/RNG call is invented here.
    }
}

fn parse_config(bytes:&[u8],viewport:[u32;2])->Result<[f32;50]> {
    if bytes.len()>1024*1024 {return Err(native_state_error("particle plist exceeds1 MiB"));}
    let xml=std::str::from_utf8(bytes).map_err(|_|native_state_error("particle plist is not UTF8 XML"))?;
    let mut rest=xml.split_once("<dict>").ok_or_else(||native_state_error("particle plist dict is missing"))?.1;
    let mut values=[None;50];let mut entries=0;
    loop {
        rest=skip_space_comments(rest)?;
        if rest.starts_with("</dict>") {break;}
        let key=rest.strip_prefix("<key>").ok_or_else(||native_state_error("particle plist expected key"))?;
        let (key,next)=key.split_once("</key>").ok_or_else(||native_state_error("particle plist key is truncated"))?;
        rest=skip_space_comments(next)?;entries+=1;
        if entries>256 {return Err(native_state_error("particle plist exceeds256 entries"));}
        let tag_end=rest.find('>').ok_or_else(||native_state_error("particle plist value is truncated"))?;
        let tag=&rest[1..tag_end];
        let (value,next)=if tag.ends_with('/') {("",&rest[tag_end+1..])} else {
            if !matches!(tag,"real"|"integer"|"string"|"data") {return Err(native_state_error("particle plist has unsupported nested value"));}
            let closing=format!("</{tag}>");
            rest[tag_end+1..].split_once(closing.as_str()).ok_or_else(||native_state_error("particle plist scalar is truncated"))?
        };
        if let Some(index)=KEYS.iter().position(|name|*name==key.trim()) {
            let value=value.trim().parse::<f32>().map_err(|_|native_state_error("particle plist numeric scalar is invalid"))?;
            if !value.is_finite() {return Err(native_state_error("particle plist contains nonfinite numeric scalar"));}
            values[index]=Some(value);
        }
        rest=next;
    }
    let radius=values[5].map(int)==Some(1);let mut c=[0.;50];
    for i in 0..50 {
        //Missing consumed fields originally expose uninitialized storage.
        //Diagnose them rather than inventing a playable default preset.
        let unused=matches!(i,3|18) || if radius {matches!(i,16|17|24|25|36|37|48|49)} else {matches!(i,19|20|21|26|27)};
        c[i]=match values[i] {Some(v)=>v,None if unused=>0.,None=>return Err(native_state_error("particle plist lacks a consumed numeric field"))};
        c[i]=match i {
            0=>-c[i],2|3|5=>int(c[i]) as f32,4 if c[i]>0.=>c[i]*60.,
            6..=13|38..=45=>c[i]*255.,14|15|46|47=>c[i]/30.,
            16=>c[i]/3600.,17=>-c[i]/3600.,22|23=>c[i]*60.,
            24|25=>c[i]/360000.,26=>-c[i]/60.,27|36|37=>c[i]/60.,
            35=>viewport[1] as f32-c[i],48|49=>c[i]/3600.,_=>c[i],
        };
    }
    if c[22]<=0. {return Err(native_state_error("particle plist lifespan must be positive"));}
    Ok(c)
}
fn skip_space_comments(mut text:&str)->Result<&str> {
    loop {text=text.trim_start();
        if let Some(comment)=text.strip_prefix("<!--") {text=comment.split_once("-->").ok_or_else(||native_state_error("particle plist comment is truncated"))?.1;}
        else {return Ok(text);}
    }
}
fn word(p:&[u8;128],i:usize)->f32 {f32::from_le_bytes(p[i*4..i*4+4].try_into().unwrap())}
fn int(v:f32)->i32 {if !v.is_finite() || !(-2147483648.0..2147483648.0).contains(&v) {i32::MIN} else {v.trunc() as i32}}
fn varied(base:f32,variance:f32,random:&mut impl ParticleRandomSource)->f32 {
    let value=f32::from_bits(random.next_float_bits());((value*2.)-1.)*variance+base
}
fn radians(degrees:f32)->f32 {(f64::from(degrees)*f64::from(std::f32::consts::PI)/180.) as f32}
fn nonnegative(v:f32)->f32 {if v>0. {v} else {0.}}
fn add(a:[f32;3],b:[f32;3])->[f32;3] {std::array::from_fn(|i|a[i]+b[i])}
fn sub(a:[f32;3],b:[f32;3])->[f32;3] {std::array::from_fn(|i|a[i]-b[i])}
fn scale(a:[f32;3],s:f32)->[f32;3] {a.map(|v|v*s)}
fn unit(a:[f32;3])->[f32;3] {
    let length=f64::from((a[1]*a[1]+a[0]*a[0])+a[2]*a[2]).sqrt() as f32;
    if length>0. {a.map(|v|v/length)} else {a}
}
fn perp(a:[f32;3])->[f32;3] {[-a[1],a[0],a[2]]}
