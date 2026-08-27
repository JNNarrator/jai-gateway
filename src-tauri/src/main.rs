//! JAI 桌面壳（Tauri 2）：网关监督进程 + 系统托盘 + IPC 命令。
//!
//! 稳定性基线落点：
//! - §5-2 超时三件套：随 M1 业务代理落地（当前仅 healthz 监督面）
//! - §5-6 进程看门狗：本文件的 restart 循环
//! - 启动即应用 SQLite 迁移，失败即启动中止（storage §4 早拦截原则）

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use gateway_core::server::{self, AppState};
use gateway_core::store;
use serde::Serialize;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Emitter, Manager, State};

// ---------------------------------------------------------------- 状态模型

#[derive(Debug, Clone, Serialize)]
pub struct GwStatus {
    pub running: bool,
    pub port: u16,
    pub restarts: u64,
}

struct SupervisorInner {
    stop_tx: tokio::sync::watch::Sender<bool>,
}

/// 托盘菜单句柄（运行态刷新文案/可用性用）
struct TrayHandles {
    status_item: MenuItem<tauri::Wry>,
    start_item: MenuItem<tauri::Wry>,
    stop_item: MenuItem<tauri::Wry>,
}

struct GatewayState {
    preferred_port: u16,
    running: Arc<AtomicBool>,
    stop_flag: Arc<AtomicBool>,
    port: Arc<AtomicU16>,
    restarts: Arc<AtomicU64>,
    supervisor: Mutex<Option<SupervisorInner>>,
    tray: Mutex<Option<TrayHandles>>,
}

impl GatewayState {
    fn status(&self) -> GwStatus {
        GwStatus {
            running: self.running.load(Ordering::SeqCst),
            port: self.port.load(Ordering::SeqCst),
            restarts: self.restarts.load(Ordering::SeqCst),
        }
    }
}

// ---------------------------------------------------------------- IPC 命令

#[tauri::command]
fn gateway_status(state: State<'_, GatewayState>) -> GwStatus {
    state.status()
}

#[tauri::command]
async fn gateway_start(app: AppHandle, state: State<'_, GatewayState>) -> Result<GwStatus, String> {
    if state.running.load(Ordering::SeqCst) {
        return Ok(state.status());
    }
    spawn_supervisor(&app, &state).map_err(|e| e.to_string())?;
    reflect_status(&app, &state);
    Ok(state.status())
}

#[tauri::command]
async fn gateway_stop(app: AppHandle, state: State<'_, GatewayState>) -> Result<GwStatus, String> {
    request_stop(&state);
    reflect_status(&app, &state);
    Ok(state.status())
}

// ---------------------------------------------------------------- 监督循环

enum StopKind {
    Manual,
    Crash(String),
}

