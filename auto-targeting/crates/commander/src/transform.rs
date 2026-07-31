//! Координатный трансформ — camera frame → NED (North-East-Down).
//!
//! Сейчас в Commander::send_correction_to_fc() используется crude mapping:
//! offset_x → east, offset_y → down. Это неправильно.
//!
//! Реальный трансформ должен учитывать:
//! 1. **FOV камеры** — угол обзора определяет, насколько смещение в пикселях
//!    соответствует угловому смещению.
//! 2. **Attitude дрона** — если дрон наклонён (pitch/roll), camera frame
//!    не совпадает с body frame.
//! 3. **Расстояние до цели** — если известно, можно перевести угловое
//!    смещение в линейное (метры).
//!
//! ## Подход
//!
//! Для нашего use case (companion computer наводит дрон на цель):
//! - Цель в кадре → угловое смещение от оптической оси
//! - Угловое смещение → desired yaw rate (через PID)
//! - Yaw rate → SET_POSITION_TARGET_LOCAL_NED с yaw полем
//!
//! Это упрощённый подход — мы не пытаемся переместить дрон в конкретную
//! точку, а только поворачиваем его (yaw) так, чтобы цель была в центре кадра.

use serde::{Deserialize, Serialize};

/// Параметры камеры (для трансформа pixel → angle).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraParams {
    /// Horizontal FOV в радианах.
    pub hfov_rad: f32,
    /// Vertical FOV в радианах.
    pub vfov_rad: f32,
    /// Ширина кадра в пикселях.
    pub width: u32,
    /// Высота кадра в пикселях.
    pub height: u32,
}

impl CameraParams {
    /// Создать из градусов FOV.
    pub fn from_degrees(hfov_deg: f32, vfov_deg: f32, width: u32, height: u32) -> Self {
        Self {
            hfov_rad: hfov_deg.to_radians(),
            vfov_rad: vfov_deg.to_radians(),
            width,
            height,
        }
    }

    /// Ширина одного пикселя в радианах (по горизонтали).
    pub fn radians_per_pixel_x(&self) -> f32 {
        self.hfov_rad / self.width as f32
    }

    /// Высота одного пикселя в радианах (по вертикали).
    pub fn radians_per_pixel_y(&self) -> f32 {
        self.vfov_rad / self.height as f32
    }
}

impl Default for CameraParams {
    fn default() -> Self {
        // Типичные значения для USB-камеры 1280x720 с ~60° HFOV
        Self::from_degrees(60.0, 34.0, 1280, 720)
    }
}

/// Смещение цели в кадре (нормализованное, [-1.0, 1.0]).
///
/// - `x`: -1 = левый край, 0 = центр, +1 = правый край
/// - `y`: -1 = верхний край, 0 = центр, +1 = нижний край
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameOffset {
    pub x: f32,
    pub y: f32,
}

impl FrameOffset {
    pub fn new(x: f32, y: f32) -> Self {
        Self { x, y }
    }

    /// Создать из pixel-координат bbox-центра и размеров кадра.
    pub fn from_pixel(cx: f32, cy: f32, width: u32, height: u32) -> Self {
        let w = width as f32;
        let h = height as f32;
        Self {
            x: (cx - w / 2.0) / (w / 2.0),
            y: (cy - h / 2.0) / (h / 2.0),
        }
    }

    /// Magnitude (длина вектора смещения).
    pub fn magnitude(&self) -> f32 {
        (self.x * self.x + self.y * self.y).sqrt()
    }
}

/// Результат трансформа — желаемые угловые скорости (rad/s).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AngularRates {
    /// Yaw rate (rad/s). Положительное = по часовой.
    pub yaw_rate: f32,
    /// Pitch rate (rad/s). Положительное = нос вверх.
    pub pitch_rate: f32,
}

/// Результат трансформа — желаемая yaw позиция (для SET_POSITION_TARGET_LOCAL_NED).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NedTarget {
    /// Желаемый yaw в радианах (относительно текущего).
    pub yaw: f32,
    /// Желаемый yaw rate в rad/s.
    pub yaw_rate: f32,
}

/// Трансформ из camera frame (offset в кадре) в угловые команды.
///
/// Учитывает FOV камеры для перевода pixel-offset в угловое смещение.
/// Не учитывает attitude дрона (упрощение для Phase 5 — добавится в Phase 6).
pub struct CameraToAngular {
    camera: CameraParams,
}

impl CameraToAngular {
    pub fn new(camera: CameraParams) -> Self {
        Self { camera }
    }

    /// Трансформ: frame offset → угловое смещение от оптической оси (rad).
    ///
    /// offset.x = 1.0 (правый край кадра) → +hfov/2
    /// offset.x = -1.0 (левый край) → -hfov/2
    pub fn offset_to_angle(&self, offset: FrameOffset) -> (f32, f32) {
        let yaw_angle = offset.x * (self.camera.hfov_rad / 2.0);
        let pitch_angle = offset.y * (self.camera.vfov_rad / 2.0);
        (yaw_angle, pitch_angle)
    }

