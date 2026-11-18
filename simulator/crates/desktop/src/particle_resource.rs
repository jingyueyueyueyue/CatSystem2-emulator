//! PPI filenames use the file-category helper, independently of the virtual
//! archive-name helper used by KCS and images. Happiness2 RVA12aaf0/12ab80
//! consumes INT.ext/dir;69319ebc4 calls it before320. The loader ultimately
//! calls LoadLibraryA (Yuki IAT8e226c), so the portable model reads that exact
//! plain file after decoding the completed script-encoded filename.
use cs2_script::Result;
use super::super::{LoggingHost, ScriptTextEncoding, native_state_error, trim_nul};

impl LoggingHost<'_> {
    pub(super) fn complete_particle_plugin_name(&self, source: &[u8]) -> Result<Option<Vec<u8>>> {
        let source = self.text_encoding.decode(trim_nul(source));
        // 12ab80's wide directory test precedes category lookup. Explicit
        // paths keep their own extension and do not acquire another directory.
        let path = if source.rfind(['/', '\\']).is_some_and(|at| at > 0) {
            source
        } else {
            let Some(config) = self.resources.startup_configuration()
                .map_err(|_| native_state_error("particle file category configuration is unreadable"))?
            else { return Ok(None); };
            let mut selected = None;
            for slot in 0..128 {
                let key = format!("int{slot:02}");
                let name = config.text(&[b"document", b"INT", key.as_bytes(), b"name"])
                    .unwrap_or_default();
                if trim_nul(name).eq_ignore_ascii_case(b"ptclpi") { selected = Some(key); break; }
            }
            let Some(key) = selected else { return Ok(None); };
            let field = |leaf: &[u8], capacity: usize| -> Result<&str> {
                let bytes = trim_nul(config.text(&[b"document", b"INT", key.as_bytes(), leaf])
                    .unwrap_or_default());
                let text = std::str::from_utf8(bytes)
                    .map_err(|_| native_state_error("particle runtime INT field is not UTF-8"))?;
                if text.encode_utf16().count() >= capacity {
                    return Err(native_state_error("particle file category field exceeds original capacity"));
                }
                Ok(text)
            };
            let extension = field(b"ext", 8)?;
            let directory = field(b"dir", 260)?;
            let file = if source.rsplit(['/', '\\']).next().unwrap_or_default().contains('.') {
                source
            } else { format!("{source}.{extension}") };
            if directory.is_empty() { file } else { format!("{directory}\\{file}") }
        };
        let bytes = match self.text_encoding {
            ScriptTextEncoding::Utf8 => path.into_bytes(),
            encoding => {
                let codec = match encoding {
                    ScriptTextEncoding::Cp932 => encoding_rs::SHIFT_JIS,
                    ScriptTextEncoding::Gbk => encoding_rs::GBK,
                    ScriptTextEncoding::Utf8 => unreachable!(),
                };
                let (bytes, _, invalid) = codec.encode(&path);
                if invalid { return Err(native_state_error("particle plugin filename is not representable in script encoding")); }
                bytes.into_owned()
            }
        };
        Ok(Some(bytes))
    }
}
