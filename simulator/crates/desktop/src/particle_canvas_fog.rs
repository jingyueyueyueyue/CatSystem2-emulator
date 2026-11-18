//! Campanella352 -> manager48cef0 -> SHD V_FOG/oFog -> RGB fog.
//! D3D/fogtype1 selects the shipped vertex-fog path. Device table fog
//! needs its real raster-depth capability and remains an explicit boundary.

#[derive(Clone, Copy, Debug, Default)]
pub(super) struct FogState {
    enabled: bool,
    table: bool,
    range: [u32; 2],
    rgb: [u8; 3],
}

#[derive(Clone, Copy)]
pub(super) struct VertexFog {
    factor: FogFactor,
    pub rgb: [f32; 3],
}

#[derive(Clone, Copy)]
enum FogFactor {
    EyeDepth { scale: f32, offset: f32 },
    CampanellaBillboardYAddHostZero,
}

impl FogState {
    pub(super) fn configure(&mut self, mode: u32, enabled: u32, record: Option<&[u8]>) -> anyhow::Result<()> {
        // 41f678..41f713: invalid configuration values and data0==0
        // disable without dereferencing data1. 48cef0 retains old parameters.
        if mode > 1 || enabled == 0 {
            self.enabled = false;
            return Ok(());
        }
        let record = record.and_then(|bytes| bytes.get(..12))
            .ok_or_else(|| anyhow::anyhow!("MDL fog needs12 owned data1 bytes"))?;
        self.range = [super::word(record, 0)?, super::word(record, 4)?];
        self.rgb.copy_from_slice(&record[8..11]);
        self.table = mode == 0;
        self.enabled = true;
        Ok(())
    }

    pub(super) fn vertex(self, program: bool, object: bool, writes_fog: bool,
        campanella_billboard_y_add: bool) -> anyhow::Result<Option<VertexFog>> {
        // 48e800 combines manager enable and this object's fog DWORD.
        // PL SHD explicitly disables fog, irrespective of the manager state.
        if !self.enabled || !program || !object { return Ok(None); }
        if self.table { anyhow::bail!("MDL device table fog needs its actual depth/capability consumer"); }
        if !writes_fog {
            if !campanella_billboard_y_add {
                anyhow::bail!("MDL active vertex fog consumes an unwritten SHD oFog output without a host compatibility policy");
            }
            // Explicit portable-host compatibility policy for the validated
            // BillboardYAdd 0x112/0x11 equations, not a universal GPU default.
            // archive/2026-11/20261118_fog_probe/observation-x86.json:
            // system D3D8, RTX5070Ti, valid vs1.0/vs1.1 controls; unwritten
            // and explicit0 both ff1040c0, explicit1 ffc04020. The shader
            // does not consume V_FOG, so do not invent an eye-depth formula.
            return Ok(Some(VertexFog { factor: FogFactor::CampanellaBillboardYAddHostZero,
                rgb: self.rgb.map(|v| f32::from(v) / 255.) }));
        }
        let [near, far] = self.range.map(f32::from_bits);
        if !near.is_finite() || !far.is_finite() || far == near {
            anyhow::bail!("MDL consumed fog range has no finite V_FOG constants");
        }
        // 4a16fc: c14=(1/(far-near), far/(far-near),0,0).
        // The two stores round the x87 results before the shader uses them.
        let span = f64::from(far) - f64::from(near);
        let scale = (1. / span) as f32;
        let offset = (f64::from(far) / span) as f32;
        if !scale.is_finite() || !offset.is_finite() {
            anyhow::bail!("MDL V_FOG constants overflow");
        }
        Ok(Some(VertexFog { factor: FogFactor::EyeDepth { scale, offset },
            rgb: self.rgb.map(|v| f32::from(v) / 255.) }))
    }
}

impl VertexFog {
    pub(super) fn factor(self, eye_z: f32) -> anyhow::Result<f32> {
        let (scale, offset) = match self.factor {
            FogFactor::CampanellaBillboardYAddHostZero => return Ok(0.),
            FogFactor::EyeDepth { scale, offset } => (scale, offset),
        };
        // Resource VS: dp4 r0.z,v0,c12; mad oFog.x,r0.z,c14.x,c14.y.
        // Fog output is clamped before its screen-linear interpolation.
        let value = eye_z * scale + offset;
        if !value.is_finite() { anyhow::bail!("MDL vertex fog output is non-finite"); }
        Ok(value.clamp(0., 1.))
    }
}
