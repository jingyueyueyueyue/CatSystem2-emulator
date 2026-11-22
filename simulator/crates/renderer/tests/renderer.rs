use cs2_renderer::{
    AlphaMask, AnimationFrameSelection, Canvas, CanvasReadback, Color, FrameFilter, FrameMetadata, FrameResources,
    FrameTransition, GlyphMask, ImageDrawOptions, Particle, ParticlePoint, ParticleSystem, Point,
    Rect, RenderCommand, RenderNode, RenderQueue, RendererError, RgbaFrame, RgbaFrameSet,
    TextEncoding, TextMarkup, TextSizePresets, TextStyle, TextToken, inspect_hg,
    map_viewport_point,
};
use image::{ColorType, ImageEncoder, codecs::jpeg::JpegEncoder};

#[test]
fn alpha_blend_uses_straight_rgba_source_over() {
    let mut canvas = Canvas::new(1, 1).unwrap();
    canvas.clear(Color::rgb(0, 0, 255));
    canvas
        .blend_pixel(0, 0, Color::rgba(255, 0, 0, 128))
        .unwrap();

    assert_eq!(canvas.pixel(0, 0).unwrap(), Color::rgba(128, 0, 127, 255));
}

#[test]
fn clipped_rectangle_only_changes_the_intersection() {
    let mut canvas = Canvas::new(4, 3).unwrap();
    canvas
        .fill_rect_clipped(
            Rect::new(-1, 0, 4, 3),
            Color::rgb(20, 40, 60),
            Some(Rect::new(1, 1, 2, 1)),
        )
        .unwrap();

    assert_eq!(canvas.pixel(0, 0).unwrap(), Color::TRANSPARENT);
    assert_eq!(canvas.pixel(1, 1).unwrap(), Color::rgb(20, 40, 60));
    assert_eq!(canvas.pixel(2, 1).unwrap(), Color::rgb(20, 40, 60));
    assert_eq!(canvas.pixel(3, 1).unwrap(), Color::TRANSPARENT);
}

#[test]
fn render_queue_sorts_by_priority_and_keeps_equal_priority_order() {
    let mut canvas = Canvas::new(1, 1).unwrap();
    let mut queue = RenderQueue::new();
    queue.push(RenderNode::new(
        10,
        RenderCommand::FillRect {
            rect: Rect::new(0, 0, 1, 1),
            color: Color::rgb(255, 0, 0),
            clip: None,
        },
    ));
    queue.push(RenderNode::new(
        0,
        RenderCommand::FillRect {
            rect: Rect::new(0, 0, 1, 1),
            color: Color::rgb(0, 0, 255),
            clip: None,
        },
    ));
    queue.push(RenderNode::new(
        10,
        RenderCommand::FillRect {
            rect: Rect::new(0, 0, 1, 1),
            color: Color::rgb(0, 255, 0),
            clip: None,
        },
    ));

    canvas.render_queue(&queue).unwrap();
    assert_eq!(canvas.pixel(0, 0).unwrap(), Color::rgb(0, 255, 0));
}

#[test]
fn image_mask_and_nearest_scale_are_applied_in_destination_space() {
    let frame = RgbaFrame::from_rgba8(2, 1, vec![255, 0, 0, 255, 0, 0, 255, 255]).unwrap();
    let mask = AlphaMask::new(4, 1, vec![0, 255, 255, 0]).unwrap();
    let mut options = ImageDrawOptions::new(Point::new(0, 0));
    options.scale_x = 2.0;
    options.mask = Some(mask);

    let mut canvas = Canvas::new(4, 1).unwrap();
    canvas.draw_frame(&frame, &options).unwrap();

    assert_eq!(canvas.pixel(0, 0).unwrap(), Color::TRANSPARENT);
    assert_eq!(canvas.pixel(1, 0).unwrap(), Color::rgb(255, 0, 0));
    assert_eq!(canvas.pixel(2, 0).unwrap(), Color::rgb(0, 0, 255));
    assert_eq!(canvas.pixel(3, 0).unwrap(), Color::TRANSPARENT);
}