    /// Трансформ: pixel-координаты → угловое смещение (rad).
    pub fn pixel_to_angle(&self, cx: f32, cy: f32) -> (f32, f32) {
        let offset = FrameOffset::from_pixel(cx, cy, self.camera.width, self.camera.height);
        self.offset_to_angle(offset)
    }

    /// Трансформ: frame offset → desired yaw rate (rad/s).
    ///
    /// Упрощённая модель: yaw_rate = K * yaw_angle, где K — коэффициент.
    /// В реальном использовании здесь должен быть PID-контроллер.
    pub fn offset_to_yaw_rate(&self, offset: FrameOffset, gain: f32) -> f32 {
        let (yaw_angle, _) = self.offset_to_angle(offset);
        yaw_angle * gain
    }

    /// Полный трансформ: frame offset → NedTarget для MAVLink.
    ///
    /// Возвращает (yaw, yaw_rate) — желаемую yaw позицию (относительно
    /// текущего attitude) и yaw rate.
    pub fn offset_to_ned_target(&self, offset: FrameOffset) -> NedTarget {
        let (yaw_angle, _) = self.offset_to_angle(offset);
        // yaw — это желаемое угловое смещение от текущего
        // (FC сам удержит текущий yaw + это смещение)
        NedTarget {
            yaw: yaw_angle,
            yaw_rate: 0.0, // используем position, не rate
        }
    }

    /// Трансформ с учётом attitude дрона (расширенная версия).
    ///
    /// Если дрон наклонён, camera frame не совпадает с body frame.
    /// Этот метод корректирует offset с учётом текущего pitch/roll.
    ///
    /// Для упрощения учитываем только pitch (дрон смотрит вверх/вниз).
    /// Roll и yaw камеры требуют полной матрицы поворота (Phase 6).
    pub fn offset_to_ned_target_with_attitude(
        &self,
        offset: FrameOffset,
        drone_pitch: f32,
        drone_yaw: f32,
    ) -> NedTarget {
        let (yaw_angle, pitch_angle) = self.offset_to_angle(offset);

        // Корректируем yaw на текущий yaw дрона (чтобы команда была в NED, не body)
        // В NED yaw = 0 это North, positive = East (по часовой)
        let ned_yaw = drone_yaw + yaw_angle;

        // Pitch влияет на то, какую часть кадра видит камера
        // Если дрон смотрит вверх (pitch > 0), нижняя половина кадра видит меньше
        // Для упрощения — игнорируем pitch_angle в NED target
        // (он управляется стабилизацией FC, не нами)
        let _ = (pitch_angle, drone_pitch);

        NedTarget {
            yaw: ned_yaw,
            yaw_rate: 0.0,
        }
    }

    /// Получить параметры камеры.
    pub fn camera(&self) -> &CameraParams {
        &self.camera
    }
}

/// Утилита: конвертировать yaw rate (rad/s) в градусы/сек.
pub fn rad_per_sec_to_deg_per_sec(rad: f32) -> f32 {
    rad.to_degrees()
}

