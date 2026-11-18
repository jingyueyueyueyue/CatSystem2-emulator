//! Ordinary movie natives, independently implemented from docs/movie.md ABI.
//! 267 is stop; the portable host's pause API remains separate.
use crate::{LoggingHost, expand_kcs_format, native_state_error, read_u32};
use cs2_runtime::{MovieHost, MovieLoadResult, MovieRequest};
use cs2_script::{NativeReturn, Vm};

impl LoggingHost<'_> {
    pub(super) fn decode_movie_name(&self, bytes: &[u8]) -> cs2_script::Result<String> {
        let bytes = crate::trim_nul(bytes);
        if bytes.len() >= 260 {
            return Err(native_state_error(
                "movie resource name exceeds 259 encoded bytes",
            ));
        }
        crate::sound_native::decode_name(self.text_encoding, bytes)
            .map_err(|_| {
                native_state_error("movie resource name is invalid in the declared script encoding")
            })
    }

    pub(super) fn movie_resource_candidates(
        &mut self,
        category: &[u8],
        _prefix: &str,
        name: &str,
    ) -> cs2_script::Result<Vec<String>> {
        let names = self.resources.category_names(category, name)
            .map_err(|cause| record_host_error(self.events, cause))?;
        for name in &names {
            let encoded =
                crate::sound_native::encode_name(self.text_encoding, name).map_err(|_| {
                    native_state_error(
                        "completed movie resource name cannot use the declared script encoding",
                    )
                })?;
            if encoded.len() >= 260 {
                return Err(native_state_error(
                    "completed movie resource name exceeds 259 encoded bytes",
                ));
            }
        }
        Ok(names)
    }

    pub(super) fn movie_options_native(
        &mut self,
        index: u32,
        arguments: &[u8],
    ) -> cs2_script::Result<NativeReturn> {
        if index == 417 && arguments.len() == 4 {
            let requested = read_u32(arguments, 0)? as i32;
            if let Some(host) = self.movie_host.as_deref_mut()
                && host.loaded_has_audio() == Some(true)
            {
                host.set_volume(legacy_movie_gain(requested));
            }
            return Ok(NativeReturn::Continue);
        }
        let required = match index {
            417 | 997 => 8,
            788 => 20,
            998 => 4,
            _ => return Err(native_state_error("unknown movie option native")),
        };
        if arguments.len() != required {
            return Err(native_state_error(
                "movie option native argument byte count is invalid",
            ));
        }
        let handle = read_u32(arguments, 0)?;
        if handle == 0 {
            return Ok(if index == 998 {
                NativeReturn::Value(0)
            } else {
                NativeReturn::Continue
            });
        }
        if !self.movie_resources.contains_key(&handle) {
            return Err(native_state_error(
                "movie option handle is missing or has the wrong type",
            ));
        }
        let effective_volume = if index == 417 {
            self.ensure_legacy_sound_settings()?;
            let block = &self.system_config_blocks[0];
            // Fixed baseline sound-block field offsets, independently read
            // from the settings field layout (not a copied implementation).
            let field = |offset: usize| -> cs2_script::Result<i32> {
                let bytes = block.get(offset..offset + 4).ok_or_else(|| {
                    native_state_error("legacy sound configuration block is truncated")
                })?;
                Ok(i32::from_le_bytes(bytes.try_into().unwrap()))
            };
            let requested = read_u32(arguments, 4)? as i32;
            if field(0x15e8)? != 0 || field(0x15ec)? != 0 {
                0
            } else {
                let background = field(0x15d8)?.wrapping_mul(field(0x15d4)?) / 100;
                requested.wrapping_mul(background) / 100
            }
        } else {
            100
        };
        let events = &mut *self.events;
        let host = self
            .movie_host
            .as_deref_mut()
            .ok_or_else(|| native_state_error("movie options have no real movie host"))?;
        let host = host.movie_player_mut(handle);
        match index {
            417 => host.set_volume(effective_volume),
            788 => host
                .set_display_rectangle([
                    read_u32(arguments, 4)? as i32,
                    read_u32(arguments, 8)? as i32,
                    read_u32(arguments, 12)? as i32,
                    read_u32(arguments, 16)? as i32,
                ])
                .map_err(|cause| record_host_error(events, cause))?,
            997 => host.set_game_drawing(read_u32(arguments, 4)? != 0),
            998 => {
                return host
                    .duration_millis()
                    .map(NativeReturn::Value)
                    .map_err(|cause| record_host_error(events, cause));
            }
            _ => unreachable!(),
        }
        Ok(NativeReturn::Continue)
    }

    pub(super) fn load_movie_native(
        &mut self,
        arguments: &[u8],
        vm: &mut Vm,
    ) -> cs2_script::Result<NativeReturn> {
        if arguments.len() != 4 {
            return Err(native_state_error(
                "native 265 requires exactly one name-format address",
            ));
        }
        let source = vm.read_string(read_u32(arguments, 0)?)?;
        let expanded = expand_kcs_format(vm, &source, 0)?;
        let name = self.decode_movie_name(&expanded)?;
        let candidates = self.movie_resource_candidates(b"movie", "movie", &name)?;
        let events = &mut *self.events;
        let host = self
            .movie_host
            .as_deref_mut()
            .ok_or_else(|| native_state_error("native 265 has no real movie host"))?;
        host.unload()
            .map_err(|cause| record_host_error(events, cause))?;
        self.movie_playbacks
            .retain(|handle, _| host.is_plane_movie(*handle));
        self.movie_resources
            .retain(|handle, _| host.is_plane_movie(*handle));
        if !host.enabled() {
            return Ok(NativeReturn::Value(0));
        }
        // Source absence is reported by the actual movie host after routing.
        // Probing candidates here would swallow routing errors and could make
        // a configured miss succeed through an unrelated guessed filename.
        let resource = candidates.into_iter().next();
        let Some(resource) = resource else {
            return Ok(NativeReturn::Value(0));
        };
        match host
            .load(MovieRequest {
                resource: resource.clone(),
                looped: false,
                muted: false,
            })
            .map_err(|cause| record_host_error(events, cause))?
        {
            MovieLoadResult::Missing | MovieLoadResult::Disabled => {
                return Ok(NativeReturn::Value(0));
            }
            MovieLoadResult::Ready(_) => {}
        }
        let handle = *self.next_xml_handle;
        *self.next_xml_handle = handle
            .checked_add(1)
            .ok_or_else(|| native_state_error("movie resource handle space exhausted"))?;
        if handle == 0 {
            return Err(native_state_error("movie resource handle zero is reserved"));
        }
        self.movie_resources.insert(handle, resource.clone());
        host.show_on_window();
        if let Some(event) = events.last_mut() {
            event.detail.push_str(&format!(
                " movie_load handle={handle} resource={resource:?}"
            ));
        }
        Ok(NativeReturn::Value(handle))
    }

    pub(super) fn control_movie_native(
        &mut self,
        index: u32,
        arguments: &[u8],
    ) -> cs2_script::Result<NativeReturn> {
        let implicit = arguments.is_empty() && (266..=270).contains(&index);
        let expected = if index == 266 { 8 } else { 4 };
        if !implicit && arguments.len() != expected {
            return Err(native_state_error(if index == 266 {
                "native 266 requires 0 or 8 argument bytes"
            } else {
                "native movie control requires 0 or 4 argument bytes"
            }));
        }
        let handle = if implicit {
            self.movie_host.as_deref().map_or(0, |host| {
                self.movie_resources
                    .keys()
                    .copied()
                    .find(|handle| !host.is_plane_movie(*handle))
                    .unwrap_or(0)
            })
        } else {
            read_u32(arguments, 0)?
        };
        if handle == 0 {
            return Ok(if index == 270 {
                NativeReturn::Continue
            } else {
                NativeReturn::Value(0)
            });
        }
        // Nonzero release succeeds even for an already released handle.
        if !self.movie_resources.contains_key(&handle) {
            return Ok(if index == 270 {
                NativeReturn::Continue
            } else {
                NativeReturn::Value(u32::from(index == 268))
            });
        }
        let events = &mut *self.events;
        let host = self
            .movie_host
            .as_deref_mut()
            .ok_or_else(|| native_state_error("movie control has no real movie host"))?;
        if index == 268 {
            self.movie_playbacks.remove(&handle);
            self.movie_resources.remove(&handle);
            if host.is_plane_movie(handle) {
                if let Some(movie) = host.planes.movies.get(&handle)
                    && let Some(node) = self.scene_nodes.get_mut(&movie.target)
                {
                    node.initialized = false;
                    node.size = [0.; 2];
                }
                host.release_plane_handle(handle)
                    .map_err(|cause| record_host_error(events, cause))?;
            } else {
                host.unload()
                    .map_err(|cause| record_host_error(events, cause))?;
            }
            return Ok(NativeReturn::Value(1));
        }
        let host = host.movie_player_mut(handle);
        let playback = self.movie_playbacks.get(&handle).copied();
        match index {
            266 => {
                if let Some(playback) = playback
                    && !host
                        .finished(playback)
                        .map_err(|cause| record_host_error(events, cause))?
                {
                    return Ok(NativeReturn::Value(0));
                }
                if let Some(playback) = self.movie_playbacks.remove(&handle) {
                    host.release(playback)
                        .map_err(|cause| record_host_error(events, cause))?;
                }
                let count = if implicit { 1 } else { read_u32(arguments, 4)? };
                let resource = self
                    .movie_resources
                    .get(&handle)
                    .expect("checked above")
                    .clone();
                let started = host
                    .play_count(
                        MovieRequest {
                            resource,
                            looped: count == 0 || count == u32::MAX,
                            muted: false,
                        },
                        count,
                    )
                    .map_err(|cause| record_host_error(events, cause))?;
                self.movie_playbacks.insert(handle, started.handle);
                Ok(NativeReturn::Value(1))
            }
            267 => {
                let playing = if let Some(playback) = playback {
                    let playing = !host
                        .finished(playback)
                        .map_err(|cause| record_host_error(events, cause))?;
                    if playing || !implicit {
                        host.stop(playback)
                            .map_err(|cause| record_host_error(events, cause))?;
                    }
                    playing
                } else {
                    false
                };
                Ok(NativeReturn::Value(u32::from(playing)))
            }
            269 => {
                let playing = if let Some(playback) = playback {
                    !host
                        .finished(playback)
                        .map_err(|cause| record_host_error(events, cause))?
                } else {
                    false
                };
                Ok(NativeReturn::Value(u32::from(playing)))
            }
            270 => {
                // The formal window event loop consumes this exact surface.
                // This activates real presentation and preserves the return slot.
                if !implicit
                    || match playback {
                        Some(playback) => !host
                            .finished(playback)
                            .map_err(|cause| record_host_error(events, cause))?,
                        None => false,
                    }
                {
                    host.show_on_window();
                }
                Ok(NativeReturn::Continue)
            }
            _ => Err(native_state_error("unsupported movie native control")),
        }
    }
}

