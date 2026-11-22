//! Ordinary 434..438's explicit resource queue and cached bytes.
//! A portable synchronous worker completes a requested batch at 436. It keeps
//! real resource failures distinct from a completed, missing-file attempt.
use cs2_runtime::{ResourceCandidate, ResourceIndex, RuntimeError, resource_router::ResourceMode};
use cs2_script::{NativeReturn, Result, Vm};

use super::{cp932_to_utf8, expand_kcs_format, native_state_error, read_u32};

#[path = "image_predecode.rs"]
mod images;

const MAX_LIVE: usize = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;

#[derive(Debug, Clone)]
pub(super) struct SourcedResource {
    pub source: ResourceCandidate,
    pub bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Request {
    name: String,
    priority: i32,
    age: i32,
    attempted: bool,
    resource: Option<SourcedResource>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ResourcePreloads {
    enabled: Option<bool>,
    requests: Vec<Request>,
    completed_through: i32,
    bytes: usize,
    images: images::ImagePredecode,
}

impl ResourcePreloads {
    pub(crate) fn graph_request_count(&self) -> usize { self.requests.len() }
    pub(super) fn cached(&self, name: &str) -> Option<&[u8]> {
        self.cached_with_source(name).map(|resource| resource.bytes.as_slice())
    }

    pub(super) fn cached_with_source(&self, name: &str) -> Option<&SourcedResource> {
        self.requests.iter().find(|request| request.name.eq_ignore_ascii_case(name))
            .and_then(|request| request.resource.as_ref())
    }

    pub(super) fn predecode_image(&mut self, bytes: &[u8]) { self.images.submit(bytes); }

    pub(super) fn take_predecoded_image(&mut self, bytes: &[u8], id: u32) -> Option<cs2_renderer::RgbaFrame> {
        self.images.take(bytes, id)
    }

    pub(super) fn call(
        &mut self, index: u32, args: &[u8], vm: &Vm, resources: &ResourceIndex,
    ) -> Result<NativeReturn> {
        self.initialize(resources)?;
        match index {
            434 => {
                if self.enabled == Some(true) {
                    self.requests.clear();
                    self.images.clear();
                    self.bytes = 0;
                    self.completed_through = i32::MAX;
                }
                Ok(NativeReturn::Continue)
            }
            435 => {
                // The wrapper formats category before filename, even when
                // caching is disabled. Both are bounded stack buffers.
                let category = formatted(vm, read_u32(args, 4)?)?;
                let name = formatted(vm, read_u32(args, 0)?)?;
                let priority = read_u32(args, 8)? as i32;
                let name = String::from_utf8(cp932_to_utf8(&name))
                    .expect("CP932 conversion emits UTF-8");
                let candidates = resources.category_names(&category, &name)
                    .map_err(|_| native_state_error("native 435 category resolution failed"))?;
                // Registration completes the name without opening it. The
                // batch worker performs the exact per-slot read later.
                let resolved = candidates.first()
                    .ok_or_else(|| native_state_error("native 435 category resolved no names"))?;
                if resolved.encode_utf16().count() >= 260 {
                    return Err(native_state_error("native 435 resolved name exceeds 259 UTF-16 units"));
                }
                if self.enabled == Some(true) {
                    if let Some(existing) = self.requests.iter_mut()
                        .find(|request| request.name.eq_ignore_ascii_case(resolved))
                    {
                        // Re-registration refreshes priority only; the
                        // completed buffer/attempt flag and age remain.
                        existing.priority = priority;
                    } else {
                        if self.requests.len() >= MAX_ENTRIES {
                            return Err(native_state_error("native 435 resource queue exceeds one million entries"));
                        }
                        self.requests.push(Request {
                            name: resolved.clone(), priority, age: priority,
                            attempted: false, resource: None,
                        });
                    }
                }
                Ok(NativeReturn::Continue)
            }
            436 => {
                if self.enabled == Some(true) {
                    // The list sort uses +0x218's age, not refreshed
                    // +0x214 priority (0x52ac20..0x52ac2c).
                    self.requests.sort_by_key(|request| request.age);
                    self.completed_through = 0;
                    for request in &mut self.requests {
                        if request.attempted { continue; }
                        let resource = read_optional(resources, &request.name, MAX_LIVE - self.bytes)?;
                        let total = self.bytes + resource.as_ref().map_or(0, |resource| resource.bytes.capacity());
                        if let Some(resource) = &resource {
                            self.images.submit(&resource.bytes);
                        }
                        request.resource = resource;
                        request.attempted = true;
                        self.bytes = total;
                        self.completed_through = request.priority;
                    }
                    self.completed_through = i32::MAX;
                }
                Ok(NativeReturn::Continue)
            }
            437 => {
                let priority = read_u32(args, 0)? as i32;
                Ok(NativeReturn::Value(u32::from(
                    self.enabled != Some(true) || priority <= self.completed_through,
                )))
            }
            438 => {
                if self.enabled == Some(true) {
                    for request in &mut self.requests {
                        request.priority = request.priority.wrapping_sub(1);
                        if request.priority >= 0 { request.age = request.age.wrapping_sub(1); }
                    }
                    let mut reclaimed = 0;
                    self.requests.retain(|request| {
                        if request.priority < 0 {
                            reclaimed += request.resource.as_ref().map_or(0, |resource| resource.bytes.capacity());
                            false
                        } else { true }
                    });
                    self.bytes -= reclaimed;
                    self.completed_through = self.completed_through.wrapping_sub(1);
                }
                Ok(NativeReturn::Continue)
            }
            _ => Err(cs2_script::ScriptError::UnimplementedNativeCall { index }),
        }
    }

    fn initialize(&mut self, resources: &ResourceIndex) -> Result<()> {
        if self.enabled.is_some() { return Ok(()); }
        if let Some(config) = resources.startup_configuration()
            .map_err(|_| native_state_error("resource preload startup configuration failed"))? {
            self.enabled = Some(config.integer(&[b"document", b"ETC", b"cache"], 1) != 0);
            return Ok(());
        }
        self.enabled = Some(true);
        Ok(())
    }
}

fn formatted(vm: &Vm, address: u32) -> Result<Vec<u8>> {
    let expanded = expand_kcs_format(vm, &vm.read_string(address)?, 0)?;
    let bytes = &expanded[..expanded.iter().position(|&byte| byte == 0).unwrap_or(expanded.len())];
    if bytes.len() >= 260 {
        return Err(native_state_error("native 435 filename/category exceeds 259 bytes"));
    }
    Ok(bytes.to_vec())
}

fn read_optional(resources: &ResourceIndex, name: &str, remaining: usize) -> Result<Option<SourcedResource>> {
    let Some(candidate) = resources.resolve_mode_candidate(name, ResourceMode::LooseFirst)
        .map_err(|_| native_state_error("native 436 resource routing failed"))? else { return Ok(None); };
    match resources.read_candidate_bounded(candidate, remaining) {
        Ok(bytes) => Ok(bytes.map(|bytes| SourcedResource { source: candidate.clone(), bytes })),
        Err(RuntimeError::ResourceNotFound(_)) => Ok(None),
        Err(RuntimeError::Io { source, .. }) if matches!(source.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
        ) => Ok(None),
        Err(_) => Err(native_state_error("native 436 resource read failed")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resource_preload_budget_miss_leaves_demand_read_available() {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("cs2-preload-budget-{stamp}"));
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join("resource.bin"), b"x").unwrap();
        let resources = ResourceIndex::scan(&root).unwrap();
        std::fs::write(root.join("resource.bin"), b"grown").unwrap();

        assert!(read_optional(&resources, "resource.bin", 4).unwrap().is_none());
        assert!(read_optional(&resources, "missing.bin", 5).unwrap().is_none());
        assert_eq!(read_optional(&resources, "resource.bin", 5).unwrap().unwrap().bytes, b"grown");
        assert_eq!(resources.read("resource.bin").unwrap(), b"grown");
        std::fs::remove_dir_all(root).unwrap();
    }
}
