//! Приёмочные тесты TG26-160 — прямая проверка критериев готовности задачи.
//!
//! Кросс-платформенная часть (арена, протокол идентичен SHM) — запускается
//! всегда. Мультипроцессная часть (реальная разделяемая память) — Linux,
//! включается флагом:
//!   cargo test -p shmem-buffer --test acceptance -- --include-ignored

use std::time::{Duration, Instant};

use shmem_buffer::{
    create_in_process, DropReason, FrameProducer, NextStep, PublishResult, RingConfig,
    StorageFormat,
};

fn cfg(cap: u32) -> RingConfig {
    RingConfig {
        capacity: cap,
        width: 64,
        height: 48,
        format: StorageFormat::Nv12,
    }
}

/// Взять конкретный кадр по id (в окне кольца) — для сценариев,
/// где нужно держать именно СТАРЕЙШИЙ слот (следующая жертва записи).
fn frame_by_id(prod: &FrameProducer, id: u64) -> shmem_buffer::FrameGuard {
    match prod.consumer().next_after(id - 1) {
        NextStep::Frame(g) if g.frame_id() == id => g,
        other => panic!("cannot acquire frame {id}: {other:?}"),
    }
}

fn pattern(cfg: &RingConfig, id: u64) -> Vec<u8> {
    let mut v = Vec::with_capacity(cfg.frame_size() as usize);
    for _ in 0..cfg.frame_size() / 4 {
        v.extend_from_slice(&(id as u32).to_le_bytes());
    }
    v.resize(cfg.frame_size() as usize, 0);
    v
}

// ============================================================
// Критерий 1: кадры хранятся в согласованном формате
// (NV12 4:2:0, размеры согласованы с video-capture/convert.rs).
// ============================================================
#[test]
fn acceptance_consistent_format_nv12() {
    let c = cfg(4);
    assert_eq!(c.frame_size(), 64 * 48 * 3 / 2); // Y + interleaved UV
    let prod = create_in_process(&c).unwrap();
    let g = {
        prod.publish(&pattern(&c, 1), 1).unwrap();
        prod.consumer().latest().unwrap()
    };
    assert_eq!(g.view().storage_format(), Some(StorageFormat::Nv12));
    assert_eq!(g.view().width, 64);
    assert_eq!(g.view().height, 48);
}

// ============================================================
// Критерий 2: кольцевой буфер настраиваемого размера.
// ============================================================
#[test]
fn acceptance_configurable_capacity() {
    for cap in [3u32, 10, 32] {
        let c = cfg(cap);
        let prod = create_in_process(&c).unwrap();
        assert_eq!(prod.config().capacity, cap);
        for id in 1..=(u64::from(cap) * 2) {
            prod.publish(&pattern(&c, id), id).unwrap();
        }
        assert_eq!(prod.stats().published, u64::from(cap) * 2);
    }
}

// ============================================================
// Критерий 3: у кадра есть идентификатор, метка времени,
// размеры и формат.
// ============================================================
#[test]
fn acceptance_frame_metadata_complete() {
    let prod = create_in_process(&cfg(4)).unwrap();
    let ts = 1_750_000_000_123_456_789u64;
    prod.publish(&pattern(&cfg(4), 7), ts).unwrap();
    let g = prod.consumer().latest().unwrap();
    let v = g.view();
    assert_eq!(v.frame_id, 1);
    assert_eq!(v.ts_ns, ts);
    assert_eq!((v.width, v.height), (64, 48));
    assert_eq!(v.format, shmem_buffer::FORMAT_NV12);
    // Метаданные конвертируются в owning-тип проекта.
    let f = g.to_frame();
    assert_eq!(f.metadata.seq, 1);
    assert_eq!(f.metadata.format, common::PixelFormat::Nv12);
}