#[test]
fn unscaled_nearest_source_over_matches_masked_raster_at_clipped_edges() {
    let source_alphas = [0, 1, 2, 127, 128, 254, 255];
    let source = (0_u8..20)
        .flat_map(|index| {
            [
                index * 11,
                255 - index * 7,
                index * 3,
                source_alphas[usize::from(index) % source_alphas.len()],
            ]
        })
        .collect();
    let frame = RgbaFrame::from_rgba8(5, 4, source).unwrap();
    let target = (0_u8..48)
        .flat_map(|index| {
            [
                index * 3,
                200 - index * 2,
                index * 5,
                [0, 73, 255][usize::from(index) % 3],
            ]
        })
        .collect();
    let base = Canvas::from_rgba8(8, 6, target).unwrap();
    let opaque_mask = AlphaMask::new(8, 6, vec![255; 48]).unwrap();

    for (position, clip) in [
        (Point::new(1, 1), None),
        (Point::new(-2, -1), None),
        (Point::new(6, 4), None),
        (Point::new(0, 0), Some(Rect::new(1, 1, 3, 2))),
        (Point::new(-1, 0), Some(Rect::new(2, 2, 1, 1))),
        (Point::new(i32::MIN, i32::MAX), None),
    ] {
        let mut options = ImageDrawOptions::new(position);
        options.clip = clip;
        let mut actual = base.clone();
        actual.draw_frame(&frame, &options).unwrap();

        options.mask = Some(opaque_mask.clone());
        let mut expected = base.clone();
        expected.draw_frame(&frame, &options).unwrap();
        assert_eq!(actual.pixels(), expected.pixels(), "{position:?}, {clip:?}");
    }
}

#[test]
fn glyph_and_particle_paths_use_masks_and_alpha() {
    let glyph = GlyphMask::from_alpha(2, 1, vec![255, 128]).unwrap();
    let mask = AlphaMask::new(3, 1, vec![255, 128, 0]).unwrap();
    let mut canvas = Canvas::new(3, 1).unwrap();

    canvas
        .draw_glyph(
            &glyph,
            Point::new(0, 0),
            Color::rgb(255, 255, 255),
            255,
            None,
            Some(&mask),
        )
        .unwrap();
    canvas
        .draw_particle(ParticlePoint::new(Point::new(2, 0), Color::rgb(255, 0, 0)))
        .unwrap();

    assert_eq!(canvas.pixel(0, 0).unwrap(), Color::rgb(255, 255, 255));
    assert_eq!(canvas.pixel(1, 0).unwrap(), Color::rgba(255, 255, 255, 64));
    assert_eq!(canvas.pixel(2, 0).unwrap(), Color::rgb(255, 0, 0));
}

#[test]
fn snapshot_hash_is_stable_and_changes_with_pixels() {
    let mut canvas = Canvas::new(2, 1).unwrap();
    let empty_hash = canvas.hash();
    canvas.set_pixel(0, 0, Color::rgba(1, 2, 3, 4)).unwrap();
    let snapshot = canvas.snapshot();

    assert_eq!(snapshot.dimensions(), (2, 1));
    assert_eq!(snapshot.rgba(), canvas.pixels());
    assert_eq!(snapshot.hash(), canvas.hash());
    assert_ne!(empty_hash, snapshot.hash());
}

#[test]
fn viewport_pointer_mapping_matches_stretched_frame_and_rejects_invalid_points() {
    assert_eq!(
        map_viewport_point([400.0, 300.0], 800, 600, 800, 600),
        Some(Point::new(400, 300))
    );
    assert_eq!(
        map_viewport_point([800.0, 450.0], 1600, 900, 800, 600),
        Some(Point::new(400, 300))
    );
    assert_eq!(
        map_viewport_point([1599.0, 899.0], 1600, 900, 800, 600),
        Some(Point::new(799, 599))
    );
    assert_eq!(map_viewport_point([800.0, 10.0], 800, 600, 800, 600), None);
    assert_eq!(map_viewport_point([0.0, 0.0], 0, 600, 800, 600), None);
    assert_eq!(
        map_viewport_point([f64::NAN, 10.0], 800, 600, 800, 600),
        None
    );
}

#[test]
fn png_frame_decodes_to_rgba() {
    let mut encoded = Vec::new();
    image::codecs::png::PngEncoder::new(&mut encoded)
        .write_image(&[9, 8, 7, 6], 1, 1, ColorType::Rgba8.into())
        .unwrap();

    let frame = RgbaFrame::decode(&encoded).unwrap();
    assert_eq!(frame.dimensions(), (1, 1));
    assert_eq!(frame.pixel(0, 0).unwrap(), Color::rgba(9, 8, 7, 6));
}

#[test]
fn invalid_data_reports_explicit_errors() {
    assert!(matches!(
        Canvas::from_rgba8(2, 2, vec![0; 3]),
        Err(RendererError::BufferLength { .. })
    ));

    let canvas = Canvas::new(2, 2).unwrap();
    assert!(matches!(
        canvas.pixel(2, 0),
        Err(RendererError::CoordinateOutOfBounds { .. })
    ));

    let frame = RgbaFrame::from_rgba8(1, 1, vec![255, 255, 255, 255]).unwrap();
    let mut options = ImageDrawOptions::default();
    options.scale_x = 0.0;
    assert!(matches!(
        Canvas::new(1, 1).unwrap().draw_frame(&frame, &options),
        Err(RendererError::InvalidScale { .. })
    ));
}

