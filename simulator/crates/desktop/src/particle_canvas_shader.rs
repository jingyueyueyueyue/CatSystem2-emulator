//! Data-driven SHD selection; only independently understood equations execute.
//! Repeated SHADER sections are intentional (one file has several FVF variants).
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;
use cs2_renderer::RgbaFrame;
use cs2_runtime::ResourceIndex;
use super::{mesh::Material, string, word};
#[path = "particle_canvas_texture.rs"]
mod texture;

#[derive(Clone, Copy, Debug)]
pub(super) enum Diffuse { Material, Vertex, Product, Directional { alpha_lit: bool } }
#[derive(Debug)]
pub(super) struct Program {
    pub source: String,
    pub diffuse: Diffuse,
    pub additive: bool,
    pub z_write: bool,
    pub cull_cw: bool,
    pub billboard: u32,
    pub fog: bool,
    // FogEnable is an effect state; it does not imply that VS writes oFog.
    pub writes_fog: bool,
}
#[derive(Debug, Default)]
pub(super) struct Assets {
    pub materials: BTreeMap<Vec<u8>, Material>,
    pub textures: BTreeMap<Vec<u8>, Arc<RgbaFrame>>,
    programs: BTreeMap<(u32,u32), std::result::Result<Program,String>>,
    pub(super) linear_sampling: bool,
}

fn text(bytes: &[u8]) -> anyhow::Result<&str> {
    Ok(std::str::from_utf8(bytes)?.trim())
}
fn normalize(line: &str) -> String {
    line.chars().filter(|c| !c.is_ascii_whitespace()).flat_map(char::to_lowercase).collect()
}
fn fields(lines: &[String]) -> anyhow::Result<BTreeMap<String,String>> {
    let mut out = BTreeMap::new();
    for line in lines {
        let (key,value) = line.split_once('=').ok_or_else(|| anyhow::anyhow!("SHD field lacks '='"))?;
        out.insert(key.trim().to_owned(),value.trim().to_owned());
    }
    Ok(out)
}
fn flags(value: &str, shader_type: bool) -> anyhow::Result<u32> {
    let mut out=0;
    for item in value.split('|').map(str::trim) {
        out |= match (shader_type,item) {
            (_,"0") => 0,
            (false,"P")=>2,(false,"N")=>0x10,(false,"D")=>0x40,(false,"T1")=>0x100,
            (true,"BBY")=>1,(true,"BB")=>2,(true,"OP_ADD")=>0x10,(true,"DL1")=>0x10_0000,
            _=>anyhow::bail!("SHD unsupported selector {item}"),
        };
    }
    Ok(out)
}

/// a3ef0 expands cropped HG pixels before 9fbc0/a5710 uploads the texture.
/// Native bitmap rows use the bottom origin; RgbaFrame rows are top-down.
/// This private texture does not change the shared image/wrapper metadata.
pub(super) fn texture_canvas(frame: RgbaFrame) -> anyhow::Result<RgbaFrame> {
    const MAX_BYTES: usize = 128 * 1024 * 1024;
    if frame.pixels().len() > MAX_BYTES {
        anyhow::bail!("MDL source texture exceeds128MiB");
    }
    let [declared_width, declared_height] = frame.metadata().canvas_size;
    let width = if declared_width == 0 { frame.width() } else { declared_width };
    let height = if declared_height == 0 { frame.height() } else { declared_height };
    // The original bypasses expansion in this case, even with nonzero offsets.
    if (width, height) == frame.dimensions() { return Ok(frame); }
    let size = (width as usize).checked_mul(height as usize)
        .and_then(|size| size.checked_mul(4)).filter(|&size| size <= MAX_BYTES)
        .ok_or_else(|| anyhow::anyhow!("MDL expanded texture exceeds128MiB"))?;
    let mut pixels = vec![0; size];
    let [offset_x, offset_y] = frame.metadata().offset.map(i64::from);
    let top = i64::from(height) - offset_y - i64::from(frame.height());
    let left = offset_x.max(0);
    let right = (offset_x + i64::from(frame.width())).min(i64::from(width));
    let first_row = top.max(0);
    let last_row = (top + i64::from(frame.height())).min(i64::from(height));
    if left < right && first_row < last_row {
        let source_x = (left - offset_x) as usize;
        let row_bytes = (right - left) as usize * 4;
        for y in first_row..last_row {
            let source = ((y - top) as usize * frame.width() as usize + source_x) * 4;
            let destination = (y as usize * width as usize + left as usize) * 4;
            pixels[destination..destination + row_bytes]
                .copy_from_slice(&frame.pixels()[source..source + row_bytes]);
        }
    }
    Ok(RgbaFrame::from_rgba8(width, height, pixels)?)
}