/// Утилита: конвертировать градусы/сек в rad/s.
pub fn deg_per_sec_to_rad_per_sec(deg: f32) -> f32 {
    deg.to_radians()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camera_params_default() {
        let cam = CameraParams::default();
        assert_eq!(cam.width, 1280);
        assert_eq!(cam.height, 720);
        assert!((cam.hfov_rad - 60.0_f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn radians_per_pixel() {
        let cam = CameraParams::from_degrees(60.0, 34.0, 1280, 720);
        let rpp_x = cam.radians_per_pixel_x();
        // 60° = 1.047 rad, / 1280 px = 0.000818 rad/px
        assert!((rpp_x - 60.0_f32.to_radians() / 1280.0).abs() < 1e-6);
    }

    #[test]
    fn frame_offset_from_pixel_center() {
        let offset = FrameOffset::from_pixel(640.0, 360.0, 1280, 720);
        assert!((offset.x).abs() < 1e-6);
        assert!((offset.y).abs() < 1e-6);
    }

    #[test]
    fn frame_offset_from_pixel_corner() {
        let offset = FrameOffset::from_pixel(0.0, 0.0, 1280, 720);
        assert!((offset.x + 1.0).abs() < 1e-6); // left edge = -1
        assert!((offset.y + 1.0).abs() < 1e-6); // top edge = -1
    }

    #[test]
    fn frame_offset_from_pixel_right_bottom() {
        let offset = FrameOffset::from_pixel(1280.0, 720.0, 1280, 720);
        assert!((offset.x - 1.0).abs() < 1e-6); // right edge = +1
        assert!((offset.y - 1.0).abs() < 1e-6); // bottom edge = +1
    }

    #[test]
    fn frame_offset_magnitude() {
        let offset = FrameOffset::new(0.0, 0.0);
        assert!((offset.magnitude()).abs() < 1e-6);

        let offset = FrameOffset::new(3.0, 4.0);
        assert!((offset.magnitude() - 5.0).abs() < 1e-6);
    }

    #[test]
    fn offset_to_angle_center() {
        let cam = CameraParams::from_degrees(60.0, 34.0, 1280, 720);
        let transform = CameraToAngular::new(cam.clone());
        let (yaw, pitch) = transform.offset_to_angle(FrameOffset::new(0.0, 0.0));
        assert!(yaw.abs() < 1e-6);
        assert!(pitch.abs() < 1e-6);
    }

    #[test]
    fn offset_to_angle_edge() {
        let cam = CameraParams::from_degrees(60.0, 34.0, 1280, 720);
        let transform = CameraToAngular::new(cam.clone());
        let (yaw, _) = transform.offset_to_angle(FrameOffset::new(1.0, 0.0));
        // Right edge → +hfov/2 = +30°
        assert!((yaw - 30.0_f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn offset_to_angle_left_edge() {
        let cam = CameraParams::from_degrees(60.0, 34.0, 1280, 720);
        let transform = CameraToAngular::new(cam.clone());
        let (yaw, _) = transform.offset_to_angle(FrameOffset::new(-1.0, 0.0));
        // Left edge → -hfov/2 = -30°
        assert!((yaw + 30.0_f32.to_radians()).abs() < 1e-6);
    }

    #[test]
    fn offset_to_yaw_rate_positive() {
        let cam = CameraParams::default();
        let transform = CameraToAngular::new(cam.clone());
        let rate = transform.offset_to_yaw_rate(FrameOffset::new(0.5, 0.0), 1.0);
        assert!(rate > 0.0);
    }

    #[test]
    fn offset_to_yaw_rate_negative() {
        let cam = CameraParams::default();
        let transform = CameraToAngular::new(cam.clone());
        let rate = transform.offset_to_yaw_rate(FrameOffset::new(-0.5, 0.0), 1.0);
        assert!(rate < 0.0);
    }

    #[test]
    fn offset_to_ned_target() {
        let cam = CameraParams::default();
        let transform = CameraToAngular::new(cam.clone());
        let target = transform.offset_to_ned_target(FrameOffset::new(0.5, 0.0));
        assert!(target.yaw > 0.0); // positive offset → positive yaw
        assert_eq!(target.yaw_rate, 0.0); // we use position, not rate
    }

    #[test]
    fn pixel_to_angle() {
        let cam = CameraParams::from_degrees(60.0, 34.0, 1280, 720);
        let transform = CameraToAngular::new(cam.clone());
        // Pixel at right edge
        let (yaw, _) = transform.pixel_to_angle(1280.0, 360.0);
        assert!((yaw - 30.0_f32.to_radians()).abs() < 1e-3);
    }

    #[test]
    fn offset_to_ned_with_attitude() {
        let cam = CameraParams::default();
        let transform = CameraToAngular::new(cam.clone());
        let drone_yaw = 1.0; // ~57°
        let target = transform.offset_to_ned_target_with_attitude(
            FrameOffset::new(0.5, 0.0),
            0.0,
            drone_yaw,
        );
        // ned_yaw = drone_yaw + yaw_angle
        // yaw_angle = 0.5 * (hfov/2) = 0.5 * 30° = 15° = 0.2618 rad
        let expected_yaw = drone_yaw + 0.5 * (cam.hfov_rad / 2.0);
        assert!((target.yaw - expected_yaw).abs() < 1e-6);
    }

    #[test]
    fn deg_rad_conversion() {
        assert!((rad_per_sec_to_deg_per_sec(std::f32::consts::PI) - 180.0).abs() < 1e-4);
        assert!((deg_per_sec_to_rad_per_sec(180.0) - std::f32::consts::PI).abs() < 1e-4);
    }

    /// Интеграционный тест: цель в правом верхнем углу → positive yaw, negative pitch.
    #[test]
    fn integration_target_top_right() {
        let cam = CameraParams::from_degrees(60.0, 34.0, 1280, 720);
        let transform = CameraToAngular::new(cam.clone());

        // Цель в правом верхнем углу: offset = (+0.5, -0.5)
        let offset = FrameOffset::new(0.5, -0.5);
        let (yaw, pitch) = transform.offset_to_angle(offset);

        // Yaw должен быть положительным (цель справа)
        assert!(yaw > 0.0);
        // Pitch должен быть отрицательным (цель сверху)
        assert!(pitch < 0.0);
    }

    /// Интеграционный тест: цель в центре → zero angles.
    #[test]
    fn integration_target_center() {
        let cam = CameraParams::default();
        let transform = CameraToAngular::new(cam.clone());

        let offset = FrameOffset::new(0.0, 0.0);
        let (yaw, pitch) = transform.offset_to_angle(offset);
        assert!(yaw.abs() < 1e-6);
        assert!(pitch.abs() < 1e-6);
    }
}
