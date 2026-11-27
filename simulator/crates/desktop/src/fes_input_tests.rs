use super::*;

fn driver(lines: &[&[u8]]) -> (FesScriptDriver, ExpressionState) {
    let document = FesDocument {
        sections: vec![cs2_script::FesSection {
            header: b"START".to_vec(),
            lines: lines
                .iter()
                .enumerate()
                .map(|(offset, line)| cs2_script::FesLine {
                    offset,
                    bytes: line.to_vec(),
                })
                .collect(),
        }],
    };
    let mut driver = FesScriptDriver::new(&document).unwrap();
    let mut shared = ExpressionState::default();
    driver.set_named_value("_CLICK_L_", 0, 0);
    driver.set_named_value("_CLICK_R_", 0, 0);
    while driver.initialization_pending() {
        driver.advance_initialization(&mut shared).unwrap();
    }
    (driver, shared)
}

fn notify_press(driver: &mut FesScriptDriver, shared: &mut ExpressionState, code: u32, event: &[u8]) {
    let scope = driver.begin_media_notification_scope();
    driver
        .on_input(FesDriverInput {
            code,
            target: Some(FesDriverTarget {
                name: b"start_button".to_vec(),
                bank: 0,
                default_group: None,
                value: 17,
            }),
            event: Some(event.to_vec()),
        })
        .unwrap();
    assert!(matches!(
        driver.advance_media_notification(4, shared).unwrap(),
        Some(FesDriverEffect::ObjectEvent {
            phase: FesObjectEventPhase::Complete { handled: false, .. },
            ..
        }),
    ));
    assert!(!driver.has_media_notification_work());
    driver.end_media_notification_scope(scope);
}

#[test]
fn fresh_fes_dialog_waits_until_a_child_really_exits() -> Result<()> {
    let (mut driver, mut shared) = driver(&[
        b"if (_EXITCODE_==0)", b"exit 42", b"endif", b"wait 2", b"next START",
    ]);
    for frame in [10, 11] {
        assert!(matches!(
            driver.advance(frame, &mut shared, 32)?,
            FesDriverEffect::Waiting { .. }
        ));
    }
    driver.set_child_exit_code(0);
    assert!(matches!(driver.advance(12, &mut shared, 32)?, FesDriverEffect::Exit(42)));
    Ok(())
}

#[test]
fn fes_wait_voice_without_an_object_uses_the_numeric_duration() -> Result<()> {
    for (value, duration) in [(0, 1), (3, 3)] {
        let (mut driver, mut shared) = driver(&[b"wait voice", b"exit -2"]);
        driver.set_named_value("voice", 0, value);
        let until = 1873 + duration;
        for frame in 1873..until {
            assert!(matches!(
                driver.advance(frame, &mut shared, 32)?,
                FesDriverEffect::Waiting {
                    wait: FesDriverWait::Frames { until: deadline },
                    handler_local: false,
                } if deadline == until
            ));
        }
        assert!(matches!(driver.advance(until, &mut shared, 32)?, FesDriverEffect::Exit(-2)));
    }
    Ok(())
}

#[test]
fn reset_frame_restarts_fes_absolute_wait_and_visible_counter() -> Result<()> {
    let (mut driver, mut shared) = driver(&[
        b"reset_frame", b"$0 = _FRAME_", b"wait frame 2", b"$1 = _FRAME_", b"exit 42",
    ]);
    shared.shared.resize(2, -1);
    assert!(matches!(driver.advance(100, &mut shared, 32)?, FesDriverEffect::Waiting { .. }));
    assert_eq!(shared.shared[0], 0);
    let snapshot = driver.snapshot();
    assert!(matches!(driver.advance(100, &mut shared, 32)?, FesDriverEffect::Waiting { .. }));
    driver.restore_snapshot(&snapshot);
    assert!(matches!(driver.advance(105, &mut shared, 32)?, FesDriverEffect::Waiting { .. }));
    assert!(matches!(driver.advance(106, &mut shared, 32)?, FesDriverEffect::Exit(42)));
    assert_eq!(shared.shared[1], 2);
    Ok(())
}

