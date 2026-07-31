//! PID-контроллер с anti-windup для управления наведением.
//!
//! Заменяет crude proportional control в Commander::send_correction_to_fc().
//! PID даёт плавную сходимость без перерегулирования и осцилляций.
//!
//! ## Особенности
//!
//! - **Anti-windup:** ограничивает интегральную составляющую, чтобы избежать
//!   "накопления ошибки" при насыщении выхода.
//! - **Derivative filtering:** экспоненциальное сглаживание D-члена для
//!   подавления высокочастотного шума.
//! - **Output limiting:** выход ограничен ±`max_output`.
//! - **Deadband:** если |error| < `deadband`, выход = 0 (предотвращает jitter).
//!
//! ## Использование
//!
//! ```ignore
//! use commander::PidController;
//!
//! let mut pid = PidController::new(2.0, 0.5, 0.1)  // Kp, Ki, Kd
//!     .with_limits(-30.0, 30.0)    // max yaw rate (dps)
//!     .with_deadband(0.02)         // 2% of frame
//!     .with_anti_windup(15.0);     // integral clamp
//!
//! let output = pid.update(target_offset, dt);
//! ```

use serde::{Deserialize, Serialize};

/// Конфигурация PID-контроллера.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PidConfig {
    /// Proportional gain.
    pub kp: f32,
    /// Integral gain.
    pub ki: f32,
    /// Derivative gain.
    pub kd: f32,
    /// Максимальное значение выхода (output clamping).
    pub max_output: f32,
    /// Минимальное значение выхода.
    pub min_output: f32,
    /// Зона нечувствительности (если |error| < deadband, выход = 0).
    pub deadband: f32,
    /// Anti-windup: максимальное значение интеграла.
    pub integral_limit: f32,
    /// Коэффициент сглаживания D-члена (0 = нет сглаживания, 1 = полное).
    /// Чем больше, тем сильнее фильтрация шума.
    pub derivative_filter: f32,
}

impl Default for PidConfig {
    fn default() -> Self {
        Self {
            kp: 2.0,
            ki: 0.5,
            kd: 0.1,
            max_output: 30.0,
            min_output: -30.0,
            deadband: 0.02,
            integral_limit: 15.0,
            derivative_filter: 0.3,
        }
    }
}

/// PID-контроллер с anti-windup и derivative filtering.
pub struct PidController {
    config: PidConfig,
    /// Накопленная интегральная составляющая.
    integral: f32,
    /// Предыдущее значение ошибки (для D-члена).
    prev_error: f32,
    /// Сглаженное значение производной.
    filtered_derivative: f32,
    /// Инициализирован ли контроллер (есть ли prev_error).
    initialized: bool,
}

impl PidController {
    pub fn new(kp: f32, ki: f32, kd: f32) -> Self {
        Self {
            config: PidConfig {
                kp,
                ki,
                kd,
                ..Default::default()
            },
            integral: 0.0,
            prev_error: 0.0,
            filtered_derivative: 0.0,
            initialized: false,
        }
    }

    pub fn with_config(config: PidConfig) -> Self {
        Self {
            config,
            integral: 0.0,
            prev_error: 0.0,
            filtered_derivative: 0.0,
            initialized: false,
        }
    }

    pub fn with_limits(mut self, min: f32, max: f32) -> Self {
        self.config.min_output = min;
        self.config.max_output = max;
        self
    }

    pub fn with_deadband(mut self, deadband: f32) -> Self {
        self.config.deadband = deadband;
        self
    }

    pub fn with_anti_windup(mut self, limit: f32) -> Self {
        self.config.integral_limit = limit;
        self
    }

    /// Обновить состояние PID и вернуть управляющий сигнал.
    ///
    /// Параметры:
    /// - `error`: текущая ошибка (target - actual), например offset цели от центра кадра.
    /// - `dt`: время с прошлого обновления, секунды.
    pub fn update(&mut self, error: f32, dt: f32) -> f32 {
        if dt <= 0.0 {
            return 0.0;
        }

        // Deadband: если ошибка мала, не генерируем управляющий сигнал
        if error.abs() < self.config.deadband {
            // Сбрасываем интеграл в deadband — предотвращаем накопление
            self.integral *= 0.9;
            return 0.0;
        }

        // P: proportional
        let p = self.config.kp * error;

        // I: integral с anti-windup
        self.integral += error * dt;
        // Clamp integral
        self.integral = self
            .integral
            .clamp(-self.config.integral_limit, self.config.integral_limit);
        let i = self.config.ki * self.integral;

        // D: derivative с фильтрацией
        let derivative = if self.initialized {
            (error - self.prev_error) / dt
        } else {
            0.0
        };

        // Exponential smoothing: filtered = α * raw + (1-α) * prev_filtered
        let alpha = 1.0 - self.config.derivative_filter;
        self.filtered_derivative = alpha * derivative + (1.0 - alpha) * self.filtered_derivative;
        let d = self.config.kd * self.filtered_derivative;

        // Сохранить для следующего шага
        self.prev_error = error;
        self.initialized = true;

        // Сумма
        let mut output = p + i + d;

        // Anti-windup: если выход насыщен, "откатываем" интеграл
        // чтобы он не продолжал накапливаться
        let clamped = output.clamp(self.config.min_output, self.config.max_output);
        if clamped != output {
            // Back-calculate integral (conditional integration)
            let saturation = output - clamped;
            // Уменьшаем интеграл на величину насыщения (с коэффициентом)
            self.integral -= saturation / self.config.ki.max(1e-6);
            output = clamped;
        }

        output
    }

