//! CSTL and physical input assertions for the ordinary scene 01 continuation.
use super::{Anchor, physical_mouse};
use cs2_desktop::{HeadlessPlayer, LogicEvent};
use cs2_runtime::message_object::InputRouteReceipt;
use serde::Deserialize;
use serde_json::json;
use std::collections::BTreeMap;

const RESUME: &str = "message=[2, 625, 478, 0]";

pub(super) const JOIN_GROUP: i32 = 35;

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Page {
    pub(super) group: i32,
    pub(super) wait: i32,
    text: Vec<Fragment>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Fragment {
    ordinal: i32,
    record: i32,
    body: bool,
    bytes: usize,
    sha256: String,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Chapter {
    pub(super) scene: String,
    pub(super) pages: Vec<Page>,
}

struct ChapterResource {
    archive: &'static str,
    cst_sha: &'static str,
    cstl_sha: &'static str,
    waits: Option<(i32, usize, i32)>,
}

fn chapter_resource(name: &str) -> Option<ChapterResource> {
    include_str!("../../src/neko_chapter_resources.txt").lines().find_map(|line| {
        let mut fields = line.split_ascii_whitespace();
        let archive = fields.next()?;
        if fields.next()?.strip_suffix(".cst")? != name {
            return None;
        }
        let cst_sha = fields.next()?;
        let cstl_sha = fields.next()?;
        let waits = if cstl_sha == "-" { None } else {
            Some((fields.next()?.parse().ok()?, fields.next()?.parse().ok()?, fields.next()?.parse().ok()?))
        };
        Some(ChapterResource { archive, cst_sha, cstl_sha, waits })
    })
}

pub(super) const FIRST_CHAPTERS: &[&str] = &["h01", "01_02", "02", "03", "04", "05", "06", "07"];

pub(super) fn chapter_path(from: &str, to: &str) -> Option<&'static [&'static str]> {
    Some(match (from, to) {
        ("01", "h01") => &["01_01end_1", "h01"],
        ("01", "01_02") => &["01_01end_2", "01_01_2", "01_02"],
        ("h01", "01_02") => &["01_01", "01_02"],
        ("01_02", "02") => &["02"],
        ("02", "03") => &["03"],
        ("03", "04") => &["04"],
        ("04", "05") => &["05"],
        ("05", "06") => &["06"],
        ("06", "07") => &["07"],
        _ => return None,
    })
}

pub(super) fn chapter_load_matches(event: &LogicEvent, name: &str, earliest: u64) -> bool {
    event.frame >= earliest && event.detail.split_once(" member=").is_some_and(|(_, actual)| {
        chapter_resource(name).is_some_and(|resource| actual == format!("{name}.cst sha256={}", resource.cst_sha))
    })
}

impl Chapter {
    pub(super) fn valid(&self) -> bool {
        let Some((first, last_group, last_wait)) = chapter_resource(&self.scene).and_then(|resource| resource.waits) else {
            return false;
        };
        (2..=last_group + 1).contains(&self.pages.len())
            && self.pages[0].wait == first && valid_sequence(&self.pages, 0, -1, last_wait)
    }

    pub(super) fn complete(&self) -> bool {
        chapter_resource(&self.scene).and_then(|resource| resource.waits)
            .is_some_and(|(_, last_group, last_wait)| self.pages.len() == last_group + 1
                && self.pages.last().is_some_and(|page| page.wait == last_wait))
    }
}

pub(super) fn valid_pages(pages: &[Page], terminal: Option<i32>) -> bool {
    if !(2..=457).contains(&pages.len())
        || (pages[0].group, pages[0].wait) != (JOIN_GROUP, 485)
        || pages.last().map(|page| page.wait) != terminal
    {
        return false;
    }
    valid_sequence(pages, JOIN_GROUP, 483, 5386)
}

fn valid_sequence(pages: &[Page], first_group: i32, mut previous: i32, last_wait: i32) -> bool {
    for (index, page) in pages.iter().enumerate() {
        let Some(first) = previous.checked_add(1) else {
            return false;
        };
        if page.group != first_group + index as i32
            || !(first..=last_wait).contains(&page.wait)
            || !page.text.iter().any(|fragment| fragment.body)
        {
            return false;
        }
        let mut ordinal = -1;
        for fragment in &page.text {
            if fragment.ordinal <= ordinal
                || first.checked_add(fragment.ordinal) != Some(fragment.record)
                || fragment.record >= page.wait
                || fragment.sha256.len() != 64
                || !fragment
                    .sha256
                    .bytes()
                    .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
            {
                return false;
            }
            ordinal = fragment.ordinal;
        }
        previous = page.wait;
    }
    true
}

#[derive(Default)]
pub(super) struct Reads {
    cst: BTreeMap<(u32, u64), LogicEvent>,
    cstl: BTreeMap<(u32, u64), LogicEvent>,
    text: BTreeMap<(u32, i32), Vec<LogicEvent>>,
}

impl Reads {
    pub(super) fn observe(&mut self, events: &[LogicEvent]) {
        for event in events {
            let Some((handle, detail)) = event
                .detail
                .strip_prefix("handle=")
                .and_then(|fields| fields.split_once(' '))
                .and_then(|(handle, detail)| Some((handle.parse::<u32>().ok()?, detail)))
            else {
                continue;
            };
            match event.kind.as_str() {
                "neko-cst-origin" => {
                    self.cst.insert((handle, event.frame), event.clone());
                }
                "neko-cstl-load" => {
                    self.cstl.insert((handle, event.frame), event.clone());
                }
                "neko-cstl-read" if detail.contains(" kind=0x20 destination=0x00062c6b ") => {
                    if let Some(group) = detail
                        .strip_prefix("input=")
                        .and_then(|fields| fields.split_once(' '))
                        .and_then(|(group, _)| group.parse::<i32>().ok())
                        .filter(|group| (0..=576).contains(group))
                    {
                        self.text
                            .entry((handle, group))
                            .or_default()
                            .push(event.clone());
                    }
                }
                _ => {}
            }
        }
    }

    pub(super) fn verify(
        &mut self,
        frame: u64,
        scene: &str,
        page: &Page,
        language: &str,
        anchor: Anchor,
        (loaded, marked, complete, sources): (u64, u64, u64, Vec<usize>),
        earliest: u64,
    ) -> Result<(), String> {
        let (context, load, owner, epoch, source_frame) = anchor;
        let Some(ChapterResource { archive, cst_sha, cstl_sha, .. }) = chapter_resource(scene) else {
            return Err(format!("unknown NEKO chapter {scene}"));
        };
        let expected_cst = format!("handle={context} load_id={load} source=\"{archive}::{scene}.cst\" sha256={cst_sha}");
        let expected_cstl = format!("\"{archive}::{scene}.cstl\" sha256={cstl_sha} columns=[\"cn\", \"en\", \"jp\"]");
        let cst = self.cst.get(&(context, loaded));
        let cstl = self.cstl.get(&(context, loaded));
        if !cst.is_some_and(|event| event.detail == expected_cst)
            || !cstl.is_some_and(|event| {
                event.detail.split_once(" source=")
                    .is_some_and(|(_, actual)| actual.eq_ignore_ascii_case(&expected_cstl))
            })
        {
            return Err(format!(
                "CST/CSTL load mismatch: f{loaded} handle={context} load={load} cst={cst:?} cstl={cstl:?}"
            ));
        }
        if !(loaded <= earliest
            && earliest <= source_frame
            && source_frame <= complete
            && complete <= frame
            && source_frame <= marked
            && marked < frame)
            || !sources.iter().copied().eq(page
                .text
                .iter()
                .filter(|fragment| fragment.body)
                .map(|fragment| fragment.record as usize))
        {
            return Err(format!(
                "CSTL body timing or contribution mismatch: earliest={earliest} \
                source={source_frame} complete={complete} marker={marked} sources={sources:?} expected={:?}",
                page.text
            ));
        }
        let column = if language == "jp" { 2 } else { 0 };
        let reads = self.text.remove(&(context, page.group)).unwrap_or_default();
        let group_reads: Vec<_> = reads
            .iter()
            .filter(|event| (earliest..=marked).contains(&event.frame))
            .collect();
        let prefix = format!("handle={context} input={} ordinal=", page.group);
        for event in &group_reads {
            let ordinal = event
                .detail
                .strip_prefix(&prefix)
                .and_then(|detail| detail.split_once(' '))
                .and_then(|(ordinal, _)| ordinal.parse::<i32>().ok());
            if !page
                .text
                .iter()
                .any(|fragment| Some(fragment.ordinal) == ordinal)
            {
                return Err(format!("unexpected primary read: {event:?}"));
            }
        }
        for fragment in &page.text {
            let start = format!("{prefix}{} ", fragment.ordinal);
            let expected = format!(
                "Some(\"{language}\") requested=Some(\"{language}\") source=cstl \
                column=Some({column}) bytes={} sha256={}",
                fragment.bytes, fragment.sha256
            );
            let mut matches = group_reads
                .iter()
                .filter(|event| event.detail.starts_with(&start))
                .peekable();
            if matches.peek().is_none()
                || !matches
                    .clone()
                    .any(|event| !fragment.body || event.frame <= source_frame)
                || !matches.all(|event| {
                    event
                        .detail
                        .split_once(" main=")
                        .is_some_and(|(_, actual)| actual == expected)
                })
            {
                return Err(format!(
                    "CSTL r{} ordinal={} expected {expected}; primary reads={group_reads:?}",
                    fragment.record, fragment.ordinal
                ));
            }
        }
        let mut proof = json!({
                "group": page.group, "record": page.wait, "context": context, "load_id": load,
                "owner": owner, "epoch": epoch, "source_frame": source_frame, "sources": sources,
                "loaded": loaded, "marked": marked, "complete": complete, "earliest": earliest,
            });
        if scene != "01" {
            proof["scene"] = json!(scene);
        }
        println!("frame={frame} neko-cstl-page {proof}");
        Ok(())
    }
}

#[derive(Default)]
pub(super) struct Opening {
    start: Option<LogicEvent>,
    end: Option<LogicEvent>,
    sample: Option<(u32, u64, f64)>,
    advanced: bool,
    ambiguous: bool,
}

impl Opening {
    pub(super) fn observe_events(&mut self, events: &[LogicEvent], started: u64) {
        for event in events.iter().filter(|event| event.frame > started) {
            if !event.detail.starts_with("op handle=") {
                continue;
            }
            let target = match event.kind.as_str() {
                "neko-film-start" => &mut self.start,
                "neko-film-end" => &mut self.end,
                _ => continue,
            };
            self.ambiguous |= target.as_ref().is_some_and(|previous| {
                previous.frame != event.frame || previous.detail != event.detail
            });
            *target = Some(event.clone());
        }
    }

    pub(super) fn observe_composition(&mut self, player: &HeadlessPlayer) {
        if let Some(("op", handle, playback, pts)) = player.host_neko_composited_film() {
            if let Some((old_handle, old_playback, old_pts)) = self.sample {
                self.ambiguous |= (old_handle, old_playback) != (handle, playback) || pts < old_pts;
                self.advanced |= pts > old_pts;
            }
            self.sample = Some((handle, playback, pts));
        }
    }

    pub(super) fn completed(&self, press: u64, successor_wait: u64) -> bool {
        let (Some(start), Some(end), Some((handle, playback, _))) =
            (&self.start, &self.end, self.sample)
        else { return false; };
        self.advanced && !self.ambiguous && press <= start.frame && start.frame < end.frame
            && end.frame < successor_wait
            && start.detail == format!("op handle={handle} playback={playback} count=1")
            && end.detail == format!("op handle={handle} playback={playback} natural=true")
    }
}

pub(super) struct RootPress {
    region: u32,
    owner: u32,
    receipt: Option<InputRouteReceipt>,
    error: Option<String>,
}

#[derive(Default)]
pub(super) struct Eyecatch {
    enter: Option<u64>,
    voice: Option<u64>,
    resume: Option<u64>,
}

impl Eyecatch {
    pub(super) fn observe(&mut self, events: &[LogicEvent], after: u64) {
        for event in events.iter().filter(|event| event.frame > after) {
            if event.kind == "native-call" && event.detail.starts_with("index=1040 ") {
                if event.detail.ends_with(" debug=\"[command] fesjs eyecatch 2\"") {
                    self.enter.get_or_insert(event.frame);
                } else if self.voice.is_some() && event.detail.contains(" debug=\"[command] scenetitle ") {
                    self.resume.get_or_insert(event.frame);
                }
            } else if event.kind == "fes-media"
                && event.detail.contains(" sound_load channel=15 ")
                && event.detail.contains("resource=\"pcm_b.int/B_eyecatch.ogg\"")
            {
                self.voice.get_or_insert(event.frame);
            }
        }
    }

    pub(super) fn completed(&self, press: u64, successor: u64) -> bool {
        matches!((self.enter, self.voice, self.resume), (Some(enter), Some(voice), Some(resume))
            if press <= enter && enter <= voice && voice < resume && resume < successor)
    }
}

impl RootPress {
    pub(super) fn live(player: &HeadlessPlayer, owner: u32, region: u32) -> Option<Self> {
        player
            .host_neko_body_root_owner(owner, region)
            .map(|owner| Self {
                region,
                owner,
                receipt: None,
                error: None,
            })
    }

    pub(super) fn observe(&mut self, player: &HeadlessPlayer, started: u64) {
        let expected = (2, self.region, self.owner, 0);
        for receipt in player.host_input_route_receipts().iter().filter(|receipt| {
            physical_mouse(receipt, started)
                && (receipt.response == 0 || receipt.key == 2 && receipt.receiver == expected.2)
        }) {
            if (
                receipt.key,
                receipt.region,
                receipt.receiver,
                receipt.response,
            ) != expected
                || self.receipt.is_some()
            {
                self.error = Some(format!(
                    "expected one root key-2 consumer {expected:?}; got {receipt:?}"
                ));
                continue;
            }
            let resumes: Vec<_> = player
                .events()
                .iter()
                .rev()
                .take_while(|event| event.frame >= receipt.frame)
                .filter(|event| event.frame == receipt.frame && event.kind == "message-resume")
                .collect();
            if !matches!(resumes.as_slice(), [resume] if resume.detail == RESUME) {
                self.error = Some(format!(
                    "root consumer without unique VM resume: {receipt:?}; resumes={resumes:?}"
                ));
            }
            self.receipt = Some(receipt.clone());
        }
    }

    pub(super) fn check(&self, frame: u64) -> Result<(), String> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if !self
            .receipt
            .as_ref()
            .is_some_and(|receipt| receipt.frame == frame)
        {
            return Err(format!(
                "missing same-frame root region {} owner {} consumption; got {:?}",
                self.region, self.owner, self.receipt
            ));
        }
        Ok(())
    }

    // Called only after the controller has verified the device scan release.
    pub(super) fn log(&self, frame: u64, started: u64, low: &InputRouteReceipt) {
        println!(
            "frame={frame} neko-cstl-consumed {}",
            json!({
                "started": started, "press_frame": low.frame, "region": low.region, "owner": low.receiver,
                "root_region": self.region, "root_owner": self.owner, "released": true,
            })
        );
    }
}
