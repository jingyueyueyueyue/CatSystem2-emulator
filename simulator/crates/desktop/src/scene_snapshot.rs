use std::{collections::BTreeMap, ops::Range, sync::Arc};

use anyhow::Result;
use cs2_renderer::{Canvas, ImageDrawOptions, Point, Rect, RenderCommand, RenderNode, RenderQueue};

use super::{
    HeadlessPlayer, HostPhase, ScriptTextEncoding, fes_text, image_priority,
    measure_host_operation, plane_native, text::{LoadedFont, draw_message_text},
};

pub(super) struct SceneDrawable {
    pub(super) id: u32,
    pub(super) kind: u32,
    pub(super) depth: f32,
    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
    pub(super) foreign_depth: bool,
}

/// Stable draw inputs shared by all depth ranges of one same-thread composition.
/// The scene and its renderer services stay with the player until composition ends.
pub(super) struct SceneSnapshot {
    drawables: Vec<SceneDrawable>,
    text: Vec<fes_text::FesTextProjection>,
    legacy: LegacyLayers,
}

impl SceneSnapshot {
    pub(super) fn capture(player: &mut HeadlessPlayer, prefix_cached: bool) -> Result<Self> {
        let mut depths = BTreeMap::new();
        let nodes = &player.scene_nodes;
        let movies = &player.movie_host;
        for node in nodes.values() {
            if node.kind == 61 {
                let base = player.data_services.effekseer.manager.plane_base(node.id)?;
                depths.insert(node.id, base[2]);
                continue;
            }
            if movies.plane_target_handle(node.id).is_none() {
                continue;
            }
            let mut depth = movies.movie_plane_depth(node.id).unwrap_or(5000.);
            if let Some(properties) = movies.planes.properties.get(&node.id)
                && let Some(offset) = properties.parent_offset
                && let Some(parent) = nodes.get(&node.parent)
            {
                let parent_base = movies.planes.properties.get(&parent.id)
                    .map_or(5000., |properties| properties.base[2]);
                depth = parent_base + offset[2];
            }
            depths.insert(node.id, depth);
        }

        let planes = &mut player.data_services.planes;
        let drawables = planes.draw_order_with_depths(nodes, &depths).into_iter()
            .filter_map(|id| {
                let node = nodes.get(&id)?;
                if !node.visible || !node.initialized || matches!(node.kind, 35..=39) {
                    return None;
                }
                let depth = depths.get(&id).copied();
                Some(SceneDrawable {
                    id,
                    kind: node.kind,
                    depth: depth.unwrap_or_else(|| plane_native::plane_depth(node, planes)),
                    #[cfg(all(feature = "gpu-raster", any(windows, target_os = "android")))]
                    foreign_depth: depth.is_some(),
                })
            })
            .collect();
        let text = measure_host_operation(HostPhase::ComposeText, || {
            fes_text::scene_projections(&player.data_services, f32::NEG_INFINITY, f32::INFINITY)
        });
        let legacy = LegacyLayers::capture(player, prefix_cached);
        Ok(Self { drawables, text, legacy })
    }

    pub(super) fn drawables<'s>(
        &'s self, range: &'s Range<f32>,
    ) -> impl Iterator<Item = &'s SceneDrawable> + Clone {
        self.drawables.iter().filter(move |drawable| range.contains(&drawable.depth))
    }

    pub(super) fn text<'s>(
        &'s self, range: &'s Range<f32>,
    ) -> impl Iterator<Item = &'s fes_text::FesTextProjection> {
        self.text.iter().filter(move |layer| range.contains(&(layer.priority as f32)))
    }

    pub(super) fn draw_legacy_layers(&self, target: &mut Canvas) -> Result<()> {
        self.legacy.draw(target)
    }
}

struct LegacyMessage {
    font: Arc<[LoadedFont]>,
    speaker: String,
    text: String,
    encoding: ScriptTextEncoding,
}

struct LegacyLayers {
    background: Option<cs2_renderer::RgbaFrame>,
    fes_images: RenderQueue,
    cst_images: RenderQueue,
    message: Option<LegacyMessage>,
}

impl LegacyLayers {
    fn capture(player: &HeadlessPlayer, prefix_cached: bool) -> Self {
        let (background, fes_images, cst_images) = measure_host_operation(HostPhase::ComposeLayers, || {
            let background = if prefix_cached { None } else { player.image_frame.clone() };
            let mut fes_images = RenderQueue::new();
            if let Some(fes) = &player.fes {
                for layer in fes.media.values() {
                    let animation_visible = layer.animation_display.is_none_or(|display| display.visible != 0);
                    if !layer.visible || !animation_visible || !layer.is_drawable() {
                        continue;
                    }
                    let Some(image) = layer.image.as_ref() else { continue };
                    let mut options = ImageDrawOptions::new(layer.draw_position(&fes.planes));
                    options.opacity = layer.animation_display.map_or(layer.opacity, |display| {
                        display.compose((0.0, 0.0), i32::from(layer.opacity), 1).blend.clamp(0, 255) as u8
                    });
                    if let Some(id) = &layer.plane {
                        let Some(plane) = fes.planes.get(id).filter(|plane| plane.visible) else { continue };
                        options.opacity = ((u16::from(options.opacity) * u16::from(plane.opacity) + 127) / 255) as u8;
                        options.clip = Some(Rect::new(plane.position.x, plane.position.y, plane.size[0], plane.size[1]));
                    }
                    fes_images.push(RenderNode::new(layer.priority, RenderCommand::Frame {
                        frame: image.clone(),
                        options,
                    }));
                }
            }
            let mut cst_images = RenderQueue::new();
            if !prefix_cached {
                for (&(kind, bank), images) in &player.cst_images {
                    for image in images {
                        let mut options = ImageDrawOptions::new(image.draw_position());
                        options.opacity = image.opacity;
                        cst_images.push(RenderNode::new(image_priority(kind, bank), RenderCommand::Frame {
                            frame: image.image.clone(),
                            options,
                        }));
                    }
                }
            }
            (background, fes_images, cst_images)
        });
        let message = measure_host_operation(HostPhase::ComposeText, || {
            let cst = player.cst.as_ref().filter(|_| !player.font.is_empty())?;
            let (speaker, text) = cst.display_text();
            Some(LegacyMessage {
                font: player.font.clone(),
                speaker,
                text,
                encoding: cst.display_encoding(),
            })
        });
        Self { background, fes_images, cst_images, message }
    }

    fn draw(&self, target: &mut Canvas) -> Result<()> {
        {
            let _timing = super::measure_host_phase(HostPhase::ComposeLayers);
            if let Some(frame) = &self.background {
                target.draw_frame(frame, &ImageDrawOptions::new(Point::new(0, 0)))?;
            }
            if !self.fes_images.is_empty() {
                target.render_queue(&self.fes_images)?;
            }
            if !self.cst_images.is_empty() {
                target.render_queue(&self.cst_images)?;
            }
        }
        if let Some(message) = &self.message {
            let _timing = super::measure_host_phase(HostPhase::ComposeText);
            draw_message_text(target, &message.font, &message.speaker, &message.text, message.encoding)?;
        }
        Ok(())
    }
}