#[test]
fn hg2_old_and_new_headers_are_inspectable_without_placeholder_pixels() {
    let frame = RgbaFrame::from_rgba8(2, 1, vec![255, 0, 0, 255, 0, 0, 0, 0]).unwrap();

    let old = frame.encode_hg2_v20().unwrap();
    let inspection = inspect_hg(&old).unwrap();
    assert_eq!(inspection.container, cs2_renderer::HgContainer::Hg2V20);
    assert_eq!(inspection.frames.len(), 1);
    assert_eq!(inspection.frames[0].width, 1);
    assert_eq!(inspection.frames[0].image_offset, [0, 0]);
    assert!(inspection.has_native_pixels());

    let current = frame.encode_hg2().unwrap();
    let inspection = inspect_hg(&current).unwrap();
    assert_eq!(inspection.container, cs2_renderer::HgContainer::Hg2V25);
    assert_eq!(inspection.frames[0].alpha_flag, 1);
}

#[test]
fn hg_frame_sets_keep_frame_directory_offsets_and_ids() {
    let first = RgbaFrame::from_rgba8_with_metadata(
        1,
        1,
        vec![1, 2, 3, 255],
        FrameMetadata {
            frame_id: Some(4),
            ..FrameMetadata::raster([1, 1])
        },
    )
    .unwrap();
    let second = RgbaFrame::from_rgba8_with_metadata(
        1,
        1,
        vec![9, 8, 7, 255],
        FrameMetadata {
            frame_id: Some(9),
            ..FrameMetadata::raster([1, 1])
        },
    )
    .unwrap();
    let set = RgbaFrameSet::new(vec![first, second]).unwrap();
    let bytes = set.encode_hg3().unwrap();
    let inspection = inspect_hg(&bytes).unwrap();
    assert_eq!(
        inspection
            .frames
            .iter()
            .map(|frame| frame.id)
            .collect::<Vec<_>>(),
        vec![4, 9]
    );
    assert!(inspection.frames[0].offset < inspection.frames[1].offset);
    assert!(inspection.frames.iter().all(|frame| {
        frame
            .tags
            .iter()
            .any(|tag| tag.kind == cs2_renderer::HgTagKind::NativeImage)
    }));
}

#[test]
fn hg3_round_trips_asymmetric_rows_and_geometry() {
    let source = RgbaFrame::from_rgba8_with_metadata(
        2,
        2,
        vec![1, 2, 3, 255, 4, 5, 6, 127, 7, 8, 9, 255, 10, 11, 12, 255],
        FrameMetadata {
            offset: [4, 7],
            base: [2, -3],
            frame_id: Some(9),
            ..FrameMetadata::raster([8, 12])
        },
    )
    .unwrap();
    let encoded = source.encode_hg3().unwrap();
    let decoded = RgbaFrame::decode_hg(&encoded, AnimationFrameSelection::Id(9)).unwrap();
    assert_eq!(decoded.pixels(), source.pixels());
    assert_eq!(decoded.metadata().offset, source.metadata().offset);
    assert_eq!(decoded.metadata().base, source.metadata().base);
    assert_eq!(decoded.metadata().canvas_size, source.metadata().canvas_size);
}