#[test]
fn automove_and_restoremove_dispatch_ordered_pointer_requests() -> Result<()> {
    let section = |header: &[u8], lines: &[&[u8]]| cs2_script::FesSection {
        header: header.to_vec(),
        lines: lines.iter().enumerate().map(|(offset, line)| cs2_script::FesLine {
            offset, bytes: line.to_vec(),
        }).collect(),
    };
    let document = FesDocument { sections: vec![
        section(b"DEFINE", &[b"BUTTON button[2]"]),
        section(b"START", &[b"automove button[1]", b"restoremove", b"exit 0"]),
    ] };
    let mut driver = FesScriptDriver::new(&document)?;
    let mut shared = ExpressionState::default();
    while driver.initialization_pending() {
        driver.advance_initialization(&mut shared)?;
    }
    assert!(matches!(driver.advance(10, &mut shared, 32)?,
        FesDriverEffect::ControlFlow(main_flow::controls_flow::Request::AutoMove(target))
            if target.name == b"button" && target.bank == 1));
    assert!(matches!(driver.advance(10, &mut shared, 32)?,
        FesDriverEffect::ControlFlow(main_flow::controls_flow::Request::RestoreMove)));
    assert!(matches!(driver.advance(10, &mut shared, 32)?, FesDriverEffect::Exit(0)));
    Ok(())
}

#[test]
fn routed_click_reaches_next_title_poll_only_once() {
    let (mut driver, mut shared) = driver(&[b"if (_CLICK_L_==17)", b"exit 42", b"endif"]);
    assert!(matches!(
        driver.advance(10, &mut shared, 32).unwrap(),
        FesDriverEffect::Frame
    ));
    notify_press(&mut driver, &mut shared, 2, b"PUSH_L");
    assert!(matches!(
        driver.advance(11, &mut shared, 32).unwrap(),
        FesDriverEffect::Exit(42)
    ));
    assert!(matches!(
        driver.advance(11, &mut shared, 32).unwrap(),
        FesDriverEffect::Frame
    ));
    assert!(matches!(
        driver.advance(12, &mut shared, 32).unwrap(),
        FesDriverEffect::Frame
    ));
}

#[test]
fn routed_right_click_reaches_the_next_fes_poll_only_once() -> Result<()> {
    let (mut driver, mut shared) = driver(&[b"if (_CLICK_R_==17)", b"exit 42", b"endif"]);
    assert!(matches!(driver.advance(10, &mut shared, 32)?, FesDriverEffect::Frame));
    notify_press(&mut driver, &mut shared, 3, b"PUSH_R");
    assert!(matches!(driver.advance(11, &mut shared, 32)?, FesDriverEffect::Exit(42)));
    assert!(matches!(driver.advance(11, &mut shared, 32)?, FesDriverEffect::Frame));
    assert!(matches!(driver.advance(12, &mut shared, 32)?, FesDriverEffect::Frame));
    Ok(())
}

#[test]
fn routed_click_reaches_wait_cancellation() {
    let (mut driver, mut shared) = driver(&[b"wait 100", b"exit 42"]);
    assert!(matches!(
        driver.advance(10, &mut shared, 32).unwrap(),
        FesDriverEffect::Waiting { .. }
    ));
    notify_press(&mut driver, &mut shared, 2, b"PUSH_L");
    assert!(matches!(
        driver.advance(11, &mut shared, 32).unwrap(),
        FesDriverEffect::CancelWait { .. }
    ));
    assert!(matches!(
        driver.advance(11, &mut shared, 32).unwrap(),
        FesDriverEffect::Exit(42)
    ));
}

