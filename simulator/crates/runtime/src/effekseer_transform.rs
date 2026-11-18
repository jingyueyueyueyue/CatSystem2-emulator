//! Row-vector affine transforms used by the recovered SKFE instance tree.
//! Format/behavior reference: pinned Effekseer101 Matrix43 and Instance;
//! independently implemented. Degenerate decomposition is explicit, not identity.

use super::Error;

#[derive(Debug, Clone, Copy)]
pub(super) struct Matrix(pub [[f32; 3]; 4]);
impl Matrix {
    pub const IDENTITY: Self = Self([[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0], [0.0; 3]]);
    pub fn translation(t: [f32; 3]) -> Self {
        let mut m = Self::IDENTITY;
        m.0[3] = t;
        m
    }
    pub fn scale(s: [f32; 3]) -> Self {
        let mut m = Self::IDENTITY;
        for (i, v) in s.into_iter().enumerate() {
            m.0[i][i] = v;
        }
        m
    }
    pub fn euler(v: [f32; 3]) -> Self {
        // Z, then X, then Y, with row vectors (not XYZ column matrices).
        let (sx, cx) = v[0].sin_cos();
        let (sy, cy) = v[1].sin_cos();
        let (sz, cz) = v[2].sin_cos();
        Self([
            [cz * cy + sz * sx * sy, sz * cx, -cz * sy + sz * sx * cy],
            [-sz * cy + cz * sx * sy, cz * cx, sz * sy + cz * sx * cy],
            [cx * sy, -sx, cx * cy],
            [0.0; 3],
        ])
    }
    pub fn replace_rotation(self, angles: [f32; 3]) -> Result<Self, Error> {
        // d8678..d8748 constructs three axis matrices and multiplies
        // identity*Z*X*Y. e2f60 sums row squares as Y^2+X^2+Z^2.
        let (sx, cx) = super::trig::sin_cos(angles[0])?;
        let (sy, cy) = super::trig::sin_cos(angles[1])?;
        let (sz, cz) = super::trig::sin_cos(angles[2])?;
        let x = Self([[1.0, 0.0, 0.0], [0.0, cx, sx], [0.0, -sx, cx], [0.0; 3]]);
        let y = Self([[cy, 0.0, -sy], [0.0, 1.0, 0.0], [sy, 0.0, cy], [0.0; 3]]);
        let z = Self([[cz, sz, 0.0], [-sz, cz, 0.0], [0.0, 0.0, 1.0], [0.0; 3]]);
        let mut rotation = Self::IDENTITY.then(z).then(x).then(y);
        for i in 0..3 {
            let row = self.0[i];
            let scale = (row[1] * row[1] + row[0] * row[0] + row[2] * row[2]).sqrt();
            for j in 0..3 {
                rotation.0[i][j] *= scale;
            }
        }
        rotation.0[3] = self.0[3];
        if rotation.0.iter().flatten().all(|v| v.is_finite()) {
            Ok(rotation)
        } else {
            Err(Error::Host("nonfinite Effekseer live root rotation"))
        }
    }
    pub fn point(self, p: [f32; 3]) -> [f32; 3] {
        std::array::from_fn(|j| {
            p[0] * self.0[0][j] + p[1] * self.0[1][j] + p[2] * self.0[2][j] + self.0[3][j]
        })
    }
    pub fn axis(a: [f32; 3], angle: f32) -> Self {
        // AxisPVA/Easing stores the sampled axis directly; native does not
        // normalize it. Preserve the resulting nonunit affine matrix.
        let (sn, cs) = angle.sin_cos();
        let d = 1.0 - cs;
        let mut m = Self::IDENTITY;
        for i in 0..3 {
            for j in 0..3 {
                m.0[i][j] = d * a[i] * a[j];
            }
            m.0[i][i] += cs;
        }
        m.0[0][1] += a[2] * sn;
        m.0[0][2] -= a[1] * sn;
        m.0[1][0] -= a[2] * sn;
        m.0[1][2] += a[0] * sn;
        m.0[2][0] += a[1] * sn;
        m.0[2][1] -= a[0] * sn;
        m
    }
    pub fn then(self, other: Self) -> Self {
        let mut out = Self::IDENTITY;
        for i in 0..3 {
            for j in 0..3 {
                out.0[i][j] = (0..3).map(|k| self.0[i][k] * other.0[k][j]).sum();
            }
        }
        out.0[3] = other.point(self.0[3]);
        out
    }
    pub fn scales(self) -> [f32; 3] {
        std::array::from_fn(|i| self.0[i].iter().map(|v| v * v).sum::<f32>().sqrt())
    }
    pub fn split(self) -> Result<([f32; 3], Self, [f32; 3]), Error> {
        let s = self.scales();
        if s.into_iter().any(|v| !v.is_finite() || v == 0.0) {
            return Err(Error::Host(
                "singular/nonfinite SKFE parent or billboard matrix",
            ));
        }
        let mut r = Self::IDENTITY;
        for i in 0..3 {
            for j in 0..3 {
                r.0[i][j] = self.0[i][j] / s[i];
            }
        }
        Ok((s, r, self.0[3]))
    }
    pub fn capture(parent: Self, bind: [i32; 3]) -> Result<Self, Error> {
        let mut m = Self::IDENTITY;
        if bind[0] == 1 {
            m.0[3] = parent.0[3];
        }
        if bind[1] == 1 && bind[2] == 1 {
            m.0[..3].copy_from_slice(&parent.0[..3]);
        } else if bind[1] == 1 {
            let (_, r, _) = parent.split()?;
            m.0[..3].copy_from_slice(&r.0[..3]);
        } else if bind[2] == 1 {
            for (i, v) in parent.scales().into_iter().enumerate() {
                m.0[i][i] = v;
            }
        }
        Ok(m)
    }
    pub fn bind(self, parent: Self, root: Self, bind: [i32; 3]) -> Result<Self, Error> {
        if bind == [2; 3] {
            return Ok(parent);
        }
        if bind == [3; 3] {
            return Ok(root);
        }
        // Native GetSRT normalizes each row independently, including shear;
        // do not orthogonalize it or preserve signed scale artificially.
        let (mut s, mut r, mut t) = self.split()?;
        if matches!(bind[0], 2 | 3) {
            t = if bind[0] == 2 { parent } else { root }.0[3];
        }
        if matches!(bind[1], 2 | 3) {
            r = if bind[1] == 2 { parent } else { root }.split()?.1;
        }
        if matches!(bind[2], 2 | 3) {
            s = if bind[2] == 2 { parent } else { root }.scales();
        }
        let mut m = Self::scale(s).then(r);
        m.0[3] = t;
        Ok(m)
    }
    pub fn geometry_point(self, p: [f32; 3], billboard: i32) -> Result<[f32; 3], Error> {
        if billboard == 2 {
            return Ok(self.point(p));
        }
        let (s, r, t) = self.split()?;
        // Fixed type61 camera faces +Z. Renderer takes F=-camera column2,
        // R=normalize(up cross F), U=normalize(F cross R), then uses -R,U.
        let mut right = [-1.0, 0.0, 0.0];
        let mut up = [0.0, 1.0, 0.0];
        let mut forward = [0.0, 0.0, -1.0];
        if billboard == 3 {
            let cx = (1.0 - r.0[2][1] * r.0[2][1]).sqrt();
            let (sn, cs) = if cx.abs() > 0.05 {
                let sn = -r.0[0][1] / cx;
                let cs = (1.0 - sn * sn).sqrt() * if r.0[1][1] < 0.0 { -1.0 } else { 1.0 };
                (sn, cs)
            } else {
                (0.0, 1.0)
            };
            right = [-cs, sn, 0.0];
            up = [sn, cs, 0.0];
        } else if billboard == 1 {
            up = r.0[1];
            let len = (up[0] * up[0] + up[1] * up[1]).sqrt();
            if len == 0.0 {
                return Err(Error::Host("singular Y-fixed billboard"));
            }
            right = [-up[1] / len, up[0] / len, 0.0];
            forward = [
                right[1] * up[2],
                -right[0] * up[2],
                right[0] * up[1] - right[1] * up[0],
            ];
            let norm = forward.iter().map(|v| v * v).sum::<f32>().sqrt();
            for v in &mut forward {
                *v /= norm;
            }
        }
        let out = std::array::from_fn(|i| {
            t[i] - right[i] * p[0] * s[0] + up[i] * p[1] * s[1] + forward[i] * p[2] * s[2]
        });
        if out.into_iter().any(|v| !v.is_finite()) {
            return Err(Error::Host("nonfinite SKFE billboard output"));
        }
        Ok(out)
    }
}
