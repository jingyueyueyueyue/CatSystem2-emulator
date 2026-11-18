//! Real group-connected ribbon geometry, target SKFE0..9 non-distortion path.
//! PE ee2a0/ee560/ee600 and renderer1008a9..100ee3. Independent code.
use super::{
    Error, Node, Renderer, Vertex,
    geometry::{ColorCurve, fade_alpha, modulate},
    transform::Matrix,
    tree::Instance,
};

#[derive(Debug, Clone)]
pub(super) struct Ribbon {
    pub color: ColorCurve,
    pub colors: Option<[[u8; 4]; 2]>,
    pub positions: [f32; 2],
    pub viewpoint: bool,
}
impl Ribbon {
    pub fn quads(
        &self,
        particles: &[&Instance],
        node: &Node,
        renderer: &Renderer,
        play_color: [u8; 4],
    ) -> Result<Vec<[Vertex; 4]>, Error> {
        if particles.len() < 2 {
            return Ok(Vec::new());
        }
        // BeginRenderingGroup ee595..ee5a8 samples the group's first UV.
        // Every edge uses that same rect, not the current particle's rect.
        let uv = renderer.uv.value(particles[0].age)?;
        let mut edges = Vec::with_capacity(particles.len());
        for (index, particle) in particles.iter().enumerate() {
            // eeaee..eeb1d applies fade to allColor before corner/global
            // multiplication; renderer color state samples preincrement age.
            let age = particle.render_age;
            let color = fade_alpha(
                self.color.value(age, node.life)?,
                renderer.fade(age, node.life)?,
            )?;
            let v = index as f32 / (particles.len() - 1) as f32 * uv[3] + uv[1];
            let positions = if self.viewpoint {
                camera_edges(particle.world, self.positions)?
            } else {
                self.positions.map(|x| particle.world.point([x, 0.0, 0.0]))
            };
            let edge: [Vertex; 2] = std::array::from_fn(|side| Vertex {
                position: positions[side],
                uv: [uv[0] + if side == 1 { uv[2] } else { 0.0 }, v],
                color: std::array::from_fn(|k| {
                    let edge = self
                        .colors
                        .as_ref()
                        .map_or(color[k], |c| modulate(color[k], c[side][k]));
                    modulate(edge, play_color[k])
                }),
            });
            edges.push(edge);
        }
        Ok(edges
            .windows(2)
            .map(|pair| [pair[0][0], pair[0][1], pair[1][0], pair[1][1]])
            .collect())
    }
}

fn camera_edges(matrix: Matrix, widths: [f32; 2]) -> Result<[[f32; 3]; 2], Error> {
    // PE e2f60 uses sqrt(squareSum)/squareSum as its row multiplier,
    // not division by the norm. Preserve that f32 operation sequence.
    let squares: [f32; 2] = std::array::from_fn(|i| dot(matrix.0[i]));
    if squares.into_iter().any(|v| !v.is_finite() || v <= 0.0) {
        return Err(Error::Host("singular/nonfinite ribbon camera frame"));
    }
    let scales = squares.map(f32::sqrt);
    let up = matrix.0[1].map(|v| v * (scales[1] / squares[1]));
    // Type61 fixes eye(0,0,-100), target0; f43d9..f4403 takes the
    // resulting camera column2. f44e0 supplies it to the actual renderer.
    let camera_z = 100.0 * super::inverse_sqrt(10000.0);
    let forward = normal([0.0, 0.0, -camera_z])?;
    let right = normal(cross(up, forward))?;
    let _forward = normal(cross(right, up))?;
    // PE100a04..100a25 multiplies the LEFT width by scale.X but the
    // RIGHT width by scale.Y. This differs from the public101 template.
    Ok(std::array::from_fn(|side| {
        let x = widths[side] * scales[side];
        std::array::from_fn(|i| -right[i] * x + matrix.0[3][i])
    }))
}
fn dot(v: [f32; 3]) -> f32 {
    v[1] * v[1] + v[0] * v[0] + v[2] * v[2]
}
fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}
fn normal(v: [f32; 3]) -> Result<[f32; 3], Error> {
    let square = dot(v);
    if !square.is_finite() || square <= 0.0 {
        return Err(Error::Host(
            "singular/nonfinite ribbon camera cross product",
        ));
    }
    // Renderer100a65/100b71/100c61 uses this magic seed and two Newton
    // steps. Zero cross products deliberately remain an explicit boundary.
    let inverse = super::inverse_sqrt(square);
    let out = v.map(|component| component * inverse);
    if out.into_iter().all(f32::is_finite) {
        Ok(out)
    } else {
        Err(Error::Host("nonfinite ribbon camera normalization"))
    }
}
