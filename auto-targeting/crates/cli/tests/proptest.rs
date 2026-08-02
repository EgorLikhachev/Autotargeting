//! Property-based tests — использование proptest для поиска edge cases.
//!
//! proptest генерирует случайные входные данные и проверяет, что
//! определённые свойства выполняются для всех комбинаций.
//!
//! Запуск:
//!   cargo test -p target-tracker --test proptest -- --ignored
//!   cargo test -p cv-inference --test proptest -- --ignored

#![cfg(test)]

use proptest::prelude::*;

// ============================================================
// Kalman filter properties
// ============================================================

proptest! {
    /// Property: Kalman predict никогда не должен паниковать
    /// для любого reasonable dt > 0.
    #[test]
    fn kalman_predict_never_panics(dt in 0.0001f32..10.0) {
        let mut kf = target_tracker::KalmanFilter2D::new(100.0, 200.0);
        kf.predict(dt);
    }

    /// Property: Kalman update всегда сходится к наблюдаемой позиции
    /// при достаточном количестве итераций.
    #[test]
    fn kalman_converges_to_observation(
        initial_x in -1000.0f32..1000.0,
        initial_y in -1000.0f32..1000.0,
        target_x in -1000.0f32..1000.0,
        target_y in -1000.0f32..1000.0,
        iterations in 50usize..200,
    ) {
        let mut kf = target_tracker::KalmanFilter2D::new(initial_x, initial_y);
        let dt = 0.033; // 30 FPS

        for _ in 0..iterations {
            kf.predict(dt);
            kf.update(target_x, target_y, dt);
        }

        let (final_x, final_y) = kf.position();
        prop_assert!((final_x - target_x).abs() < 50.0, "x not converged: {} vs {}", final_x, target_x);
        prop_assert!((final_y - target_y).abs() < 50.0, "y not converged: {} vs {}", final_y, target_y);
    }

    /// Property: Bounding box IoU всегда в [0, 1]
    #[test]
    fn iou_always_in_range(
        x1 in 0u32..1000, y1 in 0u32..1000, w1 in 1u32..200, h1 in 1u32..200,
        x2 in 0u32..1000, y2 in 0u32..1000, w2 in 1u32..200, h2 in 1u32..200,
    ) {
        let a = common::BoundingBox { x: x1, y: y1, width: w1, height: h1 };
        let b = common::BoundingBox { x: x2, y: y2, width: w2, height: h2 };
        let iou = a.iou(&b);
        prop_assert!((0.0..=1.0).contains(&iou), "IoU out of range: {}", iou);
    }

    /// Property: IoU симметричен — iou(a, b) == iou(b, a)
    #[test]
    fn iou_symmetric(
        x1 in 0u32..500, y1 in 0u32..500, w1 in 1u32..100, h1 in 1u32..100,
        x2 in 0u32..500, y2 in 0u32..500, w2 in 1u32..100, h2 in 1u32..100,
    ) {
        let a = common::BoundingBox { x: x1, y: y1, width: w1, height: h1 };
        let b = common::BoundingBox { x: x2, y: y2, width: w2, height: h2 };
        let iou_ab = a.iou(&b);
        let iou_ba = b.iou(&a);
        prop_assert!((iou_ab - iou_ba).abs() < 1e-6, "IoU not symmetric: {} vs {}", iou_ab, iou_ba);
    }

    /// Property: IoU identical boxes == 1.0
    #[test]
    fn iou_identical_is_one(
        x in 0u32..1000, y in 0u32..1000, w in 1u32..200, h in 1u32..200,
    ) {
        let a = common::BoundingBox { x, y, width: w, height: h };
        let iou = a.iou(&a);
        prop_assert!((iou - 1.0).abs() < 1e-6, "IoU of identical boxes: {}", iou);
    }
}

// ============================================================
// NMS properties
// ============================================================

