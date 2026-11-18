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
