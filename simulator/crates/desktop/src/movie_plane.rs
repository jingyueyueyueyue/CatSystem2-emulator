//! Independent movie players attached to PLANE tasks, never to script audio channels.
use crate::SceneNode;
use crate::media_process::error;
use crate::movie::{MovieFrame, MovieFrameSurface, MpegMovieHost};
use cs2_renderer::{BlendMode, Rect};
use cs2_runtime::{MovieHost, MovieLoadResult, MovieRequest, RuntimeResult};
use std::collections::BTreeMap;
use std::sync::Arc;

pub(super) const IDENTITY: [f32; 16] = [
    1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1., 0., 0., 0., 0., 1.,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum PlaneMovieKind {
    Movie,
    Effect,
}

#[derive(Clone, Debug)]
pub(super) struct PlaneProperties {
    pub base: [f32; 3],
    pub rotation: [f32; 3],
    pub matrix: Option<[f32; 16]>,
    pub clip: Option<Rect>,
    pub blend: BlendMode,
    pub color: Option<u32>,
    pub display: u32,
    pub create: u32,
    pub parent_offset: Option<[f32; 3]>,
    pub camera: u32,
    pub perspective_origin: u32,
    pub sequence: u64,
    pub raster: Option<crate::plane_native::RasterVertexTransform>,
}

impl Default for PlaneProperties {
    fn default() -> Self {
        Self {
            base: [0., 0., 5000.],
            rotation: [0.; 3],
            matrix: None,
            clip: None,
            blend: BlendMode::SourceOver,
            color: None,
            display: 1,
            create: 1,
            parent_offset: None,
            camera: 0,
            perspective_origin: 0,
            sequence: 0,
            raster: None,
        }
    }
}

#[derive(Clone, Debug)]
pub(super) struct PlaneCamera {
    pub offset: [f32; 2],
    pub scale: [f32; 2],
    pub matrix: [f32; 16],
}

impl Default for PlaneCamera {
    fn default() -> Self {
        Self {
            offset: [0.; 2],
            scale: [1.; 2],
            matrix: IDENTITY,
        }
    }
}

pub(super) struct PlaneMovie {
    pub target: u32,
    pub kind: PlaneMovieKind,
    pub at: [f32; 3],
    pub size: [f32; 2],
    pub player: Box<MpegMovieHost>,
}

#[derive(Clone)]
pub(super) struct MoviePresentation {
    pub surface: Arc<MovieFrameSurface>,
    pub playback: u64,
    pub sequence: u64,
    pub time: f64,
}

/// One scene boundary's actual decoder frame. Shared Arc pixels keep repeated
/// depth captures consistent without copying pixels into the persistent VRAM.
#[derive(Clone)]
pub(super) struct MovieDrawSnapshot {
    pub surface: Arc<MovieFrameSurface>,
    pub playback: u64,
    pub sequence: u64,
    pub frame: Arc<MovieFrame>,
    pub rectangle: Option<[i32; 4]>,
}

impl MovieDrawSnapshot {
    pub(super) fn read(surface: Arc<MovieFrameSurface>) -> RuntimeResult<Option<Self>> {
        let Some((playback, sequence, frame)) = surface.frame_for_composition()? else {
            return Ok(None);
        };
        let rectangle = surface.display_rectangle()?;
        Ok(Some(Self {
            surface,
            playback,
            sequence,
            frame,
            rectangle,
        }))
    }
}

#[derive(Default)]
pub(super) struct MoviePlanes {
    pub movies: BTreeMap<u32, PlaneMovie>,
    pub properties: BTreeMap<u32, PlaneProperties>,
    pub cameras: BTreeMap<u32, PlaneCamera>,
    pub presentations: Vec<MoviePresentation>,
    pub projection: crate::movie_plane_render::MoviePlaneProjection,
    pub frame_snapshots: BTreeMap<u32, MovieDrawSnapshot>,
    pub exclusive_snapshot: Option<MovieDrawSnapshot>,
    pub snapshot_active: bool,
    pub(super) next_sequence: u64,
}

impl MpegMovieHost {
    pub(super) fn record_movie_presentation(&mut self, receipt: MoviePresentation) {
        #[cfg(target_os = "android")]
        receipt.surface.trace_frame(receipt.playback, 2, "first_composited", receipt.time);
        if let Some(previous) = self.planes.presentations.iter_mut().find(|previous| {
            previous.playback == receipt.playback
                && Arc::ptr_eq(&previous.surface, &receipt.surface)
        }) {
            if receipt.sequence > previous.sequence {
                *previous = receipt;
            }
        } else {
            self.planes.presentations.push(receipt);
        }
    }

    pub(super) fn plane_draw_snapshot(
        &self,
        handle: u32,
    ) -> RuntimeResult<Option<MovieDrawSnapshot>> {
        if self.planes.snapshot_active {
            return Ok(self.planes.frame_snapshots.get(&handle).cloned());
        }
        match self.planes.movies.get(&handle) {
            Some(movie) => MovieDrawSnapshot::read(movie.player.frame_surface()),
            None => Ok(None),
        }
    }

    pub(super) fn exclusive_draw_snapshot(&self) -> RuntimeResult<Option<MovieDrawSnapshot>> {
        if self.planes.snapshot_active {
            Ok(self.planes.exclusive_snapshot.clone())
        } else {
            MovieDrawSnapshot::read(self.frame_surface())
        }
    }

    pub fn movie_plane_shape_kind(&self, target: u32) -> Option<u32> {
        let handle = self.plane_target_handle(target)?;
        Some(match self.planes.movies.get(&handle)?.kind {
            PlaneMovieKind::Movie => 3,
            PlaneMovieKind::Effect => 4,
        })
    }

    pub(super) fn movie_player_mut(&mut self, handle: u32) -> &mut MpegMovieHost {
        if self.planes.movies.contains_key(&handle) {
            return &mut self
                .planes
                .movies
                .get_mut(&handle)
                .expect("checked movie")
                .player;
        }
        self
    }

    pub(super) fn is_plane_movie(&self, handle: u32) -> bool {
        self.planes.movies.contains_key(&handle)
    }

    pub(super) fn plane_movie_handle(&self, target: u32, kind: PlaneMovieKind) -> u32 {
        self.planes
            .movies
            .iter()
            .find_map(|(&handle, movie)| {
                (movie.target == target && movie.kind == kind).then_some(handle)
            })
            .unwrap_or(0)
    }

    pub(super) fn plane_target_handle(&self, target: u32) -> Option<u32> {
        self.planes
            .movies
            .iter()
            .find_map(|(&handle, movie)| (movie.target == target).then_some(handle))
    }

    pub(super) fn release_plane(&mut self, target: u32) -> RuntimeResult<Option<u32>> {
        let Some(handle) = self.plane_target_handle(target) else {
            return Ok(None);
        };
        let mut movie = self
            .planes
            .movies
            .remove(&handle)
            .expect("selected plane movie");
        self.planes.frame_snapshots.remove(&handle);
        let result = movie.player.unload();
        self.planes.presentations.retain(|presentation| {
            !Arc::ptr_eq(&presentation.surface, &movie.player.frame_surface())
        });
        result?;
        Ok(Some(handle))
    }

    pub(super) fn release_plane_handle(&mut self, handle: u32) -> RuntimeResult<()> {
        self.planes.frame_snapshots.remove(&handle);
        if let Some(mut movie) = self.planes.movies.remove(&handle) {
            let result = movie.player.unload();
            self.planes.presentations.retain(|presentation| {
                !Arc::ptr_eq(&presentation.surface, &movie.player.frame_surface())
            });
            result?;
        }
        Ok(())
    }

    pub(super) fn load_plane(
        &mut self,
        handle: u32,
        target: u32,
        kind: PlaneMovieKind,
        resource: String,
        at: [f32; 3],
    ) -> RuntimeResult<Option<[f32; 2]>> {
        if self.planes.movies.len() >= 64 {
            return Err(error(
                "more than 64 simultaneous movie planes are not supported",
            ));
        }
        self.release_plane(target)?;
        let create = self.planes.properties.entry(target).or_default().create;
        if create != 1 || (kind == PlaneMovieKind::Movie && !self.enabled()) {
            return Ok(None);
        }
        let mut player = self.fork_movie_player();
        // Video effects intentionally bypass the ordinary movie-disable switch.
        player.set_enabled(true);
        let metadata = match player.load(MovieRequest {
            resource,
            looped: false,
            muted: false,
        })? {
            MovieLoadResult::Ready(metadata) => metadata,
            MovieLoadResult::Missing | MovieLoadResult::Disabled => return Ok(None),
        };
        let size = [
            f32::from(
                metadata
                    .width
                    .ok_or_else(|| error("movie plane width is absent"))?,
            ),
            f32::from(
                metadata
                    .height
                    .ok_or_else(|| error("movie plane height is absent"))?,
            ),
        ];
        self.planes.next_sequence = self
            .planes
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| error("movie plane registration sequence exhausted"))?;
        let properties = self.planes.properties.entry(target).or_default();
        let create = properties.create;
        *properties = PlaneProperties {
            create,
            sequence: self.planes.next_sequence,
            ..Default::default()
        };
        self.planes.movies.insert(
            handle,
            PlaneMovie {
                target,
                kind,
                at,
                size,
                player: Box::new(player),
            },
        );
        Ok(Some(size))
    }

    pub(super) fn reconcile_movie_planes(
        &mut self,
        nodes: &BTreeMap<u32, SceneNode>,
    ) -> RuntimeResult<Vec<u32>> {
        let stale = self
            .planes
            .movies
            .iter()
            .filter_map(|(&handle, movie)| {
                nodes
                    .get(&movie.target)
                    .is_none_or(|node| !node.initialized)
                    .then_some(handle)
            })
            .collect::<Vec<_>>();
        for handle in &stale {
            self.release_plane_handle(*handle)?;
        }
        self.planes
            .properties
            .retain(|target, _| nodes.contains_key(target));
        self.planes
            .cameras
            .retain(|target, _| nodes.contains_key(target));
        Ok(stale)
    }

    pub(super) fn scene_movie_presentations(&self) -> Vec<MoviePresentation> {
        self.planes.presentations.clone()
    }
}