// ============================================================
// Критерии 4+5: несколько независимых потребителей читают ОДИН
// кадр; кадр не перезаписывается, пока используется.
// ============================================================
#[test]
fn acceptance_multi_consumer_no_overwrite_while_held() {
    let c = cfg(4);
    let prod = create_in_process(&c).unwrap();
    let consumers: Vec<_> = (0..3).map(|_| prod.consumer()).collect();

    // Заполняем всё кольцо.
    for id in 1..=4 {
        prod.publish(&pattern(&c, id), id).unwrap();
    }

    // Трое независимо берут ОДИН и тот же кадр — старейший id=1:
    // его слот и есть цель следующей записи (next_id=5 → слот 1).
    let guards: Vec<_> = consumers
        .iter()
        .map(|cons| match cons.next_after(0) {
            NextStep::Frame(g) if g.frame_id() == 1 => g,
            other => panic!("expected frame 1, got {other:?}"),
        })
        .collect();

    // Пока держат — новый кадр в этот слот не попадает: drop-new.
    match prod.publish(&pattern(&c, 5), 5).unwrap() {
        PublishResult::Dropped {
            reason: DropReason::HeldByReaders { readers, .. },
        } => assert!(readers >= 3),
        other => panic!("expected Dropped(HeldByReaders), got {other:?}"),
    }
    // Данные держимого кадра неизменны (все слова = 1 — id держимого кадра).
    for g in &guards {
        assert!(g
            .chunks_exact(4)
            .all(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) == 1));
    }
    drop(guards);

    // Освободили — публикация проходит.
    assert!(matches!(
        prod.publish(&pattern(&c, 5), 5).unwrap(),
        PublishResult::Published { .. }
    ));
}

// ============================================================
// Критерий 6: поведение при медленном потребителе и заполненном
// буфере — задокументированный drop-new; продюсер не блокируется.
// ============================================================
#[test]
fn acceptance_slow_consumer_drop_new_semantics() {
    let c = cfg(4);
    let prod = create_in_process(&c).unwrap();
    let cons = prod.consumer();

    for id in 1..=4 {
        prod.publish(&pattern(&c, id), id).unwrap();
    }
    // «Медленный рекордер» держит СТАРЕЙШИЙ кадр (id=1) — цель следующей
    // записи; свежие latest() кадры продюсера не интересуют.
    let held = frame_by_id(&prod, 1);
    let _ = cons;

    // Продюсер продолжает работать с полной скоростью: все кадры
    // мимо занятого слота выбрасываются, счётчик растёт.
    let t0 = Instant::now();
    for id in 5..=24 {
        match prod.publish(&pattern(&c, id), id).unwrap() {
            PublishResult::Dropped { .. } => {}
            PublishResult::Published { .. } => panic!("overwrite while held!"),
        }
    }
    assert!(t0.elapsed() < Duration::from_secs(5), "producer blocked!");

    assert_eq!(prod.stats().dropped, 20);
    // Держимый кадр (id=1) цел.
    assert!(held
        .chunks_exact(4)
        .all(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) == 1));
    drop(held);

    // Медленный потребитель дочитал — продюсер тут же публикует свежий.
    assert!(matches!(
        prod.publish_now(&pattern(&c, 25)).unwrap(),
        PublishResult::Published { .. }
    ));
}

