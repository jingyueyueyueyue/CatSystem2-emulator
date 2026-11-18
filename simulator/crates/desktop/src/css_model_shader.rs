//! SHD data reader and explicit CPU equations used by the legacy mesh sink.
//! This reads owned data; it neither invokes D3D nor executes shader bytecode.
use std::collections::BTreeMap;
use anyhow::{Result,anyhow,ensure,bail};

#[derive(Debug,Clone,Copy)]
pub(super) enum Diffuse{Material,Vertex,Product}
#[derive(Debug,Clone,Copy)]
pub(super) enum Texture{None,Material,Base}
#[derive(Debug,Clone,Copy)]
pub(super) enum Cull{None,Clockwise,CounterClockwise}
#[derive(Debug,Clone,Copy)]
pub(super) enum Blend{Opaque,Alpha,Add}
#[derive(Debug)]
pub(super) struct Shader{
    pub diffuse:Diffuse,pub texture:Texture,pub blend:Blend,
    pub z_test:bool,pub z_write:bool,pub cull:Cull,pub wrap:bool,pub alpha_ref:Option<u8>,
    pub linear:bool,
}
fn word(bytes:&[u8],at:usize)->Result<u32>{Ok(u32::from_le_bytes(bytes.get(at..at+4).ok_or_else(||anyhow!("SHD record bounds"))?.try_into()?))}
fn lines<'a>(bytes:&'a[u8],count:usize,offset:usize)->Result<Vec<&'a str>>{
    ensure!(count<65536,"SHD line count limit");let mut at=offset;let mut out=Vec::new();
    for _ in 0..count{let end=bytes.get(at..).and_then(|s|s.iter().position(|b|*b==0)).ok_or_else(||anyhow!("SHD unterminated line"))?+at;
        out.push(std::str::from_utf8(&bytes[at..end])?);at=end+1;}
    Ok(out)
}
fn compact(value:&str)->String{value.chars().filter(|c|!c.is_ascii_whitespace()).map(|c|c.to_ascii_lowercase()).collect()}
impl Shader{
    pub(super) fn parse(bytes:&[u8],attributes:u8)->Result<Self>{
        ensure!(bytes.get(..4)==Some(b"SHD\0")&&bytes.len()<=16*1024*1024,"invalid SHD container");
        let count=word(bytes,4)? as usize;ensure!(count<=4096,"SHD section limit");
        let start=8+count*24;ensure!(start<=bytes.len(),"truncated SHD directory");
        let mut sections=BTreeMap::new();let mut programs=Vec::new();
        for i in 0..count{
            let at=8+i*24;let name=&bytes[at..at+16];let end=name.iter().position(|b|*b==0).ok_or_else(||anyhow!("SHD section name lacks NUL"))?;
            let name=std::str::from_utf8(&name[..end])?;let offset=word(bytes,at+20)? as usize;ensure!(offset>=start&&offset<=bytes.len(),"SHD section offset bounds");
            let data=lines(bytes,word(bytes,at+16)? as usize,offset)?;
            if name=="SHADER"{programs.push(data);}else{ensure!(sections.insert(name.to_owned(),data).is_none(),"SHD duplicate named section");}
        }
        // 496240 selects by actual geometry channels. Never use the first
        // textured declaration for an untextured primitive or fabricate D/UV.
        let mut selected=None;let mut fallback=None;
        for headers in &programs{
            let settings:BTreeMap<_,_>=headers.iter().filter_map(|line|line.split_once('=')).map(|(k,v)|(compact(k),v.trim())).collect();
            let Some(fvf)=settings.get("fvf")else{continue;};let mut needed=0u8;let mut supported=true;
            for part in compact(fvf).split('|'){needed|=match part{"p"=>1,"n"=>2,"d"=>4,"t1"=>8,_=>{supported=false;0}};}
            if supported&&needed&1!=0&&needed&attributes==needed&&needed&8==attributes&8{
                if needed==attributes{selected=Some(settings);break;}
                if fallback.is_none(){fallback=Some(settings);}
            }
        }
        let settings=selected.or(fallback).ok_or_else(||anyhow!("SHD has no recovered program for actual mesh channels {attributes:#x}"))?;
        ensure!(settings.get("type").is_none_or(|v|*v=="0"),"SHD program type is not mesh0");
        let fvf=compact(settings.get("fvf").ok_or_else(||anyhow!("SHD lacks FVF"))?);
        ensure!(fvf.split('|').all(|v|matches!(v,"p"|"n"|"d"|"t1")),"SHD FVF requires unsupported mesh data");
        let section=|key:&str|->Result<&Vec<&str>>{let name=settings.get(key).ok_or_else(||anyhow!("SHD lacks {key} selector"))?;sections.get(*name).ok_or_else(||anyhow!("SHD missing section {name}"))};
        let inputs=section("input")?;
        let inputs:BTreeMap<_,_>=inputs.iter().filter_map(|line|line.split_once('=')).map(|(k,v)|(compact(k),compact(v))).collect();
        let mut position=false;let mut diffuse=None;let mut uv=false;
        for line in section("vs")?{
            let line=compact(line.split("//").next().unwrap());
            if matches!(line.as_str(),"vs.1.0"|"vs.1.1"|""){continue;}
            match line.as_str(){
                "m4x4opos,v0,c0"=>{ensure!(inputs.get("0").is_some_and(|v|v=="m_worldviewproj"),"SHD c0 is not WORLDVIEWPROJ");position=true;},
                "movod0,c4"=>{ensure!(inputs.get("4").is_some_and(|v|v=="v_diffuse"),"SHD c4 is not DIFFUSE");diffuse=Some(Diffuse::Material);},
                "movod0,v5"=>{ensure!(fvf.split('|').any(|v|v=="d"),"SHD reads absent vertex diffuse");diffuse=Some(Diffuse::Vertex);},
                "mulod0,v5,c4"=>{ensure!(inputs.get("4").is_some_and(|v|v=="v_diffuse")&&fvf.split('|').any(|v|v=="d"),"SHD diffuse product lacks inputs");diffuse=Some(Diffuse::Product);},
                "movot0,v7"=>{ensure!(fvf.split('|').any(|v|v=="t1"),"SHD reads absent UV");uv=true;},
                // Fog output is unused only if the actual pass disables fog.
                "dp4r0.z,v0,c12"|"madofog.x,r0.z,c14.x,c14.y"=>{},
                _=>bail!("SHD vertex equation is not recovered: {line}"),
            }
        }
        ensure!(position,"SHD does not write recovered clip position");let diffuse=diffuse.ok_or_else(||anyhow!("SHD lacks recovered diffuse output"))?;
        let effect=section("effect")?.join("\n");let program=compact(&effect);
        ensure!(!program.contains("pixelshader")&&!program.contains("vertexshader"),"SHD compiled shader pass is not recovered");
        ensure!(program.matches("pass").count()==1,"SHD multipass program requires a real pass scheduler");
        // Only declarations and the actual pass assignments participate.
        let open=program.find('{').ok_or_else(||anyhow!("SHD technique body missing"))?;
        let pass=program[open+1..].find('{').map(|i|open+2+i).ok_or_else(||anyhow!("SHD pass body missing"))?;
        let end=program[pass..].find('}').map(|i|pass+i).ok_or_else(||anyhow!("SHD pass closing brace missing"))?;
        let mut state=BTreeMap::new();
        for statement in program[pass..end].split(';').filter(|s|!s.is_empty()){
            let(k,v)=statement.split_once('=').ok_or_else(||anyhow!("SHD pass statement unsupported"))?;
            ensure!(state.insert(k,v).is_none(),"SHD repeated render state requires ordered interpretation");
        }
        let get=|k:&str,default:&str|state.get(k).copied().unwrap_or(default).to_owned();
        ensure!(get("fogenable","false")=="false","SHD fog equation is not implemented");
        ensure!(get("fillmode","solid")=="solid"&&get("colorop[1]","disable")=="disable","SHD fill/multitexture mode unsupported");
        let texture=match get("texture[0]","").as_str(){""=>Texture::None,"<tex0>"=>Texture::Material,"<tlts>"=>Texture::Base,_=>bail!("SHD texture semantic is not recovered")};
        match texture{Texture::None=>{ensure!(get("colorop[0]","selectarg1")=="selectarg1"&&get("colorarg1[0]","diffuse")=="diffuse"&&get("alphaop[0]","selectarg1")=="selectarg1"&&get("alphaarg1[0]","diffuse")=="diffuse","SHD untextured combiner unsupported");},
            _=>{ensure!(uv,"SHD textured pass lacks UV");ensure!(get("colorop[0]","modulate")=="modulate"&&get("colorarg1[0]","texture")=="texture"&&get("colorarg2[0]","diffuse")=="diffuse"&&get("alphaop[0]","modulate")=="modulate"&&get("alphaarg1[0]","texture")=="texture"&&get("alphaarg2[0]","diffuse")=="diffuse","SHD texture combiner unsupported");}}
        let blend=if get("alphablendenable","false")=="false"{Blend::Opaque}else{match(get("srcblend","one").as_str(),get("destblend","zero").as_str()){
            ("srcalpha","invsrcalpha")=>Blend::Alpha,("srcalpha","one")=>Blend::Add,_=>bail!("SHD blend equation unsupported")}};
        let cull=match get("cullmode","ccw").as_str(){"none"=>Cull::None,"cw"=>Cull::Clockwise,"ccw"=>Cull::CounterClockwise,_=>bail!("SHD cull mode invalid")};
        let wrap=match(get("addressu[0]","wrap").as_str(),get("addressv[0]","wrap").as_str()) {("wrap","wrap")=>true,("clamp","clamp")=>false,_=>bail!("SHD asymmetric/mirror texture addressing unsupported")};
        // D3D8 MIN/MAG defaults are POINT. A resource may explicitly choose
        // LINEAR for both; differing min/mag needs derivative-based selection.
        let linear=match(get("minfilter[0]","point").as_str(),get("magfilter[0]","point").as_str()){
            ("point","point")=>false,("linear","linear")=>true,_=>bail!("SHD min/mag filter pair requires derivative sampling"),
        };
        let alpha_ref=if get("alphatestenable","false")=="false"{None}else{ensure!(get("alphafunc","always")=="greaterequal","SHD alpha comparison unsupported");let raw=get("alpharef","0");Some(if let Some(hex)=raw.strip_prefix("0x"){u8::from_str_radix(hex,16)?}else{raw.parse()?})};
        // An unrecognized active render state cannot be silently discarded.
        for key in state.keys(){ensure!(matches!(*key,"texture[0]"|"cullmode"|"zwriteenable"|"zenable"|"zfunc"|"srcblend"|"destblend"|"alphablendenable"|"alphatestenable"|"alphafunc"|"alpharef"|"fillmode"|"colorop[0]"|"colorarg1[0]"|"colorarg2[0]"|"alphaop[0]"|"alphaarg1[0]"|"alphaarg2[0]"|"colorop[1]"|"addressu[0]"|"addressv[0]"|"minfilter[0]"|"magfilter[0]"|"edgeantialias"|"lastpixel"|"fogenable"),"SHD state {key} requires a consumer");}
        ensure!(get("edgeantialias","false")=="false"&&get("zfunc","lessequal")=="lessequal","SHD antialias/depth comparison unsupported");
        let boolean=|key:&str,default:&str|->Result<bool>{match get(key,default).as_str(){"true"=>Ok(true),"false"=>Ok(false),_=>bail!("SHD boolean {key} invalid")}};
        Ok(Self{diffuse,texture,blend,z_test:boolean("zenable","true")?,z_write:boolean("zwriteenable","true")?,cull,wrap,alpha_ref,linear})
    }
}