#[test]
fn hg3_diagnostics_distinguish_supported_jpeg_from_jpa_and_numbered_slices() {
    let mut bytes = b"HG-3".to_vec();
    bytes.extend_from_slice(&12_u32.to_le_bytes());
    bytes.extend_from_slice(&0x300_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&7_u32.to_le_bytes());

    let mut stdinfo = Vec::new();
    for value in [2_u32, 1, 32, 0, 0, 2, 1, 1, 0, 0] {
        stdinfo.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(b"stdinfo\0");
    bytes.extend_from_slice(&56_u32.to_le_bytes());
    bytes.extend_from_slice(&(stdinfo.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&stdinfo);
    bytes.extend_from_slice(b"img_jpg\0");
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());

    let inspection = inspect_hg(&bytes).unwrap();
    assert_eq!(
        inspection.frames[0].tags[1].kind,
        cs2_renderer::HgTagKind::Jpeg
    );
    assert!(inspection.unsupported_tags().next().is_none());
    assert!(!inspection.has_native_pixels());
    assert!(matches!(
        inspection.require_native_pixels(),
        Err(RendererError::InvalidHgStructure { .. })
    ));

    let mut jpa_variant = bytes.clone();
    jpa_variant[76..84].copy_from_slice(b"img_jpa\0");
    let jpa_inspection = inspect_hg(&jpa_variant).unwrap();
    assert!(matches!(
        jpa_inspection.require_decodable_pixels(),
        Err(RendererError::UnsupportedHgEncoding { .. })
    ));
    let mut slice_variant = bytes;
    slice_variant[76..84].copy_from_slice(b"img0001\0");
    assert!(matches!(
        inspect_hg(&slice_variant),
        Err(RendererError::InvalidHgStructure { .. })
    ));
}

#[test]
fn decode_hg_loads_jpeg_frame_and_preserves_hg_geometry() {
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 100)
        .encode(&[32, 96, 160], 1, 1, ColorType::Rgb8.into())
        .unwrap();
    let mut bytes = b"HG-3".to_vec();
    bytes.extend_from_slice(&12_u32.to_le_bytes());
    bytes.extend_from_slice(&0x300_u32.to_le_bytes());
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&7_u32.to_le_bytes());
    bytes.extend_from_slice(b"stdinfo\0");
    bytes.extend_from_slice(&56_u32.to_le_bytes());
    bytes.extend_from_slice(&40_u32.to_le_bytes());
    for value in [1_u32, 1, 32, 0, 0, 1, 1, 1, 0, 0] {
        bytes.extend_from_slice(&value.to_le_bytes());
    }
    bytes.extend_from_slice(b"img_jpg\0");
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(&u32::try_from(jpeg.len()).unwrap().to_le_bytes());
    bytes.extend_from_slice(&jpeg);

    let frame = RgbaFrame::decode_hg(&bytes, AnimationFrameSelection::Id(7)).unwrap();
    assert_eq!(
        frame.metadata().container,
        cs2_renderer::HgContainer::Hg3V300
    );
    assert_eq!(frame.metadata().frame_id, Some(7));
    assert_eq!(frame.pixels()[3], 255);
    assert!(frame.pixels()[0].abs_diff(32) <= 8);
}

#[test]
fn hg_frame_info_attributes_attach_to_decoded_pixels_and_survive_transforms() {
    let source = RgbaFrame::from_rgba8(1, 1, vec![11, 22, 33, 255]).unwrap();
    let mut bytes = source.encode_hg3().unwrap();
    let mut attribute = b"ats0007\0".to_vec();
    attribute.extend_from_slice(&36_u32.to_le_bytes());
    attribute.extend_from_slice(&20_u32.to_le_bytes());
    for value in [10_u32, 20, 30, 40, 50] {
        attribute.extend_from_slice(&value.to_le_bytes());
    }
    bytes.splice(76..76, attribute);

    let inspection = inspect_hg(&bytes).unwrap();
    let info = inspection.frame(0).unwrap();
    assert_eq!(inspection.attribute(0, 7), Some([10, 20, 30, 40, 50]));
    assert_eq!(
        inspection.attributes(0).unwrap().get(7),
        Some([10, 20, 30, 40, 50])
    );

    let decoded =
        RgbaFrame::from_hg_rgba8_with_frame_info(source.pixels(), inspection.container, info)
            .unwrap();
    assert_eq!(
        decoded.metadata().container,
        cs2_renderer::HgContainer::Hg3V300
    );
    assert_eq!(decoded.metadata().frame_id, Some(0));
    assert_eq!(decoded.metadata().offset, info.image_offset);
    assert_eq!(decoded.metadata().canvas_size, info.canvas_size);
    assert_eq!(decoded.attribute(7), Some([10, 20, 30, 40, 50]));
    assert_eq!(
        decoded.flip_horizontal().unwrap().attribute(7),
        Some([10, 20, 30, 40, 50])
    );
    let reencoded = decoded.encode_hg3().unwrap();
    assert_eq!(
        inspect_hg(&reencoded).unwrap().attribute(0, 7),
        Some([10, 20, 30, 40, 50])
    );
    assert!(matches!(
        decoded.encode_hg2(),
        Err(RendererError::UnsupportedHgAttributes {
            container: cs2_renderer::HgContainer::Hg2V25
        })
    ));
    let mut alias = bytes.clone();
    alias[76..84].copy_from_slice(b"ats0x7\0\0");
    let native = inspection.frames[0].tags.iter()
        .find(|tag| tag.kind == cs2_renderer::HgTagKind::NativeImage).unwrap();
    let next = alias.len() - native.offset;
    alias[native.offset + 8..native.offset + 12].copy_from_slice(&(next as u32).to_le_bytes());
    alias.extend_from_slice(b"ats+7\0\0\0");
    alias.extend_from_slice(&0u32.to_le_bytes());
    alias.extend_from_slice(&20u32.to_le_bytes());
    alias.extend([60u32, 70, 80, 90, 100].into_iter().flat_map(u32::to_le_bytes));
    assert_eq!(inspect_hg(&alias).unwrap().attribute(0, 7), Some([60, 70, 80, 90, 100]));
    assert_eq!(RgbaFrame::decode_hg(&alias, AnimationFrameSelection::Index(0)).unwrap()
        .attribute(7), Some([60, 70, 80, 90, 100]));
}

