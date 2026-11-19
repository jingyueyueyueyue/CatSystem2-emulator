use std::{
    collections::{HashMap, VecDeque},
    sync::Arc,
};

use super::{
    GlyphMask, StyledTextGlyph, StyledTextRun, TextEncoding, TextLayoutSettings, TextToken,
};

const GLYPH_BYTES: usize = 8 * 1024 * 1024;
const GLYPH_ENTRIES: usize = 2048;
const LAYOUT_BYTES: usize = 2 * 1024 * 1024;
const LAYOUT_ENTRIES: usize = 16;

type GlyphKey = (char, u32, bool, bool);

#[derive(Debug, Default)]
pub(super) struct GlyphCache {
    entries: HashMap<GlyphKey, (u64, Arc<GlyphMask>)>,
    generation: u64,
    bytes: usize,
}

impl GlyphCache {
    pub(super) fn get(&mut self, key: GlyphKey) -> Option<&Arc<GlyphMask>> {
        let (generation, glyph) = self.entries.get_mut(&key)?;
        self.generation += 1;
        *generation = self.generation;
        Some(glyph)
    }

    pub(super) fn insert(&mut self, key: GlyphKey, glyph: Arc<GlyphMask>) -> Arc<GlyphMask> {
        self.insert_bounded(key, glyph, GLYPH_BYTES, GLYPH_ENTRIES)
    }

    fn insert_bounded(
        &mut self,
        key: GlyphKey,
        glyph: Arc<GlyphMask>,
        max_bytes: usize,
        max_entries: usize,
    ) -> Arc<GlyphMask> {
        if let Some(existing) = self.get(key) {
            return Arc::clone(existing);
        }
        let bytes = glyph_bytes(&glyph);
        if bytes > max_bytes {
            return glyph;
        }
        while self.entries.len() >= max_entries || self.bytes + bytes > max_bytes {
            let (&evicted, _) = self
                .entries
                .iter()
                .min_by_key(|(_, (generation, _))| generation)
                .unwrap();
            let (_, removed) = self.entries.remove(&evicted).unwrap();
            self.bytes -= glyph_bytes(&removed);
        }
        self.generation += 1;
        self.entries
            .insert(key, (self.generation, Arc::clone(&glyph)));
        self.bytes += bytes;
        glyph
    }
}

fn glyph_bytes(mask: &GlyphMask) -> usize {
    size_of::<GlyphMask>() + mask.alpha.capacity()
}

#[derive(Debug, Default)]
pub(super) struct LayoutCache {
    entries: VecDeque<LayoutEntry>,
    bytes: usize,
}

#[derive(Debug)]
struct LayoutEntry {
    tokens: Vec<TextToken>,
    encoding: TextEncoding,
    settings: TextLayoutSettings,
    run: Arc<StyledTextRun>,
    bytes: usize,
}

impl LayoutCache {
    pub(super) fn get(
        &mut self,
        tokens: &[TextToken],
        encoding: TextEncoding,
        settings: &TextLayoutSettings,
    ) -> Option<Arc<StyledTextRun>> {
        let index = self.entries.iter().position(|entry| {
            entry.encoding == encoding && entry.settings == *settings && entry.tokens == tokens
        })?;
        let entry = self.entries.remove(index).unwrap();
        let run = Arc::clone(&entry.run);
        self.entries.push_back(entry);
        Some(run)
    }

    pub(super) fn insert(
        &mut self,
        tokens: &[TextToken],
        encoding: TextEncoding,
        settings: &TextLayoutSettings,
        run: Arc<StyledTextRun>,
    ) -> Arc<StyledTextRun> {
        if let Some(existing) = self.get(tokens, encoding, settings) {
            return existing;
        }
        let bytes = size_of::<LayoutEntry>()
            .saturating_add(size_of_val(tokens))
            .saturating_add(size_of_val(settings.break_before.as_slice()))
            .saturating_add(size_of_val(settings.break_after.as_slice()))
            .saturating_add(
                run.glyphs
                    .capacity()
                    .saturating_mul(size_of::<StyledTextGlyph>()),
            );
        let bytes = run.glyphs().iter().fold(bytes, |total, glyph| {
            total.saturating_add(glyph.glyph.alpha.capacity())
        });
        if bytes > LAYOUT_BYTES {
            return run;
        }
        while self.entries.len() >= LAYOUT_ENTRIES || self.bytes + bytes > LAYOUT_BYTES {
            self.bytes -= self.entries.pop_front().unwrap().bytes;
        }
        self.entries.push_back(LayoutEntry {
            tokens: tokens.to_vec(),
            encoding,
            settings: settings.clone(),
            run: Arc::clone(&run),
            bytes,
        });
        self.bytes += bytes;
        run
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_eviction_preserves_recent_glyphs_and_live_leases() {
        let mut cache = GlyphCache::default();
        let first = ('A', 10, false, false);
        let second = ('B', 10, false, false);
        let third = ('C', 10, false, false);
        let small = Arc::new(GlyphMask::from_alpha(2, 2, vec![73; 4]).unwrap());
        let large = Arc::new(GlyphMask::from_alpha(2, 4, vec![120; 8]).unwrap());
        let cap = glyph_bytes(&small) + glyph_bytes(&large);
        cache.insert_bounded(first, Arc::clone(&small), cap, 3);
        let leased = cache.insert_bounded(second, Arc::clone(&small), cap, 3);
        assert!(Arc::ptr_eq(cache.get(first).unwrap(), &small));
        cache.insert_bounded(third, Arc::clone(&large), cap, 3);
        assert!(cache.get(second).is_none());
        assert!(Arc::ptr_eq(cache.get(first).unwrap(), &small));
        assert!(Arc::ptr_eq(cache.get(third).unwrap(), &large));
        assert_eq!(leased.alpha(), &[73; 4]);
        assert_eq!(cache.bytes, cap);
    }
}