fn legacy_movie_gain(requested: i32) -> i32 {
    let attenuation = match requested {
        ..=0 => -10_000,
        100.. => 0,
        _ => ((f64::from(requested) / 100.0).log10() * 3322.0) as i32,
    };
    (100.0 * 10_f64.powf(f64::from(attenuation) / 2000.0)).round() as i32
}

pub(super) fn record_host_error(
    events: &mut Vec<crate::LogicEvent>,
    cause: impl std::fmt::Display,
) -> cs2_script::ScriptError {
    let detail = format!("movie host failed: {cause}");
    if let Some(event) = events.last_mut() {
        event.detail.push_str(&format!(" {detail}"));
    } else {
        events.push(crate::LogicEvent {
            frame: 0,
            kind: "movie-host-error".into(),
            detail,
        });
    }
    native_state_error("movie host failed; see recorded native event diagnostic")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::HeadlessPlayer;
    use std::path::Path;

    #[test]
    fn legacy_volume_uses_logarithmic_attenuation() {
        for (requested, gain) in [
            (-1, 0),
            (0, 0),
            (25, 10),
            (50, 32),
            (60, 43),
            (100, 100),
            (150, 100),
        ] {
            assert_eq!(legacy_movie_gain(requested), gain, "volume {requested}");
        }
    }

    #[test]
    fn movie_option_417_distinguishes_legacy_volume_from_explicit_handle() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let save_root =
            std::env::temp_dir().join(format!("cs2-movie-options-{}", std::process::id()));
        let mut player = HeadlessPlayer::open_with_save_root(root, save_root).unwrap();
        player.movie_resources.insert(22, "fixture.mpg".to_owned());
        player.movie_host.set_volume(81);

        player
            .with_host(|host| {
                assert_eq!(
                    host.movie_options_native(417, &60_i32.to_le_bytes())?,
                    NativeReturn::Continue,
                );
                assert_eq!(host.movie_host.as_deref().unwrap().volume(), 81);

                let explicit: Vec<_> = [22_u32, 60].into_iter().flat_map(u32::to_le_bytes).collect();
                assert_eq!(host.movie_options_native(417, &explicit)?, NativeReturn::Continue);
                assert_eq!(host.movie_host.as_deref().unwrap().volume(), 60);
                assert!(host.movie_options_native(417, &[0; 12]).is_err());
                assert!(host.movie_options_native(997, &[0; 4]).is_err());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn movie_control_accepts_implicit_arguments_and_releases_the_current_movie() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        let save_root =
            std::env::temp_dir().join(format!("cs2-movie-control-{}", std::process::id()));
        let mut player = HeadlessPlayer::open_with_save_root(root, save_root).unwrap();
        player
            .with_host(|host| {
                for index in 266..=269 {
                    assert_eq!(host.control_movie_native(index, &[])?, NativeReturn::Value(0));
                    assert!(host.control_movie_native(index, &[0; 2]).is_err());
                }
                assert_eq!(host.control_movie_native(270, &[])?, NativeReturn::Continue);
                assert!(host.control_movie_native(266, &[0; 4]).is_err());
                assert_eq!(host.control_movie_native(266, &[0; 8])?, NativeReturn::Value(0));
                host.movie_resources.insert(22, "fixture.mpg".to_owned());
                assert_eq!(host.control_movie_native(269, &[])?, NativeReturn::Value(0));
                assert_eq!(host.control_movie_native(268, &[])?, NativeReturn::Value(1));
                assert!(!host.movie_resources.contains_key(&22));
                assert_eq!(host.control_movie_native(268, &[])?, NativeReturn::Value(0));
                assert_eq!(
                    host.control_movie_native(268, &22_u32.to_le_bytes())?,
                    NativeReturn::Value(1),
                );
                Ok(())
            })
            .unwrap();
    }
}