fn compile(name: &str, header: &BTreeMap<String,String>, sections: &[(Vec<u8>,Vec<String>)], fvf:u32, kind:u32) -> anyhow::Result<Program> {
    // PLCommonD/PLBillboardYD use the same real registry and depth surface.
    // Their FVF has no normal, their constant is V_DIFFUSE, and UV is copied.
    let particle = matches!(fvf, 0x42 | 0x142);
    let section = |field: &str| -> anyhow::Result<&[String]> {
        let key=header.get(field).ok_or_else(||anyhow::anyhow!("SHD lacks {field}"))?;
        sections.iter().find(|(name,_)|name==key.as_bytes()).map(|(_,lines)|lines.as_slice())
            .ok_or_else(||anyhow::anyhow!("SHD lacks referenced {field} section"))
    };
    let inputs=fields(section("INPUT")?)?;
    let matrix=match kind&3 {0=>"M_WORLDVIEWPROJ",1=>"M_WVP_BILL_Y",2=>"M_WVP_BILL_XYZ",_=>anyhow::bail!("SHD combined billboard selectors")};
    if inputs.get("0").map(String::as_str)!=Some(matrix) {anyhow::bail!("SHD matrix input has no recovered binding");}
    let source=section("VS")?.iter().map(|s|normalize(s)).collect::<Vec<_>>();
    // Compile these resource equations, never an arbitrary vertex bytecode.
    let color=if source.iter().any(|s|s=="mulod0,r0.x,r1" || s=="mulod0,r0,r1") {
        Diffuse::Directional {alpha_lit:source.iter().any(|s|s=="mulod0,r0.x,r1")}
    } else if source.iter().any(|s|s=="mulod0,v5,c4"||s=="mulod0,c4,v5") {Diffuse::Product}
    else if source.iter().any(|s|s=="movod0,v5") {Diffuse::Vertex}
    else if source.iter().any(|s|s=="movod0,c4") {Diffuse::Material}
    else {anyhow::bail!("SHD diffuse equation is not recovered")};
    let mut expected_inputs=BTreeMap::from([("0".to_owned(),matrix.to_owned()),
        ("10".into(),"M_WORLDVIEW".into()),("14".into(),"V_FOG".into())]);
    if !matches!(color,Diffuse::Vertex)||inputs.contains_key("4") {expected_inputs.insert("4".into(),if particle {"V_DIFFUSE"} else {"V_DIFCOL"}.into());}
    if kind&0x10_0000!=0 {expected_inputs.insert("6".into(),"V_DIRECTIONAL0".into());}
    if fvf&0x100!=0 && !particle {expected_inputs.insert("20".into(),"UV_ADD".into());}
    if inputs!=expected_inputs {anyhow::bail!("SHD constant bindings are not recovered");}
    let mut expected=vec!["vs.1.0".to_owned()];
    if kind&0x10_0000!=0 {expected.push(if fvf&0x100!=0 {"defc10,0.f,0.f,0.f,0.f"} else {"defc10,0.f,0.f,0.f,1.f"}.into());}
    expected.push("m4x4opos,v0,c0".into());
    if kind&0x10_0000!=0 {
        expected.push("dp3r0,v3,-c6".into());
        // Some shipped Common variants compute a dead r0 without max.
        if source.iter().any(|s|s=="maxr0,r0.x,c10") {expected.push("maxr0,r0.x,c10".into());}
        else if source.iter().any(|s|s=="maxr0,r0,c10") {expected.push("maxr0,r0,c10".into());}
    }
    match color {
        Diffuse::Material=>expected.push("movod0,c4".into()),
        Diffuse::Vertex=>expected.push("movod0,v5".into()),
        Diffuse::Product=>expected.push(if source.iter().any(|s|s=="mulod0,c4,v5") {"mulod0,c4,v5"} else {"mulod0,v5,c4"}.into()),
        Diffuse::Directional {alpha_lit}=>{
            expected.extend(["movr1,c4".into(),"mulr1,r1,c7".into()]);
            if fvf&0x40!=0 {expected.push("mulr1,r1,v5".into());}
            expected.push(if alpha_lit {"mulod0,r0.x,r1"} else {"mulod0,r0,r1"}.into());
        }
    }
    if fvf&0x100!=0 {expected.push(if particle {"movot0,v7"} else {"addot0,v7,c20"}.into());}
    expected.extend(["dp4r0.z,v0,c12".into(),"mad ofog.x,r0.z,c14.x,c14.y".replace(' ',"")]);
    let writes_fog=source==expected;
    // Shipped BillboardYAdd INPUT_00 still binds c10/c14, but SOURCE_V_00
    // ends at add oT0. Dworld's light_a meshes actually select this key.
    // Keep the missing output explicit. The fog consumer applies its narrow,
    // measured host compatibility policy; no instructions are appended here.
    let known_unwritten_fog=fvf==0x112 && kind==0x11
        && source.as_slice()==&expected[..expected.len()-2];
    if !writes_fog && !known_unwritten_fog {anyhow::bail!("SHD source equation sequence is not recovered");}
    let effects=section("EFFECT")?.iter().map(|s|normalize(s)).collect::<Vec<_>>();
    let has=|s:&str|effects.iter().any(|v|v==s);
    let textured=fvf&0x100!=0;
    let required=["srcblend=srcalpha;","alphablendenable=true;","alphatestenable=true;","alphafunc=greaterequal;",
        "fillmode=solid;","alpharef=0x01;","colorop[1]=disable;"];
    if required.iter().any(|s|!has(s)) || !has(if kind&0x10!=0 {"destblend=one;"} else {"destblend=invsrcalpha;"})
        || !has(if textured {"colorop[0]=modulate;"} else {"colorop[0]=selectarg1;"})
        || !has(if textured {"alphaop[0]=modulate;"} else {"alphaop[0]=selectarg1;"})
        || !has(if textured {"colorarg1[0]=texture;"} else {"colorarg1[0]=diffuse;"})
        || !has(if textured {"alphaarg1[0]=texture;"} else {"alphaarg1[0]=diffuse;"})
        || (textured&&(!has("texture[0]=<tex0>;")||!has("colorarg2[0]=diffuse;")||!has("alphaarg2[0]=diffuse;"))) {
        anyhow::bail!("SHD effect state is not recovered");
    }
    // Reject extra mutable states rather than silently flattening a custom effect.
    for line in &effects {
        if !line.contains('=') {continue;}
        let known=required.contains(&line.as_str()) || matches!(line.as_str(),
            "texture[0]=<tex0>;"|"cullmode=cw;"|"cullmode=none;"|"zwriteenable=true;"|"zwriteenable=false;"|
            "destblend=one;"|"destblend=invsrcalpha;"|"colorop[0]=modulate;"|"colorop[0]=selectarg1;"|
            "alphaop[0]=modulate;"|"alphaop[0]=selectarg1;"|"colorarg1[0]=texture;"|"colorarg1[0]=diffuse;"|
            "alphaarg1[0]=texture;"|"alphaarg1[0]=diffuse;"|"colorarg2[0]=diffuse;"|"alphaarg2[0]=diffuse;"|
            "addressu[0]=wrap;"|"addressv[0]=wrap;"|"fogenable=<fog>;"|"fogtablemode=<fogt>;"|
            "fogvertexmode=<fogv>;"|"fogstart=<fogn>;"|"fogend=<fogf>;"|"fogdensity=<fogd>;"|"fogcolor=<fogc>;")
            || (particle && matches!(line.as_str(),"edgeantialias=false;"|"lastpixel=false;"|"fogenable=false;"));
        if !known {anyhow::bail!("SHD extra effect state is not recovered: {line}");}
    }
    if !has("cullmode=cw;")&&!has("cullmode=none;") {anyhow::bail!("SHD lacks recovered cull mode");}
    if !has("zwriteenable=true;")&&!has("zwriteenable=false;") {anyhow::bail!("SHD lacks recovered Z write mode");}
    if !has("fogenable=<fog>;") && !has("fogenable=false;") {
        anyhow::bail!("SHD lacks its recovered fog enable state");
    }
    Ok(Program {source:name.into(),diffuse:color,additive:kind&0x10!=0,z_write:has("zwriteenable=true;"),cull_cw:has("cullmode=cw;"),billboard:kind&3,
        fog:has("fogenable=<fog>;"),writes_fog})
}