/// 启动带看门狗的网关任务。
/// 端口顺延（roadmap M0 验收 3）由 bind_with_fallback 保证；
/// 异常退出自动重启（稳定性基线 §5-6）由下方 restart 循环保证。
fn spawn_supervisor(app: &AppHandle, st: &GatewayState) -> Result<(), tauri::Error> {
    if st.supervisor.lock().unwrap().is_some() {
        return Ok(());
    }
    let (stop_tx, mut stop_rx) = tokio::sync::watch::channel(false);

    st.stop_flag.store(false, Ordering::SeqCst);
    st.restarts.store(0, Ordering::SeqCst);

    let running = st.running.clone();
    let stop_flag = st.stop_flag.clone();
    let port_cell = st.port.clone();
    let restarts = st.restarts.clone();
    let preferred_port = st.preferred_port;

    let app_handle = app.clone();
    // detached 任务：生命周期由 running 标志与 stop 信号管理，无需持有句柄
    tauri::async_runtime::spawn(async move {
        running.store(true, Ordering::SeqCst);
        loop {
            // 每轮重新绑定（上一轮可能刚释放端口）
            let (listener, actual_port) =
                match server::bind_with_fallback("127.0.0.1", preferred_port) {
                    Ok(v) => v,
                    Err(e) => {
                        eprintln!("[gateway] 绑定失败，停止监督循环: {e}");
                        let _ = app_handle.emit("gateway://event", format!("bind-failed:{e}"));
                        break;
                    }
                };
            port_cell.store(actual_port, Ordering::SeqCst);
            println!("[gateway] listening on 127.0.0.1:{actual_port}");

            let app_state = AppState {
                version: env!("CARGO_PKG_VERSION").to_string(),
                started_at_ms: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0),
            };

            let serve = tokio::spawn(server::run_until_shutdown(
                listener,
                server::build_router(app_state),
                stop_rx.clone(), // watch::Receiver 是 Clone
            ));
            tokio::pin!(serve);

            let kind = tokio::select! {
                _ = wait_stop(&mut stop_rx) => {
                    // 手动停机：等优雅关闭完成后才离开本轮，避免下一轮绑定撞旧监听
                    let io_res = serve.as_mut().await;
                    if let Err(e) = io_res {
                        eprintln!("[gateway] graceful shutdown io error: {e}");
                    }
                    StopKind::Manual
                }
                done = serve.as_mut() => match done {
                    Ok(Ok(())) => StopKind::Crash("serve 未因停机信号而退出".into()),
                    Ok(Err(e)) => StopKind::Crash(format!("serve io error: {e}")),
                    Err(e) => StopKind::Crash(format!("serve task joined err: {e}")),
                }
            };

            // stop_flag 兜底：即使 select 先落在 Crash 分支，用户已点停机则不重启
            match kind {
                StopKind::Manual => break,
                StopKind::Crash(reason) => {
                    if stop_flag.load(Ordering::SeqCst) {
                        break;
                    }
                    // ---- 看门狗：异常退出延迟重启 ----
                    let n = restarts.fetch_add(1, Ordering::SeqCst) + 1;
                    eprintln!("[watchdog] 网关异常退出({reason})，第 {n} 次自动重启");
                    let _ = app_handle.emit("gateway://event", format!("restart:{n}"));
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        }
        running.store(false, Ordering::SeqCst);
        println!("[gateway] supervisor exited");
    });

    *st.supervisor.lock().unwrap() = Some(SupervisorInner { stop_tx });
    Ok(())
}

async fn wait_stop(rx: &mut tokio::sync::watch::Receiver<bool>) {
    loop {
        if *rx.borrow_and_update() {
            return;
        }
        if rx.changed().await.is_err() {
            return; // sender dropped ⇒ 视为停机指令
        }
    }
}

fn request_stop(st: &GatewayState) {
    st.stop_flag.store(true, Ordering::SeqCst);
    if let Some(inner) = st.supervisor.lock().unwrap().take() {
        let _ = inner.stop_tx.send(true);
        // 监督循环的 Manual 分支会在 serve 优雅关闭完成后才退出并置 running=false，
        // 这里轮询等待即可（上限 2s；超时则后台自行收尾，UI 先行置为已停止）。
        let deadline = std::time::Instant::now() + std::time::Duration::from_millis(2000);
        while st.running.load(Ordering::SeqCst) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }
    st.running.store(false, Ordering::SeqCst);
}

// ---------------------------------------------------------------- 状态外显

fn reflect_status(app: &AppHandle, st: &GatewayState) {
    let s = st.status();
    if let Some(tray) = st.tray.lock().unwrap().as_ref() {
        let text = if s.running {
            format!("状态：运行中 · 127.0.0.1:{} · 重启 {}", s.port, s.restarts)
        } else {
            "状态：已停止".to_string()
        };
        let _ = tray.status_item.set_text(text);
        let _ = tray.start_item.set_enabled(!s.running);
        let _ = tray.stop_item.set_enabled(s.running);
    }
    let _ = app.emit("gateway://status", &s);
}

// ---------------------------------------------------------------- 入口

fn main() {
    tauri::Builder::default()
        .setup(|app| {
            // 1) 数据目录 + 迁移（失败即中止启动 —— storage §4 早拦截）
            let data_dir = app.path().app_data_dir()?;
            std::fs::create_dir_all(&data_dir)?;
            let db_path = data_dir.join("jai.db");
            // open_and_migrate 内含 PRAGMA 三件套（WAL/NORMAL/foreign_keys）
            store::open_and_migrate(&db_path.to_string_lossy())?;
            println!("[store] db ready at {}", db_path.display());

            // 2) 托盘
            let status_item =
                MenuItem::with_id(app, "status", "状态：已停止", false, None::<&str>)?;
            let start_item = MenuItem::with_id(app, "gw-start", "启动网关", true, None::<&str>)?;
            let stop_item = MenuItem::with_id(app, "gw-stop", "停止网关", false, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let show_item = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
            let sep3 = PredefinedMenuItem::separator(app)?;
            let quit_item = MenuItem::with_id(app, "quit", "退出 JAI", true, None::<&str>)?;
            let menu = Menu::with_items(
                app,
                &[
                    &status_item,
                    &sep1,
                    &start_item,
                    &stop_item,
                    &sep2,
                    &show_item,
                    &sep3,
                    &quit_item,
                ],
            )?;

            TrayIconBuilder::with_id("jai-tray")
                .icon(app.default_window_icon().expect("window icon").clone())
                .tooltip("JAI Gateway")
                .menu(&menu)
                .show_menu_on_left_click(true)
                .on_menu_event(|app, event| {
                    let st = app.state::<GatewayState>();
                    match event.id().as_ref() {
                        "gw-start" => {
                            if let Err(e) = spawn_supervisor(app, &st) {
                                eprintln!("[tray] start failed: {e}");
                            }
                            reflect_status(app, &st);
                        }
                        "gw-stop" => {
                            request_stop(&st);
                            reflect_status(app, &st);
                        }
                        "show" => {
                            if let Some(w) = app.get_webview_window("main") {
                                let _ = w.show();
                                let _ = w.set_focus();
                            }
                        }
                        "quit" => app.exit(0),
                        _ => {}
                    }
                })
                .build(app)?;

            // 3) 受管状态 + 初始状态外显
            let gw = GatewayState {
                preferred_port: server::DEFAULT_PORT,
                running: Arc::new(AtomicBool::new(false)),
                stop_flag: Arc::new(AtomicBool::new(false)),
                port: Arc::new(AtomicU16::new(server::DEFAULT_PORT)),
                restarts: Arc::new(AtomicU64::new(0)),
                supervisor: Mutex::new(None),
                tray: Mutex::new(Some(TrayHandles {
                    status_item,
                    start_item,
                    stop_item,
                })),
            };
            app.manage(gw);
            {
                let st = app.state::<GatewayState>();
                spawn_supervisor(app.handle(), &st)?;
                reflect_status(app.handle(), &st);
            }

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            gateway_status,
            gateway_start,
            gateway_stop
        ])
        .run(tauri::generate_context!())
        .expect("error while running jai");
}
