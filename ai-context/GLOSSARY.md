# Glossary — термины проекта

| Термин | Значение |
|---|---|
| **RK3588 / RK3588S** | SoC от Rockchip с 8-ядерным CPU + NPU (6 TOPS). Целевой борт. |
| **NPU** | Neural Processing Unit — аппаратный ускоритель инференса на RK3588. |
| **RKNN** | формат модели + SDK (`librknnrt.so`) для инференса на NPU. |
| **rknn-toolkit2** | Python-пакет для конвертации ONNX→RKNN (x86 host-side, версия 2.3.0). |
| **MAVLink** | протокол связи с автопилотом (v2). |
| **ArduPilot** | open-source автопилот; целевой FC (SpeedyBee F405). |
| **FC** | Flight Controller — полётный контроллер. |
| **БВК** | Бортовой вычислительный комплекс (= Orange Pi 5). |
| **SITL** | Software-In-The-Loop — симуляция автопилота в Docker. |
| **HITL** | Hardware-In-The-Loop — реальный FC + софт-камера. |
| **FOV** | Field of View — угол обзора камеры. |
| **NED** | North-East-Down — система координат автопилота. |
| **ROI** | Region of Interest — куда направить камеру/гимбал (`MAV_CMD_DO_SET_ROI`). |
| **RTH** | Return-To-Home — возврат домой (режим ArduPilot). |
| **V4L2** | Video4Linux2 — API захвата кадров. |
| **NV12 / YUYV / MJPEG** | пиксельные форматы (YUV 4:2:0 semi-planar / YUV 4:2:2 packed / JPEG). |
| **dmabuf / SCM_RIGHTS** | механизмы zero-copy передачи кадра (план Phase 6). |
| **Anti-loop** | защита от осцилляций автопилота (7 слоёв: watchdogs, deadband, rate-limiter, oscillation detector, RC override, systemd). |
| **Watchdog** | таймер контроля живости цикла (video/inference/tracking/command/FC heartbeat). |
| **YOLOv8** | архитектура детектора объектов (Ultralytics). Выход `[1, 4+nc, anchors]`. |
| **Letterbox** | resize с сохранением пропорций + pad серым (114). |
| **NMS** | Non-Maximum Suppression — подавление дублирующих детекций. |
| **Sigmoid** | `1/(1+e^-x)` — нормализация логитов в [0,1]. RKNN-export НЕ встраивает её (нужно вручную). |
| **Zero-copy API** | `rknn_create_mem` + `rknn_set_io_mem` — NPU пишет прямо в наш буфер, без копий. |
| **SDD** | Spec-Driven Development — разработка через спецификацию (`docs/SDD-SPEC.md`). |
| **COCO** | Common Objects in Context — датасет 80 классов (person, car, bus...). Базовая модель обучена на нём. |
