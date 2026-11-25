//! Static PPI SDK metadata and callback lookup.

use crate::pe_resources::{Section, parse_sections, x86_data_directory, x86_image_layout};

pub struct ParticlePluginPe<'a> {
    data: &'a [u8],
    sections: Vec<Section>,
    image_base: u32,
    version: u32,
    make_instance: u32,
    release_instance: u32,
}

impl<'a> ParticlePluginPe<'a> {
    pub fn read(data: &'a [u8]) -> Option<Self> {
        let (image_base, _) = x86_image_layout(data)?;
        let mut pe = Self {
            data,
            sections: parse_sections(data).ok()?,
            image_base,
            version: 0,
            make_instance: 0,
            release_instance: 0,
        };
        let [kind, version, make_instance, release_instance] = pe.exports()?;
        if pe.immediate_return(kind)? != 1 {
            return None;
        }
        pe.version = pe.immediate_return(version)?;
        pe.make_instance = make_instance;
        pe.release_instance = release_instance;
        Some(pe)
    }

    pub fn version(&self) -> u32 {
        self.version
    }

    pub fn constructor(&self) -> Option<&'a [u8]> {
        self.code(self.make_instance)
    }

    pub fn release(&self) -> Option<&'a [u8]> {
        self.code(self.release_instance)
    }

    pub fn constructor_call(&self, offset: usize) -> Option<&'a [u8]> {
        self.direct_call(self.make_instance, offset)
    }

    pub fn start_call(&self, table_address: u32, offset: usize) -> Option<&'a [u8]> {
        let table = self.read_only_data(table_address)?;
        self.direct_call(word(table, 4)?.checked_sub(self.image_base)?, offset)
    }

    pub fn update_call(&self, table_address: u32, offset: usize) -> Option<&'a [u8]> {
        let table = self.read_only_data(table_address)?;
        self.direct_call(word(table, 8)?.checked_sub(self.image_base)?, offset)
    }

    fn direct_call(&self, caller: u32, offset: usize) -> Option<&'a [u8]> {
        let body = self.code(caller)?;
        if *body.get(offset)? != 0xe8 {
            return None;
        }
        let next = offset.checked_add(5)?;
        let address = caller.checked_add(u32::try_from(next).ok()?)?;
        self.code(address.checked_add_signed(word(body, offset + 1)? as i32)?)
    }

    pub fn read_only_data(&self, address: u32) -> Option<&'a [u8]> {
        self.lookup(address.checked_sub(self.image_base)?, false)
    }

    pub fn code_at_address(&self, address: u32) -> Option<&'a [u8]> {
        self.code(address.checked_sub(self.image_base)?)
    }

    pub fn callbacks(&self, table_address: u32) -> Option<[&'a [u8]; 4]> {
        let table = self.lookup(table_address.checked_sub(self.image_base)?, false)?;
        Some([
            self.code(word(table, 0)?.checked_sub(self.image_base)?)?,
            self.code(word(table, 4)?.checked_sub(self.image_base)?)?,
            self.code(word(table, 8)?.checked_sub(self.image_base)?)?,
            self.code(word(table, 12)?.checked_sub(self.image_base)?)?,
        ])
    }

    fn exports(&self) -> Option<[u32; 4]> {
        let (directory_rva, size) = x86_data_directory(self.data, 0)?;
        let end = directory_rva.checked_add(size)?;
        let directory = self.lookup(directory_rva, false)?.get(..40)?;
        let function_count = usize::try_from(word(directory, 20)?).ok()?;
        let name_count = usize::try_from(word(directory, 24)?).ok()?;
        let functions = self.lookup(word(directory, 28)?, false)?
            .get(..function_count.checked_mul(4)?)?;
        let names = self.lookup(word(directory, 32)?, false)?
            .get(..name_count.checked_mul(4)?)?;
        let ordinals = self.lookup(word(directory, 36)?, false)?
            .get(..name_count.checked_mul(2)?)?;
        let mut target = [None; 4];
        for (name, ordinal) in names.chunks_exact(4).zip(ordinals.chunks_exact(2)) {
            let symbol = self.lookup(word(name, 0)?, false)?;
            let Some(index) = [b"GetPluginType\0".as_slice(), b"GetPluginVersion\0",
                b"MakeInstance\0", b"ReleaseInstance\0"].into_iter()
                .position(|name| symbol.starts_with(name)) else { continue };
            let ordinal = usize::from(u16::from_le_bytes(ordinal.try_into().ok()?));
            let rva = word(functions, ordinal * 4)?;
            if (directory_rva..end).contains(&rva)
                || self.code(rva).is_none()
                || target[index].replace(rva).is_some()
            {
                return None;
            }
        }
        let [Some(kind), Some(version), Some(make_instance), Some(release_instance)] = target else {
            return None;
        };
        Some([kind, version, make_instance, release_instance])
    }

    fn immediate_return(&self, rva: u32) -> Option<u32> {
        let code = self.code(rva)?;
        let [0xb8, .., 0xc3] = *code.get(..6)? else {
            return None;
        };
        word(code, 1)
    }

    fn code(&self, rva: u32) -> Option<&'a [u8]> {
        self.lookup(rva, true)
    }

    fn lookup(&self, rva: u32, executable: bool) -> Option<&'a [u8]> {
        let mut ranges = self.sections.iter().filter_map(|&section| {
            Some((section, section.file_bytes_from(self.data, rva)?))
        });
        let (section, bytes) = ranges.next()?;
        (ranges.next().is_none() && if executable { section.executable() }
            else { section.is_readable_data() }).then_some(bytes)
    }
}

fn word(bytes: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(offset..offset + 4)?.try_into().ok()?))
}