impl Assets {
    pub(super) fn prepare(&mut self, resources:&ResourceIndex, encoding:crate::ScriptTextEncoding, texture_policy:[u32;2]) -> anyhow::Result<Vec<String>> {
        // 8ef17..8ef93: view copies configured min/mag and calls80540.
        // 80540 maps0/1/2 to D3D POINT/LINEAR/ANISOTROPIC, not identity.
        // The renderer ctor7df2a defaults both to POINT when absent.
        let startup=resources.startup_configuration()?;
        let filter=|key:&str| startup.as_ref().and_then(|xml|crate::xml_integer(xml,key)).unwrap_or(0);
        let min=filter("TEXTURE/filter_min");
        let mag=filter("TEXTURE/filter_mag");
        anyhow::ensure!(min<=1 && mag<=1,"MDL anisotropic/unknown texture filter lacks its actual sampler");
        anyhow::ensure!(min==mag,"MDL distinct min/mag filters need perspective footprint selection");
        self.linear_sampling=min==1;
        let mut names=BTreeSet::new();
        for file in resources.files() {
            if file.relative_path.extension().is_some_and(|ext|ext.eq_ignore_ascii_case("shd")) {
                if let Some(name)=file.relative_path.file_name().and_then(|s|s.to_str()) {names.insert(name.to_owned());}
            }
        }
        for archive in resources.archives() { for entry in &archive.entries {
            if entry.name.to_ascii_lowercase().ends_with(".shd") {names.insert(entry.name.rsplit(['/', '\\']).next().unwrap_or(&entry.name).to_owned());}
        }}
        if names.len()>512 {anyhow::bail!("MDL shader registry exceeds512 resources");}
        for name in names {
            // Names from unrelated categories must not override the configured SHD category.
            let candidates=resources.category_names(b"shd",&name)?;
            let mut bytes=None;
            for candidate in candidates {if let Some(found)=resources.read_mode(&candidate,cs2_runtime::resource_router::ResourceMode::LooseFirst)? {bytes=Some(found);break;}}
            let Some(bytes)=bytes else {continue;};
            if bytes.get(..4)!=Some(b"SHD\0") {anyhow::bail!("MDL shader resource has an invalid signature");}
            let count=word(&bytes,4)? as usize;
            if count>4096 || 8+count*24>bytes.len() {anyhow::bail!("SHD directory exceeds bounds");}
            let mut sections=Vec::new();
            for at in (8..8+count*24).step_by(24) {
                let section=string(&bytes[at..at+16])?;
                let lines=word(&bytes,at+16)? as usize;
                let mut offset=word(&bytes,at+20)? as usize;
                if lines>65536||offset<8+count*24||offset>bytes.len() {anyhow::bail!("SHD section exceeds bounds");}
                let mut parsed=Vec::new();
                for _ in 0..lines {
                    let value=string(&bytes[offset..])?;offset+=value.len()+1;
                    parsed.push(text(&value)?.to_owned());
                }
                sections.push((section,parsed));
            }
            for (_,lines) in sections.iter().filter(|(key,_)|key==b"SHADER") {
                let header=fields(lines)?;
                let fvf=header.get("FVF").ok_or_else(||anyhow::anyhow!("SHD lacks FVF")).and_then(|s|flags(s,false));
                let kind=header.get("TYPE").ok_or_else(||anyhow::anyhow!("SHD lacks TYPE")).and_then(|s|flags(s,true));
                // Unsupported equations remain diagnostics for their exact key.
                if let (Ok(fvf),Ok(kind))=(fvf,kind) {
                    self.programs.entry((fvf,kind)).or_insert_with(||compile(&name,&header,&sections,fvf,kind).map_err(|error|format!("{name}: {error}")));
                }
            }
        }
        let mut total=0usize;
        let mut absent=Vec::new();
        for material in self.materials.values() {
            for name in &material.textures {
                if self.textures.contains_key(name) {continue;}
                // 48e705 ignores a failed49f7b0 request. An absent source
                // creates no cache entry and does not unload the MDL graph.
                // Keep the real router/patch order; only None is absence.
                let candidates=resources.category_names(b"texture",&encoding.decode(name))?;
                let mut bytes=None;
                for candidate in &candidates {
                    if let Some(found)=resources.read_mode(candidate,cs2_runtime::resource_router::ResourceMode::LooseFirst)? {
                        bytes=Some(found);break;
                    }
                }
                let Some(bytes)=bytes else {
                    absent.push(format!("texture={name:?} candidates={candidates:?}; source absent, no cache insertion"));
                    continue;
                };
                let frame=texture_canvas(crate::image_manager::decode_image_frame(&bytes,0)?
                    .ok_or_else(||anyhow::anyhow!("MDL texture lacks frame0"))?)?;
                // Keep the original byte name/cache identity. Reduced pixels
                // go through the same normalized UV and SHD triangle sink.
                let frame=texture::reduce(frame,texture_policy)?;
                total=total.checked_add(frame.pixels().len()).filter(|&size|size<=256*1024*1024)
                    .ok_or_else(||anyhow::anyhow!("MDL texture working set exceeds256MiB"))?;
                self.textures.insert(name.clone(),Arc::new(frame));
            }
        }
        Ok(absent)
    }
    pub(super) fn program(&self,fvf:u32,features:u32,material:&Material)->anyhow::Result<&Program> {
        if material.textures.len()>1 {anyhow::bail!("MDL multiple texture-stage material is not recovered");}
        let program=self.programs.get(&(fvf,features)).ok_or_else(||anyhow::anyhow!("MDL no SHD variant for FVF={fvf:#x}, TYPE={features:#x}"))?
            .as_ref().map_err(|error|anyhow::anyhow!("{error}"))?;
        if !material.shader.is_empty() {
            let name=program.source.strip_suffix(".shd").unwrap_or(&program.source).as_bytes();
            if !name.eq_ignore_ascii_case(&material.shader) {anyhow::bail!("MDL explicit material shader binding is not recovered");}
        }
        Ok(program)
    }
    pub(super) fn particle_program(&self, features:u32)->anyhow::Result<&Program> {
        // 48dcf4 requests142, not the particle VM's pretransformed FVF.
        let program=self.programs.get(&(0x142,features))
            .ok_or_else(||anyhow::anyhow!("kcView particle lacks SHD FVF142 TYPE={features:#x}"))?
            .as_ref().map_err(|error|anyhow::anyhow!("{error}"))?;
        if !matches!(program.diffuse,Diffuse::Product) {
            anyhow::bail!("kcView textured particle SHD lacks its recovered diffuse equation");
        }
        Ok(program)
    }
}