#[test]
fn window_composition_keeps_pixels_without_claiming_source_hg_metadata() {
    let source = RgbaFrame::from_rgba8_with_metadata(
        1,
        1,
        vec![90, 80, 70, 255],
        FrameMetadata {
            container: cs2_renderer::HgContainer::Hg3V300,
            frame_id: Some(12),
            ..FrameMetadata::raster([1, 1])
        },
    )
    .unwrap()
    .with_hg_attributes(cs2_renderer::HgFrameAttributes::new(
        [(2, [1, 2, 3, 4, 5])].into(),
    ));
    let mut frames: [Option<RgbaFrame>; 9] = std::array::from_fn(|_| None);
    frames[0] = Some(source);

    let composed = Canvas::compose_window(&frames, [3, 2]).unwrap();
    assert_eq!(composed.dimensions(), (3, 2));
    assert_eq!(composed.pixel(0, 0).unwrap(), Color::rgb(90, 80, 70));
    assert_eq!(
        composed.metadata().container,
        cs2_renderer::HgContainer::None
    );
    assert_eq!(composed.metadata().frame_id, None);
    assert!(composed.hg_attributes().is_empty());
}

#[test]
fn frame_geometry_transforms_preserve_pixels_and_update_metadata() {
    let metadata = FrameMetadata {
        canvas_size: [4, 3],
        offset: [1, 0],
        base: [2, 1],
        ..FrameMetadata::raster([2, 2])
    };
    let frame = RgbaFrame::from_rgba8_with_metadata(
        2,
        2,
        vec![1, 0, 0, 255, 2, 0, 0, 255, 3, 0, 0, 255, 4, 0, 0, 255],
        metadata,
    )
    .unwrap();
    let flipped = frame.flip_horizontal().unwrap();
    assert_eq!(flipped.pixel(0, 0).unwrap(), Color::rgba(2, 0, 0, 255));
    assert_eq!(flipped.metadata().offset, [1, 0]);
    assert_eq!(flipped.metadata().base[0], 2);

    let rotated = frame.rotate_right_angle(90).unwrap().unwrap();
    assert_eq!(rotated.dimensions(), (2, 2));
    assert_eq!(rotated.pixel(1, 0).unwrap(), Color::rgba(1, 0, 0, 255));
    assert_eq!(rotated.metadata().canvas_size, [3, 4]);
}

#[test]
fn static_layers_union_crop_bounds_and_keep_canvas_geometry() {
    let first = RgbaFrame::from_rgba8_with_metadata(
        1,
        1,
        vec![255, 0, 0, 255],
        FrameMetadata {
            offset: [1, 1],
            canvas_size: [4, 4],
            ..FrameMetadata::raster([1, 1])
        },
    )
    .unwrap();
    let second = RgbaFrame::from_rgba8_with_metadata(
        1,
        1,
        vec![0, 0, 255, 128],
        FrameMetadata {
            offset: [0, 0],
            canvas_size: [3, 5],
            ..FrameMetadata::raster([1, 1])
        },
    )
    .unwrap();
    let composed = RgbaFrame::from_layers(&[first, second]).unwrap().unwrap();
    assert_eq!(composed.dimensions(), (2, 2));
    assert_eq!(composed.metadata().offset, [0, 0]);
    assert_eq!(composed.canvas_dimensions(), (4, 5));
    assert_eq!(composed.pixel(1, 1).unwrap(), Color::rgba(255, 0, 0, 255));
}

#[test]
fn static_layer_composition_aligns_bases_once_and_blends_transparency() {
    let first = RgbaFrame::from_rgba8_with_metadata(
        2,
        1,
        vec![0, 0, 0, 0, 255, 0, 0, 255],
        FrameMetadata {
            offset: [0, 0],
            base: [0, 0],
            canvas_size: [2, 1],
            ..FrameMetadata::raster([2, 1])
        },
    )
    .unwrap();
    let second = RgbaFrame::from_rgba8_with_metadata(
        1,
        1,
        vec![0, 0, 255, 128],
        FrameMetadata {
            offset: [1, 0],
            base: [1, 0],
            canvas_size: [2, 1],
            ..FrameMetadata::raster([1, 1])
        },
    )
    .unwrap();

    let composed = RgbaFrame::from_layers(&[first, second]).unwrap().unwrap();
    assert_eq!(composed.pixel(0, 0).unwrap(), Color::rgba(0, 0, 255, 128));
    assert_eq!(composed.pixel(1, 0).unwrap(), Color::rgb(255, 0, 0));
}