fn message_callback_driver() -> Result<(FesScriptDriver, ExpressionState)> {
    let section = |header: &[u8], base, lines: &[&[u8]]| cs2_script::FesSection {
        header: header.to_vec(),
        lines: lines.iter().enumerate().map(|(index, line)| cs2_script::FesLine {
            offset: base + index,
            bytes: line.to_vec(),
        }).collect(),
    };
    let document = FesDocument { sections: vec![
        section(b"START", 0, &[b"call BODY"]),
        section(b"BODY", 10, &[b"wait 100"]),
        section(b"MES_SHOW", 20, &[b"\\52 = \\0", b"call SETBUTTON", b"pl disp \\52"]),
        section(b"MES_WAITINPUT", 30, &[b"call SETBUTTON"]),
        section(b"SETBUTTON", 40, &[b"call _SETBUTTON", b"\\0 = 0"]),
        section(b"_SETBUTTON", 50, &[b"\\100 += 1"]),
        section(b"MES_OTHER", 60, &[b"call SETBUTTON", b"pl disp 1"]),
    ] };
    let mut driver = FesScriptDriver::new(&document)?;
    let mut shared = ExpressionState::default();
    while driver.initialization_pending() {
        driver.advance_initialization(&mut shared)?;
    }
    Ok((driver, shared))
}

#[test]
fn fes_system_block_returns_through_nested_calls_before_restoring_body_wait() -> Result<()> {
    let (mut driver, mut shared) = message_callback_driver()?;
    driver.set_rebellions_message_fes_source(true);
    let expected_wait = FesDriverWait::Frames { until: 110 };
    assert!(matches!(driver.advance(10, &mut shared, 64)?, FesDriverEffect::Waiting { .. }));
    assert_eq!(driver.interpreter.call_depth(), 1);
    let body_position = driver.interpreter.position();

    driver.interpreter.local_values_mut()[0] = 1;
    assert!(driver.dispatch_system_block(b"MES_SHOW")?);
    assert!(matches!(driver.advance_notification(10, &mut shared, 64)?,
        FesDriverEffect::SystemBlock { name, complete: false } if name == b"MES_SHOW"));
    assert!(matches!(driver.advance_notification(10, &mut shared, 64)?,
        FesDriverEffect::Command(command) if command.context.block == b"MES_SHOW"
            && command.context.local[52] == 1 && command.context.local[0] == 0));
    assert!(matches!(driver.advance_notification(10, &mut shared, 64)?,
        FesDriverEffect::SystemBlock { name, complete: true } if name == b"MES_SHOW"));

    assert!(driver.dispatch_system_block(b"MES_WAITINPUT")?);
    assert!(matches!(driver.advance_notification(11, &mut shared, 64)?,
        FesDriverEffect::SystemBlock { name, complete: false } if name == b"MES_WAITINPUT"));
    assert!(matches!(driver.advance_notification(11, &mut shared, 64)?,
        FesDriverEffect::SystemBlock { name, complete: true } if name == b"MES_WAITINPUT"));
    assert_eq!(driver.interpreter.local_values()[52], 1);
    assert_eq!(driver.interpreter.local_values()[100], 2);
    assert_eq!(driver.interpreter.position(), body_position);
    assert_eq!(driver.interpreter.call_depth(), 1);
    assert_eq!(driver.waiting(), Some(&expected_wait));
    Ok(())
}

#[test]
fn fes_system_block_keeps_legacy_return_outside_verified_rebellions_callbacks() -> Result<()> {
    for (verified, block) in [(false, b"MES_SHOW".as_slice()), (true, b"MES_OTHER".as_slice())] {
        let (mut driver, mut shared) = message_callback_driver()?;
        driver.set_rebellions_message_fes_source(verified);
        assert!(matches!(driver.advance(10, &mut shared, 64)?, FesDriverEffect::Waiting { .. }));
        let body_position = driver.interpreter.position();
        driver.interpreter.local_values_mut()[0] = 1;
        assert!(driver.dispatch_system_block(block)?);
        assert!(matches!(driver.advance_notification(10, &mut shared, 64)?,
            FesDriverEffect::SystemBlock { name, complete: false } if name == block));
        assert!(matches!(driver.advance_notification(10, &mut shared, 64)?,
            FesDriverEffect::SystemBlock { name, complete: true } if name == block));
        assert_eq!(driver.interpreter.position(), body_position);
        assert_eq!(driver.interpreter.local_values()[0], 1);
        assert_eq!(driver.interpreter.local_values()[100], 1);
        assert_eq!(driver.waiting(), Some(&FesDriverWait::Frames { until: 110 }));
    }
    Ok(())
}