proptest! {
    /// Property: NMS никогда не удаляет disjoint detections.
    #[test]
    fn nms_keeps_all_disjoint(
        n in 1usize..20,
        spacing in 100u32..500,
    ) {
        let mut dets: Vec<common::Detection> = (0..n)
            .map(|i| common::Detection {
                bbox: common::BoundingBox {
                    x: i as u32 * spacing,
                    y: 0,
                    width: 50,
                    height: 50,
                },
                class: "person".to_string(),
                class_id: 0,
                confidence: 0.9,
                frame_seq: 0,
                detected_at: chrono::Utc::now(),
            })
            .collect();

        let original_count = dets.len();
        cv_inference::non_max_suppression(&mut dets, 0.45);
        prop_assert_eq!(dets.len(), original_count, "NMS removed disjoint detections");
    }

    /// Property: NMS с порогом 0.0 удаляет все кроме highest-confidence
    #[test]
    fn nms_threshold_zero_keeps_one(
        n in 2usize..10,
    ) {
        let mut dets: Vec<common::Detection> = (0..n)
            .map(|i| common::Detection {
                bbox: common::BoundingBox {
                    x: 100,
                    y: 100,
                    width: 50,
                    height: 50,
                },
                class: "person".to_string(),
                class_id: 0,
                confidence: 1.0 - i as f32 * 0.1,
                frame_seq: 0,
                detected_at: chrono::Utc::now(),
            })
            .collect();

        cv_inference::non_max_suppression(&mut dets, 0.0);
        prop_assert_eq!(dets.len(), 1, "NMS with threshold 0 should keep 1");
        prop_assert!((dets[0].confidence - 1.0).abs() < 1e-6, "should keep highest confidence");
    }
}

// ============================================================
// State machine properties
// ============================================================

proptest! {
    /// Property: ABORT всегда доступен из любого состояния
    #[test]
    fn abort_always_reachable(
        // Генерируем любое state как u8
        state_idx in 0u8..9,
    ) {
        let state = match state_idx {
            0 => common::SystemState::Idle,
            1 => common::SystemState::Armed,
            2 => common::SystemState::Scanning,
            3 => common::SystemState::TargetSelected,
            4 => common::SystemState::Tracking,
            5 => common::SystemState::TrackingDegraded,
            6 => common::SystemState::Lost,
            7 => common::SystemState::Rth,
            _ => common::SystemState::Abort,
        };

        let allowed = commander::state_machine::is_transition_allowed(state, common::SystemState::Abort);
        prop_assert!(allowed, "ABORT should be reachable from {:?}", state);
    }

    /// Property: force_transition всегда работает (бypass проверки)
    #[test]
    fn force_transition_always_works(
        from_idx in 0u8..9,
        to_idx in 0u8..9,
    ) {
        let from = match from_idx {
            0 => common::SystemState::Idle,
            1 => common::SystemState::Armed,
            2 => common::SystemState::Scanning,
            3 => common::SystemState::TargetSelected,
            4 => common::SystemState::Tracking,
            5 => common::SystemState::TrackingDegraded,
            6 => common::SystemState::Lost,
            7 => common::SystemState::Rth,
            _ => common::SystemState::Abort,
        };
        let to = match to_idx {
            0 => common::SystemState::Idle,
            1 => common::SystemState::Armed,
            2 => common::SystemState::Scanning,
            3 => common::SystemState::TargetSelected,
            4 => common::SystemState::Tracking,
            5 => common::SystemState::TrackingDegraded,
            6 => common::SystemState::Lost,
            7 => common::SystemState::Rth,
            _ => common::SystemState::Abort,
        };

        let mut sm = commander::StateMachine::new(from);
        sm.force_transition(to);
        prop_assert_eq!(sm.state(), to);
    }
}

// ============================================================
// Anti-loop properties
// ============================================================

proptest! {
    /// Property: Anti-loop guard никогда не паникует
    #[test]
    fn anti_loop_never_panics(
        yaw in -100.0f32..100.0,
        pitch in -100.0f32..100.0,
        ox in -1.0f32..1.0,
        oy in -1.0f32..1.0,
    ) {
        let guard = commander::AntiLoopGuard::new(common::CommanderConfig::default());
        let cmd = commander::CorrectionCommand {
            yaw_rate_dps: yaw,
            pitch_rate_dps: pitch,
            offset_x: ox,
            offset_y: oy,
            generated_at: std::time::Instant::now(),
        };
        // Should not panic
        let _ = guard.process(cmd);
    }

    /// Property: Steady direction (same sign) never triggers oscillation
    #[test]
    fn steady_direction_no_oscillation(
        yaw in 0.1f32..50.0,
        ox in 0.1f32..0.9,
        iterations in 10usize..100,
    ) {
        let guard = commander::AntiLoopGuard::new(common::CommanderConfig::default());

        for _ in 0..iterations {
            let cmd = commander::CorrectionCommand {
                yaw_rate_dps: yaw,
                pitch_rate_dps: 0.0,
                offset_x: ox,
                offset_y: 0.1,
                generated_at: std::time::Instant::now(),
            };
            let decision = guard.process(cmd);
            prop_assert!(
                !matches!(decision, commander::GuardDecision::Abort),
                "steady direction should not trigger abort"
            );
        }
    }
}