#[test]
fn alpha_masks_can_be_composed_and_applied_in_place() {
    let left = AlphaMask::new(2, 1, vec![255, 128]).unwrap();
    let right = AlphaMask::new(2, 1, vec![128, 255]).unwrap();
    let mixed = left.lerp(&right, 128).unwrap();
    assert!(mixed.alpha()[0] > 180);
    assert!(mixed.alpha()[1] > 180);

    let mut canvas = Canvas::from_rgba8(2, 1, vec![10, 20, 30, 255, 40, 50, 60, 255]).unwrap();
    canvas.apply_mask(&left, None).unwrap();
    assert_eq!(canvas.pixel(0, 0).unwrap().a, 255);
    assert_eq!(canvas.pixel(1, 0).unwrap().a, 128);
}

#[test]
fn frame_filters_and_canvas_resize_preserve_alpha_and_geometry() {
    let frame = RgbaFrame::from_rgba8_with_metadata(
        2,
        1,
        vec![100, 50, 0, 128, 10, 20, 30, 64],
        FrameMetadata {
            canvas_size: [4, 2],
            offset: [1, 0],
            base: [2, 1],
            ..FrameMetadata::raster([2, 1])
        },
    )
    .unwrap();
    let filtered = frame.apply_filter(FrameFilter::Negative).unwrap();
    assert_eq!(filtered.pixel(0, 0).unwrap().a, 128);
    assert_eq!(filtered.pixel(0, 0).unwrap().r, 155);
    let resized = frame
        .resize_canvas([8, 4], cs2_renderer::ScaleFilter::Nearest)
        .unwrap();
    assert_eq!(resized.dimensions(), (4, 2));
    assert_eq!(resized.canvas_dimensions(), (8, 4));
    assert_eq!(resized.metadata().offset, [2, 0]);
    assert_eq!(resized.metadata().base, [4, 2]);
}

#[test]
fn frame_transition_advances_with_clamped_progress() {
    let from = RgbaFrame::from_rgba8(1, 1, vec![255, 0, 0, 255]).unwrap();
    let to = RgbaFrame::from_rgba8(1, 1, vec![0, 0, 255, 255]).unwrap();
    let mut transition = FrameTransition::crossfade(from, to, 4).unwrap();
    assert_eq!(transition.progress(), 0);
    transition.advance(2);
    assert_eq!(transition.progress(), 128);
    transition.advance(100);
    assert!(transition.complete());
    assert_eq!(transition.progress(), 256);
}

#[test]
fn text_markup_keeps_grouping_styles_waits_and_identifiers() {
    let markup = TextMarkup::parse(
        r"[base/ruby]\c11223344\fs\tbi;tail\w3\wf2\id7:9\@",
        TextStyle::default(),
    )
    .unwrap();
    assert!(matches!(
        markup.tokens()[0],
        TextToken::Glyph {
            character: 'b',
            ruby: false,
            ..
        }
    ));
    assert!(markup.tokens().iter().any(|token| matches!(
        token,
        TextToken::Glyph {
            character: 'r',
            ruby: true,
            ..
        }
    )));
    assert!(
        markup
            .tokens()
            .iter()
            .any(|token| matches!(token, TextToken::WaitCharacters(3)))
    );
    assert!(
        markup
            .tokens()
            .iter()
            .any(|token| matches!(token, TextToken::WaitFrames(2)))
    );
    assert_eq!(markup.identifiers().get(&7), Some(&9));
    assert!(
        markup
            .tokens()
            .iter()
            .any(|token| matches!(token, TextToken::Continue))
    );
    let ruby_run = markup
        .layout_with_settings(
            &cs2_renderer::TextLayoutSettings {
                origin: Point::new(0, 20),
                width: 200,
                line_height: 20,
                letter_spacing: 0,
                line_spacing: 0,
                ruby_offset: 0,
                ..Default::default()
            },
            |_, size, _, _| {
                GlyphMask::from_alpha_with_metrics(
                    1,
                    1,
                    vec![255],
                    cs2_renderer::GlyphMetrics {
                        advance: size as i32,
                        ..Default::default()
                    },
                )
            },
        )
        .unwrap();
    let base = ruby_run.glyphs().iter().find(|glyph| !glyph.ruby).unwrap();
    let ruby = ruby_run.glyphs().iter().find(|glyph| glyph.ruby).unwrap();
    assert_eq!(base.group, ruby.group);
    // Short ruby distributes spare advance across its intervals; it does not
    // center the reading as one compact word.
    assert_eq!(ruby.baseline.x, 0);

    let unicode =
        TextMarkup::parse_with_encoding(r"\s\u", TextStyle::default(), TextEncoding::Unicode)
            .unwrap();
    assert!(unicode.tokens().iter().any(|token| matches!(
        token,
        TextToken::Glyph {
            character: '\u{8140}',
            ..
        }
    )));
    assert!(
        unicode
            .tokens()
            .iter()
            .any(|token| matches!(token, TextToken::Control(6)))
    );
    let run = unicode
        .layout(Point::new(0, 20), 20, 0, |character, size, _, _| {
            GlyphMask::from_alpha_with_metrics(
                1,
                1,
                vec![255],
                cs2_renderer::GlyphMetrics {
                    advance: size as i32,
                    ..Default::default()
                },
            )
            .map(|glyph| {
                assert!(character == '\u{3000}' || character == '\u{8140}' || character == 'u');
                glyph
            })
        })
        .unwrap();
    assert!(!run.glyphs().is_empty());
    let presets = TextMarkup::parse_with_sizes(
        r"\fssA\flB",
        TextStyle::default(),
        TextEncoding::Cp932,
        TextSizePresets {
            text: [12, 16, 20, 24, 28],
            ruby: [6, 8, 10, 12, 14],
        },
    )
    .unwrap();
    assert!(presets.tokens().iter().any(|token| matches!(
        token,
        TextToken::Glyph {
            character: 'A',
            style: TextStyle { size: 12, .. },
            ..
        }
    )));
}

