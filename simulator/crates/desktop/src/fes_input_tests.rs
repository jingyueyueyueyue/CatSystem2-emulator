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
    while driver.initialization_pending() {
        driver.advance_initialization(&mut shared).unwrap();
    }
    (driver, shared)
}

fn notify_left_press(driver: &mut FesScriptDriver, shared: &mut ExpressionState) {
    let scope = driver.begin_media_notification_scope();
    driver
        .on_input(FesDriverInput {
            code: 2,
            target: Some(FesDriverTarget {
                name: b"start_button".to_vec(),
                bank: 0,
                default_group: None,
                value: 17,
            }),
            event: Some(b"PUSH_L".to_vec()),
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
fn routed_click_reaches_next_title_poll_only_once() {
    let (mut driver, mut shared) = driver(&[b"if (_CLICK_L_==17)", b"exit 42", b"endif"]);
    assert!(matches!(
        driver.advance(10, &mut shared, 32).unwrap(),
        FesDriverEffect::Frame
    ));
    notify_left_press(&mut driver, &mut shared);
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
fn routed_click_reaches_wait_cancellation() {
    let (mut driver, mut shared) = driver(&[b"wait 100", b"exit 42"]);
    assert!(matches!(
        driver.advance(10, &mut shared, 32).unwrap(),
        FesDriverEffect::Waiting { .. }
    ));
    notify_left_press(&mut driver, &mut shared);
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
        assert!(matches!(driver.advance_system_notification(10, &mut shared, 2, Some(block))?,
            FesDriverEffect::SystemBlock { name, complete: false } if name == block));
        let mut tails = 0;
        let mut completed = false;
        for _ in 0..32 {
            match driver.advance_system_notification(10, &mut shared, 2, Some(block))? {
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