    /// Сбросить состояние контроллера (integral, derivative, prev_error).
    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.prev_error = 0.0;
        self.filtered_derivative = 0.0;
        self.initialized = false;
    }

    /// Получить текущее значение интеграла (для диагностики).
    pub fn integral(&self) -> f32 {
        self.integral
    }

    /// Получить конфигурацию.
    pub fn config(&self) -> &PidConfig {
        &self.config
    }
}

/// Пара PID-контроллеров для управления по двум осям (yaw + pitch).
pub struct PidPair {
    pub yaw: PidController,
    pub pitch: PidController,
}

impl PidPair {
    pub fn new(yaw: PidController, pitch: PidController) -> Self {
        Self { yaw, pitch }
    }

    /// Обновить оба контроллера.
    /// Возвращает (yaw_rate, pitch_rate).
    pub fn update(&mut self, error_x: f32, error_y: f32, dt: f32) -> (f32, f32) {
        let yaw_rate = self.yaw.update(error_x, dt);
        let pitch_rate = self.pitch.update(error_y, dt);
        (yaw_rate, pitch_rate)
    }

    pub fn reset(&mut self) {
        self.yaw.reset();
        self.pitch.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn construction_with_gains() {
        let pid = PidController::new(2.0, 0.5, 0.1);
        assert_eq!(pid.config.kp, 2.0);
        assert_eq!(pid.config.ki, 0.5);
        assert_eq!(pid.config.kd, 0.1);
    }

    #[test]
    fn construction_with_builder() {
        let pid = PidController::new(2.0, 0.5, 0.1)
            .with_limits(-30.0, 30.0)
            .with_deadband(0.05)
            .with_anti_windup(15.0);
        assert_eq!(pid.config.min_output, -30.0);
        assert_eq!(pid.config.max_output, 30.0);
        assert_eq!(pid.config.deadband, 0.05);
        assert_eq!(pid.config.integral_limit, 15.0);
    }

    #[test]
    fn zero_error_gives_zero_output() {
        let mut pid = PidController::new(2.0, 0.5, 0.1);
        let output = pid.update(0.0, 0.1);
        assert_eq!(output, 0.0);
    }

    #[test]
    fn deadband_suppresses_small_errors() {
        let mut pid = PidController::new(2.0, 0.5, 0.1).with_deadband(0.05);
        // Error within deadband → output = 0
        let output = pid.update(0.03, 0.1);
        assert_eq!(output, 0.0);
    }

    #[test]
    fn deadband_does_not_suppress_large_errors() {
        let mut pid = PidController::new(2.0, 0.5, 0.1).with_deadband(0.05);
        // Error outside deadband → output > 0
        let output = pid.update(0.2, 0.1);
        assert!(output > 0.0);
    }

    #[test]
    fn proportional_only() {
        let mut pid = PidController::new(2.0, 0.0, 0.0).with_deadband(0.0);
        let output = pid.update(0.5, 0.1);
        // P = Kp * error = 2.0 * 0.5 = 1.0
        assert!((output - 1.0).abs() < 1e-6);
    }

    #[test]
    fn output_clamped_to_max() {
        let mut pid = PidController::new(100.0, 0.0, 0.0)
            .with_limits(-10.0, 10.0)
            .with_deadband(0.0);
        // P = 100 * 1.0 = 100, но max = 10
        let output = pid.update(1.0, 0.1);
        assert!((output - 10.0).abs() < 1e-6);
    }

    #[test]
    fn output_clamped_to_min() {
        let mut pid = PidController::new(100.0, 0.0, 0.0)
            .with_limits(-10.0, 10.0)
            .with_deadband(0.0);
        // P = 100 * (-1.0) = -100, но min = -10
        let output = pid.update(-1.0, 0.1);
        assert!((output + 10.0).abs() < 1e-6);
    }

    #[test]
    fn integral_accumulates() {
        let mut pid = PidController::new(0.0, 1.0, 0.0)
            .with_limits(-100.0, 100.0)
            .with_deadband(0.0)
            .with_anti_windup(100.0);

        // Первый шаг: integral = 0.1 * 0.1 = 0.01
        pid.update(0.1, 0.1);
        assert!((pid.integral() - 0.01).abs() < 1e-6);

        // Второй шаг: integral = 0.01 + 0.1 * 0.1 = 0.02
        pid.update(0.1, 0.1);
        assert!((pid.integral() - 0.02).abs() < 1e-6);
    }

    #[test]
    fn integral_anti_windup_clamps() {
        let mut pid = PidController::new(0.0, 1.0, 0.0)
            .with_limits(-100.0, 100.0)
            .with_deadband(0.0)
            .with_anti_windup(0.5); // маленький лимит

        // Накапливаем интеграл
        for _ in 0..100 {
            pid.update(1.0, 0.1);
        }

        // Integral должен быть ограничен 0.5
        assert!(pid.integral() <= 0.5 + 1e-6);
        assert!(pid.integral() >= -0.5 - 1e-6);
    }

    #[test]
    fn derivative_responds_to_rate_of_change() {
        let mut pid = PidController::new(0.0, 0.0, 1.0)
            .with_limits(-100.0, 100.0)
            .with_deadband(0.0);

        // Первый шаг: derivative = 0 (нет prev_error)
        let _ = pid.update(0.5, 0.1);

        // Второй шаг: error изменился с 0.5 до 1.0 за 0.1 сек
        // derivative = (1.0 - 0.5) / 0.1 = 5.0
        // D = Kd * derivative (с фильтрацией)
        let output = pid.update(1.0, 0.1);
        // С фильтрацией alpha=0.7: filtered = 0.7*5.0 + 0.3*0 = 3.5
        // D = 1.0 * 3.5 = 3.5
        assert!(output > 0.0, "D should be positive for increasing error");
        assert!(output < 5.0, "filtered D should be less than raw");
    }

    #[test]
    fn reset_clears_state() {
        let mut pid = PidController::new(0.0, 1.0, 0.0)
            .with_limits(-100.0, 100.0)
            .with_deadband(0.0)
            .with_anti_windup(100.0);

        // Накапливаем
        pid.update(1.0, 0.1);
        pid.update(1.0, 0.1);
        assert!(pid.integral() > 0.0);

        // Reset
        pid.reset();
        assert_eq!(pid.integral(), 0.0);
    }

    #[test]
    fn negative_error_gives_negative_output() {
        let mut pid = PidController::new(2.0, 0.0, 0.0).with_deadband(0.0);
        let output = pid.update(-0.5, 0.1);
        assert!(output < 0.0);
        assert!((output + 1.0).abs() < 1e-6);
    }

    #[test]
    fn dt_zero_returns_zero() {
        let mut pid = PidController::new(2.0, 0.5, 0.1).with_deadband(0.0);
        let output = pid.update(0.5, 0.0);
        assert_eq!(output, 0.0);
    }

    #[test]
    fn pid_pair_updates_both() {
        let mut pair = PidPair::new(
            PidController::new(2.0, 0.0, 0.0).with_deadband(0.0),
            PidController::new(1.0, 0.0, 0.0).with_deadband(0.0),
        );

        let (yaw, pitch) = pair.update(0.5, 0.3, 0.1);
        assert!((yaw - 1.0).abs() < 1e-6); // 2.0 * 0.5
        assert!((pitch - 0.3).abs() < 1e-6); // 1.0 * 0.3
    }

    /// Симуляция: цель скачет с 0 на 1, проверяем сходимость PID.
    #[test]
    fn convergence_simulation() {
        let mut pid = PidController::new(5.0, 1.0, 0.5)
            .with_limits(-10.0, 10.0)
            .with_deadband(0.01)
            .with_anti_windup(5.0);

        let dt = 0.05; // 20 Hz
        let mut actual = 0.0_f32;
        let target = 1.0_f32;

        let start = Instant::now();
        for _ in 0..200 {
            // 10 секунд
            let error = target - actual;
            let control = pid.update(error, dt);
            // Простая модель: actual += control * dt (без инерции)
            actual += control * dt;
        }
        let elapsed = start.elapsed();

        // Должны сойтись к target
        assert!(
            (actual - target).abs() < 0.1,
            "PID should converge, actual={actual}, target={target}"
        );
        println!("Converged in {elapsed:?}, final actual={actual}");
    }

    /// Симуляция: проверяем, что PID не осциллирует бесконечно.
    #[test]
    fn no_oscillation_simulation() {
        let mut pid = PidController::new(3.0, 0.5, 0.8)
            .with_limits(-10.0, 10.0)
            .with_deadband(0.02)
            .with_anti_windup(3.0);

        let dt = 0.05;
        let mut actual = 0.0_f32;
        let target = 0.5_f32;

        let mut sign_changes = 0;
        let mut prev_error = target - actual;

        for _ in 0..400 {
            // 20 секунд
            let error = target - actual;
            if error.signum() != prev_error.signum() && error.abs() > 0.01 {
                sign_changes += 1;
            }
            prev_error = error;

            let control = pid.update(error, dt);
            actual += control * dt;
        }

        // Допускаем максимум 5 смен знака (переходный процесс)
        assert!(
            sign_changes <= 5,
            "PID oscillating: {sign_changes} sign changes in 20s"
        );
    }
}
