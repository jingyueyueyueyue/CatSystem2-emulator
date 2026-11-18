//! CPU movie image rectangles in the same sorted scene pass as ordinary nodes.
//! Projection and clipping are portable; decoder and device ownership stay in desktop.
use crate::movie_plane::{IDENTITY, MoviePresentation, PlaneCamera, PlaneProperties};
use crate::{HeadlessPlayer, MpegMovieHost, SceneNode};
use anyhow::{Context, Result, bail};
use cs2_renderer::{
    BlendMode, Canvas, Color, ImageDrawOptions, Point, Rect, RgbaFrame, ScaleFilter,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct MoviePlaneProjection {
    pub center: [f32; 2],
    pub camera_depth: f32,
    pub translation: [f32; 2],
    pub matrix: [f32; 16],
    pub parent: [f32; 16],
    pub perspective: bool,
}

impl Default for MoviePlaneProjection {
    fn default() -> Self {
        Self {
            center: [0.; 2],
            camera_depth: 0.,
            translation: [0.; 2],
            matrix: IDENTITY,
            parent: IDENTITY,
            perspective: false,
        }
    }
}

impl MpegMovieHost {
    /// Final public software surface for an exclusive movie. Native788's
    /// rectangle and black surround are shared by GDI/GPU/JNI pixel consumers.
    /// A composed frame is not acknowledged as presented until its real host
    /// has successfully displayed it.
    pub(super) fn draw_exclusive_movie(&mut self, canvas: &mut Canvas) -> Result<bool> {
        let surface = self.frame_surface();
        if !surface.displaying_movie() {
            return Ok(false);
        }
        canvas.clear(Color::rgb(0, 0, 0));
        self.planes.presentations.clear();
        let Some(snapshot) = self.exclusive_draw_snapshot()? else {
            return Ok(true);
        };
        let crate::movie_plane::MovieDrawSnapshot {
            surface,
            playback,
            frame,
            rectangle,
        } = snapshot;
        let [x, y, width, height] =
            rectangle.unwrap_or([0, 0, canvas.width() as i32, canvas.height() as i32]);
        if width > 0 && height > 0 {
            let source = RgbaFrame::from_rgba8(frame.width, frame.height, frame.rgba.to_vec())?;
            let mut options = ImageDrawOptions::new(Point::new(x, y));
            options.scale_x = width as f32 / frame.width as f32;
            options.scale_y = height as f32 / frame.height as f32;
            options.filter = ScaleFilter::Linear;
            canvas
                .draw_frame(&source, &options)
                .context("compose native788 movie rectangle")?;
            self.record_movie_presentation(MoviePresentation {
                surface,
                playback,
                time: frame.presentation_time,
            });
        }
        Ok(true)
    }

    pub fn movie_plane_depth(&self, target: u32) -> Option<f32> {
        self.plane_target_handle(target)?;
        self.planes
            .properties
            .get(&target)
            .map(|properties| properties.base[2])
    }

    /// Begin an existing mixed scene pass. Returned handles were detached
    /// because their task or drawable disappeared and should leave script maps.
    pub(super) fn begin_movie_plane_frame(
        &mut self,
        nodes: &BTreeMap<u32, SceneNode>,
    ) -> Result<Vec<u32>> {
        let stale = self.reconcile_movie_planes(nodes)?;
        self.poll()?;
        self.planes.presentations.clear();
        // Freeze all decoder surfaces before any scene depth is captured.
        // Some(empty) is intentional: a first frame arriving during composition
        // belongs to the next boundary, rather than appearing in only one pass.
        let mut snapshots = BTreeMap::new();
        for (&handle, movie) in &self.planes.movies {
            if let Some(snapshot) =
                crate::movie_plane::MovieDrawSnapshot::read(movie.player.frame_surface())?
            {
                snapshots.insert(handle, snapshot);
            }
        }
        let exclusive = crate::movie_plane::MovieDrawSnapshot::read(self.frame_surface())?;
        self.planes.frame_snapshots = snapshots;
        self.planes.exclusive_snapshot = exclusive;
        self.planes.snapshot_active = true;
        Ok(stale)
    }

    /// Submit one movie plane in the caller's existing sorted graphics pass.
    /// True consumes the node even if stopped before its first frame/hidden,
    /// preventing a fallback solid rectangle from replacing a missing texture.
    pub(super) fn draw_movie_plane(
        &mut self,
        canvas: &mut Canvas,
        node: &SceneNode,
        nodes: &BTreeMap<u32, SceneNode>,
    ) -> Result<bool> {
        let Some(handle) = self.plane_target_handle(node.id) else {
            return Ok(false);
        };
        if !node.visible || !node.initialized {
            return Ok(true);
        }
        let movie = &self.planes.movies[&handle];
        let Some(snapshot) = self.plane_draw_snapshot(handle)? else {
            return Ok(true);
        };
        let crate::movie_plane::MovieDrawSnapshot {
            surface,
            playback,
            frame,
            rectangle,
        } = snapshot;
        let mut properties = self
            .planes
            .properties
            .get(&node.id)
            .cloned()
            .unwrap_or_default();
        let mut node = node.clone();
        if let Some(offset) = properties.parent_offset
            && let Some(parent) = nodes.get(&node.parent)
        {
            if !parent.visible {
                return Ok(true);
            }
            let base = self
                .planes
                .properties
                .get(&parent.id)
                .map_or([0., 0., 5000.], |properties| properties.base);
            properties.base = [
                base[0] - offset[0],
                base[1] - offset[1],
                base[2] + offset[2],
            ];
            node.position = parent.position;
        }
        let camera = self
            .planes
            .cameras
            .get(&properties.camera)
            .cloned()
            .unwrap_or_default();
        let (at, size) = match rectangle {
            Some([x, y, width, height]) => {
                if width <= 0 || height <= 0 {
                    return Ok(true);
                }
                (
                    [x as f32, y as f32, movie.at[2]],
                    [width as f32, height as f32],
                )
            }
            None => (movie.at, movie.size),
        };
        let image = RgbaFrame::from_rgba8(frame.width, frame.height, frame.rgba.to_vec())?;
        if draw_movie(
            canvas,
            &image,
            at,
            size,
            &node,
            &properties,
            &camera,
            &self.planes.projection,
        )? {
            self.record_movie_presentation(MoviePresentation {
                surface,
                playback,
                time: frame.presentation_time,
            });
        }
        Ok(true)
    }

    pub fn set_movie_plane_projection(&mut self, projection: MoviePlaneProjection) {
        self.planes.projection = projection;
    }

    pub fn set_movie_plane_clip(&mut self, target: u32, clip: Option<Rect>) {
        self.planes.properties.entry(target).or_default().clip = clip;
    }
}

impl HeadlessPlayer {
    /// Call only after the app's real pixel surface/texture was presented.
    /// Merely requesting a report or composing a Canvas is not presentation.
    pub fn acknowledge_movie_frame(&self) -> Result<()> {
        for receipt in self.movie_host.scene_movie_presentations() {
            receipt
                .surface
                .mark_presented(receipt.playback, receipt.time)?;
        }
        Ok(())
    }

    /// Replace the old final SceneNode rectangle loop with this call. This is
    /// used by software frames and the window, not an extra window-only overlay.
    pub(super) fn render_scene_nodes_with_movies(&mut self) -> Result<()> {
        for handle in self.movie_host.reconcile_movie_planes(&self.scene_nodes)? {
            self.movie_playbacks.remove(&handle);
            self.movie_resources.remove(&handle);
        }
        self.movie_host.poll().context("poll scene movie players")?;
        render_nodes(&mut self.canvas, &self.scene_nodes, &mut self.movie_host)
    }

    /// Task-tree release adapters can call this immediately at actual release.
    pub(super) fn release_task_movies(&mut self, target: u32) -> Result<()> {
        if let Some(handle) = self.movie_host.plane_target_handle(target) {
            self.movie_playbacks.remove(&handle);
            self.movie_resources.remove(&handle);
        }
        self.movie_host.release_plane(target)?;
        self.movie_host.planes.properties.remove(&target);
        self.movie_host.planes.cameras.remove(&target);
        Ok(())
    }
}

enum Draw {
    Rectangle {
        rect: Rect,
        color: Color,
    },
    Movie {
        frame: RgbaFrame,
        at: [f32; 3],
        size: [f32; 2],
        node: SceneNode,
        properties: PlaneProperties,
        camera: PlaneCamera,
        receipt: MoviePresentation,
    },
}

struct Entry {
    depth: f32,
    sequence: u64,
    draw: Draw,
}

fn render_nodes(
    canvas: &mut Canvas,
    nodes: &BTreeMap<u32, SceneNode>,
    host: &mut MpegMovieHost,
) -> Result<()> {
    host.planes.presentations.clear();
    let mut entries = Vec::new();
    for node in nodes.values() {
        if !node.visible || !node.initialized {
            continue;
        }
        let movie = host
            .plane_target_handle(node.id)
            .and_then(|handle| host.planes.movies.get(&handle).map(|movie| (handle, movie)));
        if let Some((handle, movie)) = movie {
            let Some(snapshot) = host.plane_draw_snapshot(handle)? else {
                continue;
            };
            let crate::movie_plane::MovieDrawSnapshot {
                surface,
                playback,
                frame,
                rectangle,
            } = snapshot;
            let mut properties = host
                .planes
                .properties
                .get(&node.id)
                .cloned()
                .unwrap_or_default();
            let mut node = node.clone();
            if let Some(offset) = properties.parent_offset
                && let Some(parent) = nodes.get(&node.parent)
            {
                if !parent.visible {
                    continue;
                }
                let base = host
                    .planes
                    .properties
                    .get(&parent.id)
                    .map_or([0., 0., 5000.], |parent| parent.base);
                properties.base = [
                    base[0] - offset[0],
                    base[1] - offset[1],
                    offset[2] + base[2],
                ];
                node.position = parent.position;
            }
            let camera = host
                .planes
                .cameras
                .get(&properties.camera)
                .cloned()
                .unwrap_or_default();
            let (at, size) = match rectangle {
                Some([x, y, width, height]) => {
                    if width <= 0 || height <= 0 {
                        continue;
                    }
                    (
                        [x as f32, y as f32, movie.at[2]],
                        [width as f32, height as f32],
                    )
                }
                None => (movie.at, movie.size),
            };
            entries.push(Entry {
                depth: properties.base[2],
                sequence: properties.sequence,
                draw: Draw::Movie {
                    frame: RgbaFrame::from_rgba8(frame.width, frame.height, frame.rgba.to_vec())?,
                    at,
                    size,
                    node,
                    properties,
                    camera,
                    receipt: MoviePresentation {
                        surface,
                        playback,
                        time: frame.presentation_time,
                    },
                },
            });
        } else {
            entries.push(Entry {
                depth: node.position[2],
                sequence: u64::from(node.id),
                draw: Draw::Rectangle {
                    rect: Rect::new(
                        node.position[0] as i32,
                        node.position[1] as i32,
                        node.size[0].max(0.) as u32,
                        node.size[1].max(0.) as u32,
                    ),
                    color: argb(node.color),
                },
            });
        }
    }
    // Ordered comparison intentionally does not turn NaN depths into integers.
    // Insert equal-depth movies in their stable registration order.
    for index in 1..entries.len() {
        let mut cursor = index;
        while cursor > 0
            && (entries[cursor - 1].depth > entries[cursor].depth
                || entries[cursor - 1].depth == entries[cursor].depth
                    && entries[cursor - 1].sequence > entries[cursor].sequence)
        {
            entries.swap(cursor - 1, cursor);
            cursor -= 1;
        }
    }
    for entry in entries {
        match entry.draw {
            Draw::Rectangle { rect, color } => canvas.fill_rect(rect, color)?,
            Draw::Movie {
                frame,
                at,
                size,
                node,
                properties,
                camera,
                receipt,
            } => {
                if draw_movie(
                    canvas,
                    &frame,
                    at,
                    size,
                    &node,
                    &properties,
                    &camera,
                    &host.planes.projection,
                )? {
                    host.record_movie_presentation(receipt);
                }
            }
        }
    }
    Ok(())
}

fn argb(value: u32) -> Color {
    Color::rgba(
        (value >> 16) as u8,
        (value >> 8) as u8,
        value as u8,
        (value >> 24) as u8,
    )
}

#[derive(Clone, Copy)]
struct Vertex {
    p: [f64; 3],
    rhw: f64,
    uv_rhw: [f64; 2],
}

#[allow(clippy::too_many_arguments)]
fn draw_movie(
    canvas: &mut Canvas,
    frame: &RgbaFrame,
    at: [f32; 3],
    size: [f32; 2],
    node: &SceneNode,
    properties: &PlaneProperties,
    camera: &PlaneCamera,
    projection: &MoviePlaneProjection,
) -> Result<bool> {
    let local = [
        [at[0], at[1], at[2]],
        [at[0] + size[0], at[1], at[2]],
        [at[0], at[1] + size[1], at[2]],
        [at[0] + size[0], at[1] + size[1], at[2]],
    ];
    let uv = [[0., 0.], [1., 0.], [0., 1.], [1., 1.]];
    let matrix = object_matrix(node.scale, properties)?;
    let mut vertices = [Vertex {
        p: [0.; 3],
        rhw: 1.,
        uv_rhw: [0.; 2],
    }; 4];
    for index in 0..4 {
        let mut local_point = [
                local[index][0] - properties.base[0],
                local[index][1] - properties.base[1],
                local[index][2],
                1.,
            ];
        if let Some(callback) = properties.raster {
            callback.apply(&mut local_point)?;
        }
        let mut point = transform(local_point, matrix);
        for axis in 0..3 {
            point[axis] += node.position[axis];
        }
        point[3] += 1.;
        point = transform(point, projection.parent);
        let projected = project(point, properties, node, camera, projection);
        if projected.iter().any(|value| !value.is_finite()) {
            return Ok(false); // Non-finite transformed primitives are not submitted.
        }
        vertices[index] = Vertex {
            p: [
                projected[0] as f64,
                projected[1] as f64,
                projected[2] as f64,
            ],
            rhw: projected[3] as f64,
            uv_rhw: [
                uv[index][0] * projected[3] as f64,
                uv[index][1] * projected[3] as f64,
            ],
        };
    }
    let color = argb(properties.color.unwrap_or(node.color));
    let mut drew = false;
    for indexes in [[0, 1, 2], [2, 1, 3]] {
        let mut polygon = indexes.map(|index| vertices[index]).to_vec();
        polygon = clip_depth(&polygon, 0., true);
        polygon = clip_depth(&polygon, 1., false);
        for index in 1..polygon.len().saturating_sub(1) {
            drew |= triangle(
                canvas,
                frame,
                [polygon[0], polygon[index], polygon[index + 1]],
                color,
                properties.clip,
                properties.blend,
            )?;
        }
    }
    Ok(drew)
}

fn object_matrix(scale: [f32; 3], properties: &PlaneProperties) -> Result<[f32; 16]> {
    if scale
        .iter()
        .chain(properties.rotation.iter())
        .any(|value| !value.is_finite())
    {
        bail!("movie-plane transform has a non-finite scale/rotation");
    }
    let mut matrix = IDENTITY;
    matrix[0] = scale[0];
    matrix[5] = scale[1];
    matrix[10] = scale[2];
    if let Some(explicit) = properties.matrix {
        return Ok(multiply(matrix, explicit));
    }
    for axis in 0..3 {
        let (sine, cosine) = properties.rotation[axis].sin_cos();
        let mut rotation = IDENTITY;
        let a = (axis + 1) % 3;
        let b = (axis + 2) % 3;
        rotation[a * 4 + a] = cosine;
        rotation[b * 4 + b] = cosine;
        rotation[a * 4 + b] = sine;
        rotation[b * 4 + a] = -sine;
        matrix = multiply(matrix, rotation);
    }
    Ok(matrix)
}

fn multiply(left: [f32; 16], right: [f32; 16]) -> [f32; 16] {
    std::array::from_fn(|index| {
        let row = index / 4;
        let column = index % 4;
        (0..4).fold(0., |sum, inner| {
            sum + left[row * 4 + inner] * right[inner * 4 + column]
        })
    })
}

fn transform(point: [f32; 4], matrix: [f32; 16]) -> [f32; 4] {
    std::array::from_fn(|column| {
        (0..4).fold(0., |sum, row| sum + point[row] * matrix[row * 4 + column])
    })
}

fn project(
    mut point: [f32; 4],
    properties: &PlaneProperties,
    node: &SceneNode,
    camera: &PlaneCamera,
    view: &MoviePlaneProjection,
) -> [f32; 4] {
    if view.perspective && point[2] != 0. {
        let distance = (65000. - properties.base[2]) - view.camera_depth;
        let denominator = if point[2] == distance {
            1.
        } else {
            point[2] - distance
        };
        let ratio = (view.camera_depth / denominator).max(0.);
        let origin = match properties.perspective_origin {
            0 => [node.position[0], node.position[1]],
            1 => view.center,
            _ => [0.; 2],
        };
        point = [
            (point[0] - origin[0]) * ratio + origin[0],
            (point[1] - origin[1]) * ratio + origin[1],
            point[2] / 65000.,
            1. / denominator,
        ];
    } else {
        point[2] = (65000. - (point[2] + properties.base[2])) * (1. / 65000.);
    }
    let depth = point[2];
    point[0] = (point[0] - view.center[0]) * camera.scale[0];
    point[1] = (point[1] - view.center[1]) * camera.scale[1];
    point = transform(point, camera.matrix);
    point[0] += view.center[0] - camera.offset[0];
    point[1] += view.center[1] - camera.offset[1];
    if !view.perspective {
        point[2] = depth;
    }
    point = transform(point, view.matrix);
    point[0] += view.translation[0];
    point[1] += view.translation[1];
    point
}

fn clip_depth(input: &[Vertex], plane: f64, lower: bool) -> Vec<Vertex> {
    let mut output = Vec::new();
    let Some(mut previous) = input.last().copied() else {
        return output;
    };
    let inside = |vertex: Vertex| {
        if lower {
            vertex.p[2] >= plane
        } else {
            vertex.p[2] <= plane
        }
    };
    for &current in input {
        if inside(previous) != inside(current) {
            let weight = (plane - previous.p[2]) / (current.p[2] - previous.p[2]);
            output.push(Vertex {
                p: std::array::from_fn(|axis| {
                    previous.p[axis] + (current.p[axis] - previous.p[axis]) * weight
                }),
                rhw: previous.rhw + (current.rhw - previous.rhw) * weight,
                uv_rhw: std::array::from_fn(|axis| {
                    previous.uv_rhw[axis] + (current.uv_rhw[axis] - previous.uv_rhw[axis]) * weight
                }),
            });
        }
        if inside(current) {
            output.push(current);
        }
        previous = current;
    }
    output
}

fn edge(a: Vertex, b: Vertex, x: f64, y: f64) -> f64 {
    (b.p[0] - a.p[0]) * (y - a.p[1]) - (b.p[1] - a.p[1]) * (x - a.p[0])
}

fn triangle(
    canvas: &mut Canvas,
    frame: &RgbaFrame,
    mut vertices: [Vertex; 3],
    color: Color,
    clip: Option<Rect>,
    mode: BlendMode,
) -> Result<bool> {
    let mut area = edge(vertices[0], vertices[1], vertices[2].p[0], vertices[2].p[1]);
    if area == 0. {
        return Ok(false);
    }
    if vertices
        .iter()
        .any(|vertex| !vertex.rhw.is_finite() || vertex.rhw <= 0.)
    {
        bail!("visible movie-plane triangle has a non-positive reciprocal W");
    }
    if area < 0. {
        vertices.swap(1, 2);
        area = -area;
    }
    let mut limits = [0, 0, canvas.width() as i64, canvas.height() as i64];
    if let Some(clip) = clip {
        limits[0] = limits[0].max(i64::from(clip.x));
        limits[1] = limits[1].max(i64::from(clip.y));
        limits[2] = limits[2].min(i64::from(clip.x) + i64::from(clip.width));
        limits[3] = limits[3].min(i64::from(clip.y) + i64::from(clip.height));
    }
    let bounds = |axis: usize| {
        let min = vertices
            .iter()
            .map(|vertex| vertex.p[axis])
            .fold(f64::INFINITY, f64::min)
            .floor() as i64;
        let max = vertices
            .iter()
            .map(|vertex| vertex.p[axis])
            .fold(f64::NEG_INFINITY, f64::max)
            .ceil() as i64;
        (min.max(limits[axis]), max.min(limits[axis + 2]))
    };
    let (left, right) = bounds(0);
    let (top, bottom) = bounds(1);
    let top_left = |a: Vertex, b: Vertex| b.p[1] < a.p[1] || b.p[1] == a.p[1] && b.p[0] > a.p[0];
    let inclusive = [
        top_left(vertices[1], vertices[2]),
        top_left(vertices[2], vertices[0]),
        top_left(vertices[0], vertices[1]),
    ];
    let mut drew = false;
    for y in top..bottom {
        for x in left..right {
            let weights = [
                edge(vertices[1], vertices[2], x as f64 + 0.5, y as f64 + 0.5),
                edge(vertices[2], vertices[0], x as f64 + 0.5, y as f64 + 0.5),
                edge(vertices[0], vertices[1], x as f64 + 0.5, y as f64 + 0.5),
            ];
            if (0..3).any(|index| weights[index] < 0. || weights[index] == 0. && !inclusive[index])
            {
                continue;
            }
            let weights = weights.map(|weight| weight / area);
            let rhw = (0..3)
                .map(|index| vertices[index].rhw * weights[index])
                .sum::<f64>();
            let uv = [0, 1].map(|axis| {
                (0..3)
                    .map(|index| vertices[index].uv_rhw[axis] * weights[index])
                    .sum::<f64>()
                    / rhw
            });
            let sampled = sample(frame, uv);
            let modulation = [color.r, color.g, color.b, color.a];
            let rgba: [u8; 4] = std::array::from_fn(|axis| {
                (sampled[axis] * f64::from(modulation[axis]) / 255.)
                    .round()
                    .clamp(0., 255.) as u8
            });
            canvas.blend_pixel_with_mode(
                x as i32,
                y as i32,
                Color::rgba(rgba[0], rgba[1], rgba[2], rgba[3]),
                mode,
            )?;
            drew = true;
        }
    }
    Ok(drew)
}

fn sample(frame: &RgbaFrame, uv: [f64; 2]) -> [f64; 4] {
    let x = (uv[0] * f64::from(frame.width()) - 0.5).clamp(0., f64::from(frame.width() - 1));
    let y = (uv[1] * f64::from(frame.height()) - 0.5).clamp(0., f64::from(frame.height() - 1));
    let x0 = x.floor() as u32;
    let y0 = y.floor() as u32;
    let x1 = (x0 + 1).min(frame.width() - 1);
    let y1 = (y0 + 1).min(frame.height() - 1);
    let pixel = |x: u32, y: u32, axis: usize| {
        f64::from(frame.pixels()[(y as usize * frame.width() as usize + x as usize) * 4 + axis])
    };
    let wx = x - f64::from(x0);
    let wy = y - f64::from(y0);
    std::array::from_fn(|axis| {
        (pixel(x0, y0, axis) * (1. - wx) + pixel(x1, y0, axis) * wx) * (1. - wy)
            + (pixel(x0, y1, axis) * (1. - wx) + pixel(x1, y1, axis) * wx) * wy
    })
}
