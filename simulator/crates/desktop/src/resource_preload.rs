//! Ordinary 434..438's explicit resource queue and cached bytes.
//! A portable synchronous worker completes a requested batch at 436. It keeps
//! real resource failures distinct from a completed, missing-file attempt.
use cs2_runtime::{ResourceIndex, RuntimeError};
use cs2_script::{NativeReturn, Result, Vm};

use super::{cp932_to_utf8, expand_kcs_format, native_state_error, read_u32};

const MAX_FILE: usize = 64 * 1024 * 1024;
const MAX_LIVE: usize = 256 * 1024 * 1024;
const MAX_ENTRIES: usize = 1_000_000;

#[derive(Debug, Clone)]
struct Request {
    name: String,
    priority: i32,
    age: i32,
    attempted: bool,
    bytes: Option<Vec<u8>>,
}

#[derive(Debug, Clone, Default)]
pub(super) struct ResourcePreloads {
    enabled: Option<bool>,
    requests: Vec<Request>,
    completed_through: i32,
    bytes: usize,
}

impl ResourcePreloads {
    pub(crate) fn graph_request_count(&self) -> usize { self.requests.len() }
    pub(super) fn cached(&self, name: &str) -> Option<&[u8]> {
        self.requests.iter().find(|request| request.name.eq_ignore_ascii_case(name))
            .and_then(|request| request.bytes.as_deref())
    }

    pub(super) fn call(
        &mut self, index: u32, args: &[u8], vm: &Vm, resources: &ResourceIndex,
    ) -> Result<NativeReturn> {
        self.initialize(resources)?;
        match index {
            434 => {
                if self.enabled == Some(true) {
                    self.requests.clear();
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
                            attempted: false, bytes: None,
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
                        let bytes = read_optional(resources, &request.name)?;
                        let total = self.bytes.checked_add(bytes.as_ref().map_or(0, Vec::len))
                            .filter(|&size| size <= MAX_LIVE)
                            .ok_or_else(|| native_state_error("native 436 cache exceeds 256 MiB"))?;
                        request.bytes = bytes;
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
                            reclaimed += request.bytes.as_ref().map_or(0, Vec::len);
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

fn read_optional(resources: &ResourceIndex, name: &str) -> Result<Option<Vec<u8>>> {
    let Some(candidate) = resources.resolve_mode_utf16(
        &name.encode_utf16().collect::<Vec<_>>(), cs2_runtime::resource_router::ResourceMode::LooseFirst,
    )
        .map_err(|_| native_state_error("native 436 resource routing failed"))? else { return Ok(None); };
    let candidate = candidate.value;
    if candidate.size > MAX_FILE as u64 {
        return Err(native_state_error("native 436 resource exceeds 64 MiB"));
    }
    match resources.read_candidate(candidate) {
        Ok(bytes) if bytes.len() <= MAX_FILE => Ok(Some(bytes)),
        Ok(_) => Err(native_state_error("native 436 resource exceeds 64 MiB")),
        Err(RuntimeError::ResourceNotFound(_)) => Ok(None),
        Err(RuntimeError::Io { source, .. }) if matches!(source.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
        ) => Ok(None),
        Err(_) => Err(native_state_error("native 436 resource read failed")),
    }
}
