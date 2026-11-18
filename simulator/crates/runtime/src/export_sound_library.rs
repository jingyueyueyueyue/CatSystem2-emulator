//! Portable application sound schemes. This intentionally affects only the
//! application's private library, never an Android system-global sound theme.
use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::{RuntimeError, RuntimeResult};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_SCHEMES: usize = 256;
const MAX_EVENTS: usize = 4096;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub enum ApplicationSound {
    Mute,
    File(PathBuf),
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct ApplicationSoundScheme {
    pub events: BTreeMap<String, ApplicationSound>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ApplicationSoundLibrary {
    version: u32,
    selected: Option<String>,
    schemes: BTreeMap<String, ApplicationSoundScheme>,
}

impl Default for ApplicationSoundLibrary {
    fn default() -> Self {
        Self {
            version: 1,
            selected: None,
            schemes: BTreeMap::new(),
        }
    }
}

impl ApplicationSoundLibrary {
    pub fn decode(bytes: &[u8]) -> RuntimeResult<Self> {
        if bytes.len() > MAX_BYTES {
            return Err(invalid("application sound library exceeds 4 MiB"));
        }
        let library: Self = serde_json::from_slice(bytes)
            .map_err(|cause| RuntimeError::Host(format!("application sound library: {cause}")))?;
        library.validate()?;
        Ok(library)
    }

    pub fn encode(&self) -> RuntimeResult<Vec<u8>> {
        self.validate()?;
        let bytes = serde_json::to_vec(self)
            .map_err(|cause| RuntimeError::Host(format!("application sound library: {cause}")))?;
        if bytes.len() > MAX_BYTES {
            return Err(invalid("application sound library exceeds 4 MiB"));
        }
        Ok(bytes)
    }

    /// None inherits the selected scheme's event. Some(Mute) explicitly
    /// suppresses it; File retains the actual private exported sound path.
    pub fn register(
        &mut self,
        theme: String,
        events: impl IntoIterator<Item = (String, Option<ApplicationSound>)>,
    ) -> RuntimeResult<()> {
        valid_name(&theme)?;
        if self.schemes.len() >= MAX_SCHEMES && !self.schemes.contains_key(&theme) {
            return Err(invalid("application sound library exceeds 256 schemes"));
        }
        let mut scheme = self
            .selected
            .as_ref()
            .and_then(|theme| self.schemes.get(theme))
            .cloned()
            .unwrap_or_default();
        for (name, sound) in events {
            valid_name(&name)?;
            if let Some(sound) = sound {
                if let ApplicationSound::File(path) = &sound {
                    if !path.is_absolute() {
                        return Err(invalid("application sound path is not absolute"));
                    }
                }
                scheme.events.insert(name, sound);
            }
            if scheme.events.len() > MAX_EVENTS {
                return Err(invalid("application sound scheme exceeds 4096 events"));
            }
        }
        self.schemes.insert(theme, scheme);
        Ok(())
    }

    pub fn select(&mut self, theme: &str) -> RuntimeResult<()> {
        if !self.schemes.contains_key(theme) {
            return Err(invalid("application sound scheme is not registered"));
        }
        self.selected = Some(theme.to_owned());
        Ok(())
    }
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn themes(&self) -> impl Iterator<Item = &str> {
        self.schemes.keys().map(String::as_str)
    }
    pub fn event(&self, name: &str) -> Option<&ApplicationSound> {
        self.schemes.get(self.selected.as_ref()?)?.events.get(name)
    }

    fn validate(&self) -> RuntimeResult<()> {
        if self.version != 1 || self.schemes.len() > MAX_SCHEMES {
            return Err(invalid(
                "application sound library version or size is unsupported",
            ));
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|theme| !self.schemes.contains_key(theme))
        {
            return Err(invalid(
                "application sound library selects an absent scheme",
            ));
        }
        for (name, scheme) in &self.schemes {
            valid_name(name)?;
            if scheme.events.len() > MAX_EVENTS {
                return Err(invalid("application sound scheme exceeds 4096 events"));
            }
            for (name, sound) in &scheme.events {
                valid_name(name)?;
                if let ApplicationSound::File(path) = sound {
                    if !path.is_absolute() {
                        return Err(invalid("application sound path is not absolute"));
                    }
                }
            }
        }
        Ok(())
    }
}

fn valid_name(name: &str) -> RuntimeResult<()> {
    if name.is_empty() || name.len() > 4095 || name.contains('\0') {
        return Err(invalid(
            "application sound name is empty or exceeds its storage",
        ));
    }
    Ok(())
}
fn invalid(message: &str) -> RuntimeError {
    RuntimeError::Host(message.into())
}