#[test]
fn fes_choice_reset_yields_in_body_before_redrawing_labels() -> Result<()> {
    let document = FesDocument {
        sections: [
            (b"START".as_slice(), &[b"wait 100".as_slice()][..]),
            (b"SEL_RESET", &[b"next SETSEL_0".as_slice()][..]),
            (b"SETSEL_0", &[b"str clear".as_slice(), b"wait 1", b"next SETSEL"][..]),
            (b"SETSEL", &[b"str[0] draw -1".as_slice()][..]),
        ].into_iter().enumerate().map(|(section, (header, lines))| cs2_script::FesSection {
            header: header.to_vec(),
            lines: lines.iter().enumerate().map(|(index, line)| cs2_script::FesLine {
                offset: 10 * section + index,
                bytes: line.to_vec(),
            }).collect(),
        }).collect(),
    };
    let mut driver = FesScriptDriver::new(&document)?;
    let mut shared = ExpressionState::default();
    while driver.initialization_pending() {
        driver.advance_initialization(&mut shared)?;
    }
    assert!(matches!(driver.advance(10, &mut shared, 64)?, FesDriverEffect::Waiting { .. }));
    assert!(driver.dispatch_system_block(b"SEL_RESET")?);
    for complete in [false, true] {
        assert!(matches!(driver.advance_notification(10, &mut shared, 64)?,
            FesDriverEffect::SystemBlock { name, complete: arrived }
            if name == b"SEL_RESET" && arrived == complete));
    }
    assert!(matches!(driver.advance(10, &mut shared, 64)?,
        FesDriverEffect::Command(command) if command.context.block == b"SETSEL_0"));
    assert!(matches!(driver.advance(10, &mut shared, 64)?,
        FesDriverEffect::Waiting { wait: FesDriverWait::Frames { until: 11 }, handler_local: false }));
    assert!(matches!(driver.advance(11, &mut shared, 64)?,
        FesDriverEffect::Command(command) if command.context.block == b"SETSEL"));
    Ok(())
}

#[test]
fn kanmi_message_notifications_preserve_the_main_wait() -> Result<()> {
    let section = |header: &[u8], base, lines: &[&[u8]]| cs2_script::FesSection {
        header: header.to_vec(),
        lines: lines.iter().enumerate().map(|(index, line)| cs2_script::FesLine {
            offset: base + index,
            bytes: line.to_vec(),
        }).collect(),
    };
    let document = FesDocument { sections: vec![
        section(b"START", 0, &[b"call BODY", b"next WAIT"]),
        section(b"BODY", 10, &[b"wait 1"]),
        section(b"WAIT", 20, &[b"keywait disp 1"]),
        section(b"MES_SHOW", 30, &[b"call SETBUTTON", b"keywait disp 0"]),
        section(b"MES_SETSTATE", 40, &[b"if (\\53==0)", b"call SETBUTTON", b"endif"]),
        section(b"SETBUTTON", 50, &[b"call _SETBUTTON", b"\\0 = 1", b"call _SETBUTTON"]),
        section(b"_SETBUTTON", 60, &[b"\\62 += 1"]),
    ] };
    for (block, expected_tail) in [(b"MES_SHOW".as_slice(), true), (b"MES_SETSTATE".as_slice(), false)] {
        let mut driver = FesScriptDriver::new(&document)?;
        let mut shared = ExpressionState::default();
        while driver.initialization_pending() {
            driver.advance_initialization(&mut shared)?;
        }
        assert!(matches!(driver.advance(10, &mut shared, 64)?, FesDriverEffect::Waiting { .. }));
        let position = driver.interpreter.position();
        let depth = driver.interpreter.call_depth();
        assert_eq!(depth, 1);
        assert!(driver.dispatch_system_block(block)?);
        let returns = Some(FesSystemReturns::ButtonBlocks(block));
        assert!(matches!(driver.advance_system_notification(10, &mut shared, 2, returns)?,
            FesDriverEffect::SystemBlock { name, complete: false } if name == block));
        let mut tails = 0;
        let mut completed = false;
        for _ in 0..32 {
            match driver.advance_system_notification(10, &mut shared, 2, returns)? {
                FesDriverEffect::Yield => {}
                FesDriverEffect::Command(command) => {
                    assert!(expected_tail && command.context.block == b"MES_SHOW");
                    tails += 1;
                }
                FesDriverEffect::SystemBlock { name, complete: true } if name == block => {
                    completed = true;
                    break;
                }
                effect => panic!("unexpected notification effect: {effect:?}"),
            }
        }
        assert!(completed);
        assert_eq!(driver.interpreter.position(), position);
        assert_eq!(driver.waiting(), Some(&FesDriverWait::Frames { until: 11 }));
        assert_eq!(driver.interpreter.call_depth(), depth, "{block:?} leaked a return address");
        assert_eq!(driver.interpreter.local_values()[62], 2);
        assert_eq!(tails, usize::from(expected_tail));
        assert!(matches!(driver.advance(11, &mut shared, 64)?,
            FesDriverEffect::Command(command) if command.context.block == b"WAIT"));
        assert!(matches!(driver.advance(11, &mut shared, 64)?, FesDriverEffect::Frame));
        assert_eq!(driver.interpreter.current_block(), b"WAIT");
        assert_eq!(driver.interpreter.call_depth(), 0);
    }
    Ok(())
}

