//! wc_text01 SDK2.2:2b40/4600/5340/5800. Real plist resource,
//! eight configured words and an owned per-word afterimage pool.
use super::*;
use super::super::super::super::{native_state_error,ResourceIndex};
use cs2_runtime::resource_router::ResourceMode;
use cs2_script::Result;
#[derive(Debug,Default)]
pub(super) struct TextConfig {
    words:[[f32;18];8],number:Option<i32>,counter:i32,gate:i32,
    delay:f32,travel:i32,end_hold:f32,
}
const FIELDS:[&str;18]=["startposx","startposy","startposz","startrotx","startroty","startrotz",
    "startscalex","startscaley","startscalez","endposx","endposy","endposz",
    "endrotx","endroty","endrotz","endscalex","endscaley","endscalez"];
fn scalar_entries(bytes:&[u8])->Result<Vec<(String,String)>> {
    if bytes.len()>1024*1024 {return Err(native_state_error("word particle plist exceeds1 MiB"));}
    let xml=std::str::from_utf8(bytes).map_err(|_|native_state_error("word particle plist is not UTF8 XML"))?;
    let mut rest=xml.split_once("<dict>").ok_or_else(||native_state_error("word particle plist dict is missing"))?.1;
    let mut entries=Vec::new();
    loop {
        rest=rest.trim_start();
        if let Some(comment)=rest.strip_prefix("<!--") {rest=comment.split_once("-->").ok_or_else(||native_state_error("word particle plist comment is truncated"))?.1;continue;}
        if rest.starts_with("</dict>") {return Ok(entries);}
        let (key,next)=rest.strip_prefix("<key>").and_then(|s|s.split_once("</key>"))
            .ok_or_else(||native_state_error("word particle plist key is malformed"))?;
        rest=next.trim_start();let end=rest.find('>').ok_or_else(||native_state_error("word particle plist scalar is truncated"))?;
        let tag=rest.get(1..end).ok_or_else(||native_state_error("word particle plist scalar is malformed"))?;
        if !matches!(tag,"real"|"integer"|"string") {return Err(native_state_error("word particle plist requires flat numeric scalar entries"));}
        let close=format!("</{tag}>");let (value,next)=rest[end+1..].split_once(close.as_str())
            .ok_or_else(||native_state_error("word particle plist scalar closing tag is missing"))?;
        entries.push((key.to_owned(),value.trim().to_owned()));rest=next;
        if entries.len()>256 {return Err(native_state_error("word particle plist exceeds256 entries"));}
    }
}
//5800 clamps elapsed to1 and uses the SDK's degree conversion and
//cosine/sine difference. SDK74 receives elapsed/duration in that order.
fn ease_word(from:[f32;3],to:[f32;3],elapsed:i32,duration:i32)->[f32;3] {
    let elapsed=elapsed.max(1);
    if (elapsed as u32 as f32)>=duration as f32 {return to;}
    let convert=|degrees:f32|(f64::from(degrees)*f64::from(std::f32::consts::PI)/180.) as f32;
    let cosine=|r:f32|f64::from(r).cos() as f32;
    let sine=|r:f32|f64::from(r).sin() as f32;
    let x=cosine(convert(super::super::lerp(90.,0.,duration as f32,elapsed as f32)))-cosine(convert(90.));
    let y=sine(convert(super::super::lerp(180.,270.,duration as f32,elapsed as f32)))-sine(convert(180.));
    [from[0]+x*(to[0]-from[0]),from[1]-y*(to[1]-from[1]),from[2]+x*(to[2]-from[2])]
}
fn spawn_word_child(d:&Drop,trail:&mut [Drop]) {
    if let Some((index,c))=trail.iter_mut().enumerate().find(|(_,c)|c.phase==0) {
        c.flag=1;c.position=d.position;c.position[2]+=index as f32;
        c.scale=d.scale;c.stretch=d.stretch;c.rotation=d.rotation;c.rotation_xy=d.rotation_xy;
        c.clock=0;c.phase=1;c.alpha=0;
    }
}
fn advance_word_children(trail:&mut [Drop],tick:u32,phase:u32) {
    let duration=trail.len() as f32;
    for c in trail.iter_mut().filter(|c|c.phase!=0) {
        let end=if phase<3 {duration} else if phase==5 {5.} else {30.};
        c.alpha=trunc(super::super::lerp(10.,0.,end,c.clock as i32 as f32)) as u8;
        if phase>=3 {c.stretch=c.stretch.map(|v|v*1.006);}
        c.clock=c.clock.wrapping_add(tick);
        //4a66/4ba6 deliberately retire while age is BELOW childCount.
        let retire=if phase<3 {(c.clock as i32)<duration as i32} else {(c.clock as i32)>30};
        if retire {c.phase=0;c.alpha=0;}
    }
}
impl PathFamily {
    pub(in super::super) fn load_text_config(&mut self,p:&[u8;128],resources:&ResourceIndex)->Result<()> {
        let name=format!("ptcl{}{:03}.plist",trunc(word(p,0)),trunc(word(p,1)));
        let bytes=resources.read_mode(&name,ResourceMode::LooseFirst)
            .map_err(|_|native_state_error("word particle plist resource read failed"))?
            .ok_or_else(||native_state_error("word particle plist resource is unavailable"))?;
        for (key,value) in scalar_entries(&bytes)? {
            if key=="numword" {self.text.number=Some(value.parse::<i32>().map_err(|_|native_state_error("word particle numword is not an integer"))?);continue;}
            for index in 0..8 {for (field,prefix) in FIELDS.iter().enumerate() {
                if key!=format!("{prefix}{}",index+1) {continue;}
                //Every startroty branch uses ad40 integer extraction.
                let v=if field==4 {value.parse::<i32>().map(|v|v as f32).map_err(|_|native_state_error("word particle startroty requires integer"))?}
                    else {value.parse::<f32>().map_err(|_|native_state_error("word particle numeric scalar is invalid"))?};
                if !v.is_finite() {return Err(native_state_error("word particle numeric scalar is nonfinite"));}
                self.text.words[index][field]=v;
            }}
        }
        let number=self.text.number.ok_or_else(||native_state_error("word particle plist numword is missing"))?.max(1) as usize;
        let count=self.drops.len();let partition=count/number;
        self.main_count=if partition==0 {count} else {count/partition};
        if self.main_count>8 {return Err(native_state_error("word particle group count exceeds its eight configured records"));}
        self.child_count=partition.saturating_sub(1);Ok(())
    }
    pub(super) fn start_text(&mut self,p:&[u8;128],random:&mut impl ParticleRandomSource) {
        if word(p,2)!=0. {self.stream_render_mode=1;}
        self.text.travel=trunc(word(p,3));self.text.gate=trunc(word(p,4));
        self.text.delay=word(p,5);self.text.end_hold=word(p,6);self.text.counter=0;self.active_count=1;
        for index in 0..self.main_count {
            let c=self.text.words[index];let d=&mut self.drops[index];
            d.clock=0;d.phase=0;d.phase_origin=0;d.pivot=self.image.map(|v|v*0.5);
            let uv=[(index&3) as f32*0.25,(index>>2) as f32*0.25];
            d.uv=std::array::from_fn(|i|[uv[0]+(i&1) as f32*0.25,uv[1]+(i>>1) as f32*0.25]);
            d.alpha=0;d.vertex_rgb=Some([0x00ff_ffff;4]);d.position=[c[0],c[1],c[2]];
            d.captured=d.position;d.rotation_xy=[c[3],c[4]];d.rotation=c[5];d.scale=1.;d.stretch=[c[6],c[7]];
            d.motion=std::array::from_fn(|_|(sample(random)*2.-1.)*0.05);
            d.durations=[self.text.travel,trunc(self.text.end_hold-(self.text.gate.wrapping_add(1).wrapping_mul(index as i32)) as f32),20];
            let template=d.clone();
            let first=self.main_count+index*self.child_count;
            for child in &mut self.drops[first..first+self.child_count] {
                child.clock=0;child.phase=0;child.phase_origin=0;child.pivot=template.pivot;
                child.uv=template.uv;child.alpha=0;child.vertex_rgb=Some([0x00ff_ffff;4]);
            }
        }
    }
    pub(super) fn update_text(&mut self,tick:u32,offset:[f32;2]) {
        if self.text.delay>0. {self.text.delay-=1.;return;}
        if self.text.counter>self.text.gate {self.active_count=(self.active_count+1).min(self.main_count);self.text.counter=0;}
        let (parents,children)=self.drops.split_at_mut(self.main_count);
        //4600 initializes one scratch position per call; only phase1
        //writes it, then5258 copies it into each parent's captured position.
        let mut captured=[0.;3];
        for (index,d) in parents.iter_mut().take(self.active_count).enumerate() {
            let c=self.text.words[index];let age=d.clock.wrapping_sub(d.phase_origin) as i32;
            let trail=&mut children[index*self.child_count..(index+1)*self.child_count];
            if d.phase==0 {d.phase=1;}
            match d.phase {
                1=>{
                    d.position=ease_word([c[0],c[1],c[2]],[c[9],c[10],c[11]],age,d.durations[0]);captured=d.position;
                    let rotation=ease_word([c[3],c[4],c[5]],[c[12],c[13],c[14]],age,d.durations[0]);
                    d.rotation_xy=[rotation[0],rotation[1]];d.rotation=rotation[2];
                    let size=ease_word([c[6],c[7],c[8]],[c[15],c[16],c[17]],age,d.durations[0]);d.stretch=[size[0],size[1]];
                    //48b0..48cf passes SDK74's distance interval[0.01,30].
                    d.alpha=trunc(super::super::lerp(255.,50.,30.-0.01,length(sub(captured,d.captured))-0.01)) as u8;
                    spawn_word_child(d,trail);advance_word_children(trail,tick,1);
                    if age>d.durations[0] {d.phase=2;d.phase_origin=d.clock;}
                }
                2=>{
                    d.position=add(d.position,d.motion);d.alpha=255;advance_word_children(trail,tick,2);
                    if age>d.durations[1] {d.phase=3;d.phase_origin=d.clock;d.alpha=0;self.stream_render_mode=1;
                        for uv in &mut d.uv {uv[1]+=0.5;}
                        for child in trail {for uv in &mut child.uv {uv[1]+=0.5;}}
                    }
                }
                3=>{
                    d.position=add(d.position,d.motion);d.alpha=trunc(super::super::lerp(0.,255.,20.,age as f32)) as u8;
                    spawn_word_child(d,trail);advance_word_children(trail,tick,3);
                    if age>20 {d.phase=4;d.phase_origin=d.clock;}
                }
                4=>{d.position=add(d.position,d.motion);advance_word_children(trail,tick,4);
                    if age>10 {d.phase=5;d.phase_origin=d.clock;}}
                5=>{d.position=add(d.position,d.motion);d.stretch[0]*=1.15;d.stretch[1]*=0.85;
                    d.alpha=trunc(super::super::lerp(255.,0.,5.,age as f32)) as u8;
                    advance_word_children(trail,tick,5);}
                _=>{}
            }
            d.position[0]+=offset[0];d.position[1]+=offset[1];d.captured=captured;d.clock=d.clock.wrapping_add(tick);
        }
        self.text.counter=self.text.counter.wrapping_add(1);
    }
}
