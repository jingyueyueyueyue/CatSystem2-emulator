use super::root_clock::{primary_command_receiver, read_word};
use crate::pe_resources::Section;

pub(super) fn effekseer_family<'a>(
    slots: &[(u32, &'a [u8])],
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    code: impl Fn(u32) -> Option<&'a [u8]>,
) -> bool {
    let Some(slots) = slots.get(1014..=1016) else { return false; };
    let wrappers: [&[u8]; 3] = [
        b"\x68\x42\x02\0\0\xff\x36\x8b\xcf\xff\x77\x04\xff\x50\x10",
        b"\x68\x43\x02\0\0\x52\xff\x71\x04\xff\x50\x10",
        b"\x6a\0\xff\x72\x04\x6a\0\x68\x44\x02\0\0\xff\x32\xff\x71\x04\xff\x50\x10",
    ];
    if !slots.iter().zip(wrappers).all(|((_, wrapper), signature)| {
        wrapper[..wrapper.len().min(0x80)].windows(signature.len()).any(|bytes| bytes == signature)
    }) {
        return false;
    }
    let Some(receiver) = primary_command_receiver(
        image, sections, image_base, b".?AVkcTaskEffekseer@@\0", &code,
    ).and_then(&code) else { return false; };
    let receiver = &receiver[..receiver.len().min(0x480)];
    // 578 branches directly. 579–582 share a jump table in this receiver.
    let load = b"\x81\xf9\x42\x02\0\0\x0f\x8f";
    let playback = b"\x8d\x81\xbd\xfd\xff\xff\x83\xf8\x03\x0f\x87";
    receiver.windows(load.len()).any(|bytes| bytes == load)
        && receiver.windows(playback.len()).position(|bytes| bytes == playback).is_some_and(|offset| {
            let jump = offset + playback.len() + 4;
            if receiver.get(jump..jump + 3) != Some(b"\xff\x24\x85") { return false; }
            read_word(receiver, jump + 3).and_then(|va| va.checked_sub(image_base)).and_then(&code)
                .and_then(|table| table.get(..16)).is_some_and(|table| {
                    table.chunks_exact(4).all(|entry| {
                        read_word(entry, 0).and_then(|va| va.checked_sub(image_base)).and_then(&code).is_some()
                    })
                })
        })
}

pub(super) fn tweet_family<'a>(
    slots: &[(u32, &'a [u8])],
    image: &[u8],
    sections: &[Section],
    image_base: u32,
    code: impl Fn(u32) -> Option<&'a [u8]>,
) -> bool {
    let Some(slots) = slots.get(1018..=1023) else { return false; };
    let wrappers: [&[u8]; 6] = [
        b"\x68\x47\x02\0\0\xff\x37\x8b\xcb\xff\x73\x04\xff\x50\x10",
        b"\x68\x48\x02\0\0\xff\x36\x8b\xcf\xff\x77\x04\xff\x50\x10",
        b"\x68\x49\x02\0\0\xff\x36\xff\x71\x04\xff\x50\x10\xb8\x02\0\0\0\x5e\xc2\x10\0",
        b"\x68\x4a\x02\0\0\xff\x36\xff\x71\x04\xff\x50\x10\xb8\x02\0\0\0\x5e\xc2\x10\0",
        b"\x68\x4b\x02\0\0\xff\x30\xff\x71\x04\xff\x52\x10\x8b\x4c\x24\x10\x8b\x44\x24\x08\x89\x01\xb8\x01\0\0\0\xc2\x10\0",
        b"\x68\x4c\x02\0\0\xff\x36\xff\x71\x04\xff\x50\x10\xb8\x02\0\0\0\x5e\xc2\x10\0",
    ];
    if !slots.iter().zip(wrappers).all(|((_, wrapper), signature)| {
        wrapper[..wrapper.len().min(0xe0)].windows(signature.len()).any(|bytes| bytes == signature)
    }) {
        return false;
    }
    let Some(receiver) = primary_command_receiver(
        image, sections, image_base, b".?AVkcTaskTweet@@\0", &code,
    ).and_then(&code) else { return false; };
    let dispatch = b"\x8b\x45\x0c\x05\xb9\xfd\xff\xff\x83\xf8\x05\x0f\x87";
    let prefix = &receiver[..receiver.len().min(0x80)];
    prefix.windows(dispatch.len()).position(|bytes| bytes == dispatch).is_some_and(|offset| {
        let jump = offset + dispatch.len() + 4;
        if receiver.get(jump..jump + 3) != Some(b"\xff\x24\x85") { return false; }
        read_word(receiver, jump + 3).and_then(|va| va.checked_sub(image_base)).and_then(|rva| {
            sections.iter().find_map(|section| section.file_bytes_from(image, rva)).and_then(|bytes| bytes.get(..24))
        }).is_some_and(|table| {
            table.chunks_exact(4).all(|entry| {
                read_word(entry, 0).and_then(|va| va.checked_sub(image_base)).and_then(&code).is_some()
            })
        })
    })
}

pub(super) fn wallpaper_package_wrapper(slots: &[(u32, &[u8])]) -> bool {
    let Some([(_, start), (_, poll)]) = slots.get(600..=601) else { return false; };
    // The package wrapper formats a second string from args[8] and sends it
    // alongside the package name. The adjacent wrapper queries its completion.
    let send_package = b"\x50\xff\x77\x08\xe8";
    let dispatch = b"\x8b\x03\x8d\x4c\x24\x0c\x51\x6a\0\x8d\x4c\x24\x18\x51\x6a\x02\x68";
    let status = b"\x8b\x4c\x24\x08\x8d\x44\x24\x08\x50\x8b\x44\x24\x10\x6a\0\x8b\x11\x68";
    let start = &start[..start.len().min(0xc0)];
    let Some(message) = start.windows(dispatch.len()).position(|bytes| bytes == dispatch)
        .filter(|&offset| start.get(offset + dispatch.len() + 4..).is_some_and(|tail|
            tail.starts_with(b"\xff\x37\x8b\xcb\xff\x73\x04\xff\x50\x10")))
        .and_then(|offset| read_word(start, offset + dispatch.len()))
    else { return false; };
    let Some(poll) = poll.strip_prefix(status) else { return false; };
    start.windows(send_package.len()).any(|bytes| bytes == send_package)
        && poll.get(4..7) == Some(b"\x6a\0\x68")
        && read_word(poll, 7).is_some_and(|query| message.checked_add(1) == Some(query))
        && poll.get(11..).is_some_and(|tail| tail.starts_with(
            b"\xff\x30\xff\x71\x04\xff\x52\x10\x8b\x4c\x24\x10\x8b\x44\x24\x08\x89\x01\xb8\x01\0\0\0\xc2\x10\0",
        ))
}

#[cfg(test)]
mod tests {
    use crate::native_table::OrdinaryNativeTable;
    use std::{fs, io, path::Path};

    #[test]
    fn shell_task_wrappers_survive_an_unrelated_pe_overlay() -> io::Result<()> {
        let games = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../games");
        let samples = [
            ("Yukikoi Melt/YukikoiMelt.exe", true, true, true),
            ("cs2_full_v401/system/cs2_open.exe", false, true, false),
            ("\u{30c4}\u{30ca}\u{30d0}\u{30f3} \u{3089}\u{3076}\u{307f}\u{304f}\u{3059}/cs2.exe", false, false, false),
        ];
        for (file, tweet, wallpaper_package, effekseer) in samples {
            let path = games.join(file);
            if !path.is_file() {
                continue;
            }
            let mut image = fs::read(&path)?;
            for suffix in [b"".as_slice(), b"Unrelated overlay".as_slice()] {
                image.extend_from_slice(suffix);
                let table = OrdinaryNativeTable::from_pe(&image)
                    .unwrap_or_else(|| panic!("missing ordinary native table in {file}"));
                assert_eq!(table.has_tweet_family(), tweet, "{file}");
                assert_eq!(table.has_wallpaper_package_wrapper(), wallpaper_package, "{file}");
                assert_eq!(table.has_effekseer_family(), effekseer, "{file}");
            }
        }
        Ok(())
    }
}