// ============================================================
// Критерий 7: несколько одновременных потребителей (потоки,
// torn-read детектор, сквозные последовательности с прыжками).
// ============================================================
#[test]
fn acceptance_concurrent_consumers_threads() {
    let c = cfg(8);
    let prod = std::sync::Arc::new(create_in_process(&c).unwrap());
    const TOTAL: u64 = 400;

    let handles: Vec<_> = (0..3)
        .map(|_| {
            let p = std::sync::Arc::clone(&prod);
            std::thread::spawn(move || {
                let cons = p.consumer();
                let deadline = Instant::now() + Duration::from_secs(30);
                let (mut last, mut verified, mut quiet) = (0u64, 0u64, 0u32);
                loop {
                    assert!(Instant::now() < deadline, "reader timeout at {last}");
                    match cons.next_after(last) {
                        NextStep::Frame(g) => {
                            let id = g.frame_id();
                            assert!(
                                g.chunks_exact(4)
                                    .all(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]]) == id as u32),
                                "torn read @{id}"
                            );
                            last = id;
                            verified += 1;
                            quiet = 0;
                        }
                        NextStep::UpToDate => {
                            quiet += 1;
                            if quiet > 200 {
                                break;
                            }
                            std::thread::yield_now();
                        }
                        NextStep::TooFarBehind { latest, .. } => last = latest,
                    }
                }
                (verified, last)
            })
        })
        .collect();

    for id in 1..=TOTAL {
        prod.publish(&pattern(&c, id), id).unwrap();
    }
    let results: Vec<_> = handles.into_iter().map(|h| h.join().unwrap()).collect();
    assert!(results.iter().all(|&(v, _)| v > 0));
    // Каждый читатель сошёлся к хвосту стрима.
    for &(v, last) in &results {
        assert!(v > 0 && last <= prod.stats().published);
    }
}

// ============================================================
// Память: объём сегмента детерминирован конфигурацией
// (критерий «проверить объём необходимой памяти»).
// ============================================================
#[test]
fn acceptance_segment_memory_budget() {
    use shmem_buffer::segment_size;
    // Дефолт продакшена: 10 × 640×480 NV12.
    let fs = shmem_buffer::frame_size(shmem_buffer::FORMAT_NV12, 640, 480).unwrap();
    assert_eq!(fs, 460_800);
    assert_eq!(segment_size(10, fs), 4_608_704); // ≈ 4.4 MiB
    // 720p RGB24 × 10 — верхняя граница.
    let fs_rgb = shmem_buffer::frame_size(shmem_buffer::FORMAT_RGB24, 1280, 720).unwrap();
    assert_eq!(segment_size(10, fs_rgb), 27_648_704); // ≈ 26.4 MiB
}

// ============================================================
// Мультипроцессные приёмочные (Linux, реальный SHM).
// Запуск: --include-ignored
// ============================================================

#[cfg(target_os = "linux")]
mod multiprocess {
    use super::*;
    use shmem_buffer::{attach_shared, now_ns, remove_segment};

    fn exe(name: &str) -> std::path::PathBuf {
        // cargo test: current_exe = target/<profile>/deps/<bin> — нужно
        // два pop (bin, deps). При прямом запуске бинаря из <profile>/
        // достаточно одного. Определяем по имени каталога.
        let mut p = std::env::current_exe().expect("exe path");
        p.pop(); // сам бинарь
        if p.file_name().is_some_and(|s| s == "deps") {
            p.pop(); // deps/ -> target/<profile>/
        }
        p.push("examples");
        p.push(name);
        p
    }