#[test]
fn hatsumira_message_notification_enables_each_button_and_restores_wait() -> Result<()> {
    let section = |header: &[u8], base, lines: &[&[u8]]| cs2_script::FesSection {
        header: header.to_vec(),
        lines: lines.iter().enumerate().map(|(index, line)| cs2_script::FesLine {
            offset: base + index,
            bytes: line.to_vec(),
        }).collect(),
    };
    let document = FesDocument { sections: vec![
        section(b"START", 0, &[b"call BODY"]),
        section(b"BODY", 10, &[b"wait 1"]),
        section(b"MES_SHOW", 20, &[br"\0 = 0", b"call SETBUTTON", br"\3 = 1"]),
        section(b"SETBUTTON", 30, &[b"call SETBTNSTATE", br"\0 = 1", b"call SETBTNSTATE"]),
        section(b"SETBTNSTATE", 40, &[b"call SET_BTN_ENABLE"]),
        section(b"SET_BTN_ENABLE", 50, &[br"\(1+\0) = 1"]),
    ] };
    let mut driver = FesScriptDriver::new(&document)?;
    let mut shared = ExpressionState::default();
    while driver.initialization_pending() {
        driver.advance_initialization(&mut shared)?;
    }
    assert!(matches!(driver.advance(10, &mut shared, 32)?, FesDriverEffect::Waiting { .. }));
    let position = driver.interpreter.position();
    assert_eq!(driver.interpreter.call_depth(), 1);
    assert!(driver.dispatch_system_block(b"MES_SHOW")?);
    let returns = Some(FesSystemReturns::NestedCalls(b"MES_SHOW"));
    for _ in 0..32 {
        match driver.advance_system_notification(10, &mut shared, 1, returns)? {
            FesDriverEffect::Yield | FesDriverEffect::SystemBlock { complete: false, .. } => {}
            FesDriverEffect::SystemBlock { complete: true, .. } => {
                assert_eq!(&driver.interpreter.local_values()[1..4], &[1, 1, 1]);
                assert_eq!(driver.interpreter.position(), position);
                assert_eq!(driver.interpreter.call_depth(), 1);
                assert_eq!(driver.waiting(), Some(&FesDriverWait::Frames { until: 11 }));
                return Ok(());
            }
            effect => panic!("unexpected notification effect: {effect:?}"),
        }
    }
    bail!("MES_SHOW did not complete")
}