#[test]
fn text_layout_does_not_start_a_line_with_forbidden_cjk_punctuation() {
    let markup = TextMarkup::parse("AB、C", TextStyle::default()).unwrap();
    let run = markup
        .layout_with_settings(
            &cs2_renderer::TextLayoutSettings {
                origin: Point::new(0, 20),
                width: 20,
                line_height: 20,
                letter_spacing: 0,
                line_spacing: 0,
                ruby_offset: 0,
                ..Default::default()
            },
            |_, _, _, _| {
                GlyphMask::from_alpha_with_metrics(
                    1,
                    1,
                    vec![255],
                    cs2_renderer::GlyphMetrics {
                        advance: 10,
                        ..Default::default()
                    },
                )
            },
        )
        .unwrap();
    let punctuation = run
        .glyphs()
        .iter()
        .find(|glyph| glyph.group.is_none() && glyph.glyph.advance() == 10)
        .unwrap();
    let c = run.glyphs().iter().rev().find(|glyph| !glyph.ruby).unwrap();
    assert_eq!(punctuation.baseline.y, 20);
    assert!(c.baseline.y > punctuation.baseline.y);
}

#[test]
fn text_color_restore_and_unicode_u_control_keep_marker_semantics() {
    let initial = TextStyle {
        color: Color::rgb(10, 20, 30),
        ..TextStyle::default()
    };
    let markup = TextMarkup::parse(r"\cd0xff0000ff;A\cB", initial).unwrap();
    let glyphs = markup
        .tokens()
        .iter()
        .filter_map(|token| match token {
            TextToken::Glyph {
                character, style, ..
            } => Some((*character, style.color)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(glyphs[0], ('A', initial.color));
    assert_eq!(glyphs[1], ('B', Color::rgb(255, 0, 0)));

    let unicode =
        TextMarkup::parse_with_encoding(r"\uX", TextStyle::default(), TextEncoding::Unicode)
            .unwrap();
    assert!(matches!(unicode.tokens()[0], TextToken::Control(6)));
    assert!(matches!(
        unicode.tokens()[1],
        TextToken::Glyph { character: 'X', .. }
    ));
}

#[test]
fn particle_system_updates_motion_opacity_and_releases_expired_particles() {
    let mut system = ParticleSystem::new();
    let mut particle = Particle::new([1.0, 1.0], Color::WHITE);
    particle.velocity = [1.0, 0.0];
    particle.lifetime = 3;
    particle.opacity_start = 0;
    particle.opacity_end = 255;
    system.spawn(particle).unwrap();
    system.update(1).unwrap();
    assert_eq!(system.particles()[0].position, [2.0, 1.0]);
    assert!(system.particles()[0].opacity() < 255);
    system.update(2).unwrap();
    assert!(system.particles().is_empty());
}

#[test]
fn frame_resources_release_last_reference_without_losing_shared_pixels() {
    let frame = RgbaFrame::from_rgba8(1, 1, vec![9, 8, 7, 6]).unwrap();
    let mut resources = FrameResources::new();
    let handle = resources.insert(frame).unwrap();
    resources.retain(handle).unwrap();
    assert_eq!(resources.references(handle).unwrap(), 2);
    assert!(!resources.release(handle).unwrap());
    assert_eq!(
        resources.get(handle).unwrap().pixel(0, 0).unwrap(),
        Color::rgba(9, 8, 7, 6)
    );
    assert!(resources.release(handle).unwrap());
    assert!(!resources.contains(handle));
    assert!(matches!(
        resources.get(handle),
        Err(RendererError::InvalidResourceHandle { .. })
    ));
}

#[test]
fn frame_resource_replacement_keeps_leased_version_and_clear_does_not_reuse_handles() {
    let mut resources = FrameResources::default();
    let old = RgbaFrame::from_rgba8(1, 1, vec![9, 8, 7, 255]).unwrap();
    let old_handle = resources.insert(old).unwrap();
    let lease = resources.acquire(old_handle).unwrap();
    let replacement = RgbaFrame::from_rgba8(1, 1, vec![1, 2, 3, 255]).unwrap();

    resources.replace(old_handle, replacement).unwrap();
    assert_eq!(lease.pixel(0, 0).unwrap(), Color::rgb(9, 8, 7));
    assert_eq!(
        resources.get(old_handle).unwrap().pixel(0, 0).unwrap(),
        Color::rgb(1, 2, 3)
    );
    assert_eq!(resources.references(old_handle).unwrap(), 2);
    assert!(!lease.release(&mut resources).unwrap());
    assert!(resources.release(old_handle).unwrap());

    resources.clear();
    let new_handle = resources
        .insert(RgbaFrame::from_rgba8(1, 1, vec![4, 5, 6, 255]).unwrap())
        .unwrap();
    assert_ne!(old_handle, new_handle);
}

#[test]
fn frame_resource_sets_release_all_animation_frames() {
    let set = RgbaFrameSet::new(vec![
        RgbaFrame::from_rgba8(1, 1, vec![1, 2, 3, 255]).unwrap(),
        RgbaFrame::from_rgba8(1, 1, vec![4, 5, 6, 255]).unwrap(),
    ])
    .unwrap();
    let mut resources = FrameResources::new();
    let handles = resources.insert_set(&set).unwrap();
    assert_eq!(handles.len(), 2);
    resources.release_all(handles.iter().copied()).unwrap();
    assert!(!resources.contains(handles[0]));
    assert!(!resources.contains(handles[1]));
}


#[derive(Debug)]
struct TestCanvasReadback {
    rgba: Vec<u8>,
    calls: std::sync::atomic::AtomicUsize,
}

impl CanvasReadback for TestCanvasReadback {
    fn read_rgba(&self, output: &mut [u8]) -> bool {
        self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        if output.len() != self.rgba.len() { return false; }
        output.copy_from_slice(&self.rgba);
        true
    }

    fn as_any(&self) -> &dyn std::any::Any { self }
}

#[test]
fn deferred_device_pixels_materialize_once_and_clear_skips_readback() {
    use std::sync::{Arc, atomic::Ordering};

    let source = Arc::new(TestCanvasReadback {
        rgba: vec![20, 40, 60, 80],
        calls: 0.into(),
    });
    let mut canvas = Canvas::from_rgba8(1, 1, vec![1, 2, 3, 4]).unwrap();
    let allocation = canvas.pixels().as_ptr();
    canvas.set_deferred_device_pixels(source.clone());
    assert_eq!(source.calls.load(Ordering::Relaxed), 0);
    assert!(canvas.device_pixels().is_some());
    assert_eq!(canvas.pixels(), [20, 40, 60, 80]);
    assert_eq!(canvas.clone().pixels(), [20, 40, 60, 80]);
    assert_eq!(source.calls.load(Ordering::Relaxed), 1);
    canvas.clear(Color::rgba(5, 6, 7, 8));
    assert_eq!(canvas.pixels(), [5, 6, 7, 8]);
    assert_eq!(canvas.pixels().as_ptr(), allocation);
    assert_eq!(source.calls.load(Ordering::Relaxed), 1);
    assert!(canvas.device_pixels().is_none());

    canvas.set_deferred_device_pixels(source.clone());
    canvas.clear(Color::rgba(9, 10, 11, 12));
    assert_eq!(source.calls.load(Ordering::Relaxed), 1);
    assert_eq!(canvas.pixels(), [9, 10, 11, 12]);
    assert_eq!(canvas.pixels().as_ptr(), allocation);
}