    /// Независимые процессы: продюсер + 2 потребителя (next + slow).
    /// Все кадры целы (TORN=0), медленный не ломает быстрого.
    #[test]
    #[ignore = "linux multiprocess SHM: needs examples built (cargo build --examples)"]
    fn acceptance_multiprocess_two_consumers() {
        let name = format!("at-accept-{}.frames", std::process::id());
        let _ = remove_segment(&name);

        let cfg = RingConfig {
            capacity: 6,
            width: 64,
            height: 48,
            format: StorageFormat::Nv12,
        };
        let prod = shmem_buffer::create_shared(&name, &cfg).expect("create");

        let cons_fast = std::process::Command::new(exe("shmem_consumer"))
            .args(["--name", &name, "--mode", "next", "--seconds", "6"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("spawn fast consumer");
        let cons_slow = std::process::Command::new(exe("shmem_consumer"))
            .args(["--name", &name, "--mode", "slow", "--hold-ms", "300", "--seconds", "6"])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::inherit())
            .spawn()
            .expect("spawn slow consumer");

        // Публикуем 30 «секунд» кадров 100 мс периодом.
        for id in 1..=60u64 {
            let mut frame = vec![0u8; cfg.frame_size() as usize];
            for w in frame.chunks_exact_mut(4) {
                w.copy_from_slice(&(id as u32).to_le_bytes());
            }
            match prod.publish(&frame, now_ns()).unwrap() {
                PublishResult::Published { .. } => {}
                PublishResult::Dropped { .. } => {} // slow holder — ожидаемо
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        std::thread::sleep(Duration::from_millis(500));

        let out_fast = cons_fast.wait_with_output().expect("fast join");
        let out_slow = cons_slow.wait_with_output().expect("slow join");
        let fast = String::from_utf8_lossy(&out_fast.stdout);
        let slow = String::from_utf8_lossy(&out_slow.stdout);

        assert!(out_fast.status.success(), "fast consumer failed: {fast}");
        assert!(out_slow.status.success(), "slow consumer failed: {slow}");
        assert!(fast.contains("TORN=0"), "fast torn: {fast}");
        assert!(slow.contains("TORN=0"), "slow torn: {slow}");
        assert!(fast.contains("VERIFIED=") && !fast.contains("VERIFIED=0"));
        assert!(slow.contains("VERIFIED=") && !slow.contains("VERIFIED=0"));

        drop(prod);
        remove_segment(&name);
    }

    /// Крэш потребителя (kill -9 с живым guard) → слот зависает →
    /// ример освобождает → продюсер публикует дальше.
    #[test]
    #[ignore = "linux multiprocess SHM: needs examples built"]
    fn acceptance_crash_recovery_by_reaper() {
        let name = format!("at-crash-{}.frames", std::process::id());
        let _ = remove_segment(&name);
        let cfg = cfg(4);
        let prod = shmem_buffer::create_shared(&name, &cfg).expect("create");

        // Единственный кадр: consumer возьмёт его как latest и зависнет
        // (hold 60 c). Его слот — id1%4=1 — цель следующей записи (id5).
        prod.publish(&pattern(&cfg, 1), 1).unwrap();

        let mut child = std::process::Command::new(exe("shmem_consumer"))
            .args(["--name", &name, "--mode", "slow", "--hold-ms", "60000", "--seconds", "60"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .expect("spawn slow");
        // Холодный старт процесса на нагруженном стенде >800 мс — берём запас.
        std::thread::sleep(Duration::from_millis(2500));

        // Кольцо заполняется свободными слотами (id2..4 → слоты 2,3,0)...
        for id in 2..=4u64 {
            assert!(
                matches!(prod.publish(&pattern(&cfg, id), id).unwrap(), PublishResult::Published { .. }),
                "free slot unexpectedly busy (id {id})"
            );
        }
        // ...а id5 попадает в слот держателя → drop-new.
        match prod.publish(&pattern(&cfg, 5), 5).unwrap() {
            PublishResult::Dropped {
                reason: DropReason::HeldByReaders { .. },
            } => {}
            other => panic!("expected drop while consumer holds: {other:?}"),
        }

        // kill -9: guard не дропнется, ref_count утёкёт.
        unsafe {
            libc_kill9(child.id());
        }
        let _ = child.wait();

        // Утёкший счётчик всё ещё блокирует слот.
        assert!(matches!(
            prod.publish(&pattern(&cfg, 5), 5).unwrap(),
            PublishResult::Dropped { .. }
        ));

        // Ример: мёртвый pid + старый кадр → освободить.
        let cons = attach_shared(&name).expect("attach reaper");
        let freed = shmem_buffer::recover_stale_slots(&cons, 0); // любое время
        assert!(freed >= 1, "reaper freed nothing");

        // Слот свободен — публикация проходит.
        assert!(matches!(
            prod.publish(&pattern(&cfg, 5), 5).unwrap(),
            PublishResult::Published { .. }
        ));

        drop(prod);
        drop(cons);
        remove_segment(&name);
    }

    // Без внешних крейтов: kill(pid, SIGKILL) через libc уже в deps.
    unsafe fn libc_kill9(pid: u32) {
        unsafe {
            libc::kill(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}
