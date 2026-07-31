//! Non-Maximum Suppression (NMS) — filters overlapping detections.
//!
//! Standard greedy NMS: sort by confidence, keep the highest, remove others
//! with IoU > threshold, repeat.

use common::Detection;

/// Run NMS on a list of detections.
///
/// `iou_threshold` is the IoU above which two detections are considered
/// duplicates. Typical value: 0.45.
pub fn non_max_suppression(detections: &mut Vec<Detection>, iou_threshold: f32) {
    if detections.is_empty() {
        return;
    }
    // Sort by confidence descending
    detections.sort_by(|a, b| {
        b.confidence
            .partial_cmp(&a.confidence)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut keep = Vec::with_capacity(detections.len());
    let mut suppressed = vec![false; detections.len()];

    for i in 0..detections.len() {
        if suppressed[i] {
            continue;
        }
        keep.push(detections[i].clone());
        for j in (i + 1)..detections.len() {
            if suppressed[j] {
                continue;
            }
            let iou = detections[i].bbox.iou(&detections[j].bbox);
            if iou > iou_threshold {
                suppressed[j] = true;
            }
        }
    }

    *detections = keep;
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use common::BoundingBox;

    fn det(x: u32, y: u32, w: u32, h: u32, conf: f32) -> Detection {
        Detection {
            bbox: BoundingBox {
                x,
                y,
                width: w,
                height: h,
            },
            class: "person".to_string(),
            class_id: 0,
            confidence: conf,
            frame_seq: 1,
            detected_at: Utc::now(),
        }
    }

    #[test]
    fn nms_keeps_highest_confidence() {
        let mut dets = vec![
            det(100, 100, 50, 50, 0.9),
            det(105, 105, 50, 50, 0.7), // overlaps heavily with first
            det(500, 500, 50, 50, 0.8), // disjoint
        ];
        non_max_suppression(&mut dets, 0.45);
        assert_eq!(dets.len(), 2);
        assert_eq!(dets[0].confidence, 0.9);
        assert_eq!(dets[1].confidence, 0.8);
    }

    #[test]
    fn nms_empty_input() {
        let mut dets: Vec<Detection> = vec![];
        non_max_suppression(&mut dets, 0.45);
        assert!(dets.is_empty());
    }

    #[test]
    fn nms_disjoint_detections_all_kept() {
        let mut dets = vec![
            det(0, 0, 50, 50, 0.9),
            det(200, 200, 50, 50, 0.8),
            det(400, 400, 50, 50, 0.7),
        ];
        non_max_suppression(&mut dets, 0.45);
        assert_eq!(dets.len(), 3);
    }
}
