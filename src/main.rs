//! main.rs
//! 외부 DB 부하 0(Zero-DB-Query) 패시브 실시간 터미널 모니터 엔트리포인트.

use std::thread;
use std::time::Duration;
use clap::Parser;
use database_monitor::app::applog;
use database_monitor::app::relaunch;
use database_monitor::app::cli::{CliArgs, print_text_snapshot};
use database_monitor::collector::MonitorEngine;
use database_monitor::app::config::{home_dir, resolve_log_dir};
use database_monitor::tui::{TuiExit, run_tui};

// 1. 엔트리포인트 -------------------------------------------------------------
fn main() {
    relaunch::init_build_identity();
    let args = CliArgs::parse();
    let cfg = match args.into_config() {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("Configuration error: {err:#}");
            std::process::exit(1);
        }
    };
    init_file_log();
    let mut engine = MonitorEngine::new(cfg);
    applog::info(&format!(
        "Monitor started: mode {}, capture {}, poll {}ms, config {}",
        if args.once || args.json { "single snapshot" } else { "TUI" },
        engine.capture_mode(),
        engine.config.poll_interval_ms,
        engine
            .config
            .source_path
            .as_deref()
            .map_or_else(|| "none (default watched ports only)".to_string(), |p| p.display().to_string())
    ));

    if args.once || args.json {
        // 속도 지표 산출용 2회 샘플링
        engine.tick();
        thread::sleep(Duration::from_millis(engine.config.poll_interval_ms));
        let snap = engine.tick();
        if args.json {
            match serde_json::to_string_pretty(&snap) {
                Ok(json) => println!("{json}"),
                Err(err) => {
                    applog::error(&format!("JSON serialization failed: {err}"));
                    eprintln!("JSON serialization failed: {err}");
                    std::process::exit(1);
                }
            }
        } else {
            print_text_snapshot(&snap);
        }
        applog::info("Monitor stopped (single snapshot)");
        return;
    }

    let exit = match run_tui(engine) {
        Ok(exit) => exit,
        Err(err) => {
            applog::error(&format!("Monitor exited abnormally: {err:#}"));
            eprintln!("database-monitor runtime error: {err}");
            std::process::exit(1);
        }
    };
    if exit == TuiExit::Quit {
        applog::info("Monitor stopped (user quit)");
        return;
    }

    // 강제 재시작 (터미널 복원 뒤 최신 exe)
    applog::info("Monitor restarting on the latest build (force restart)");
    match relaunch::rerun_latest_exe() {
        Ok(code) => std::process::exit(code),
        Err(err) => {
            applog::error(&format!("Force restart failed: {err:#}"));
            eprintln!("database-monitor force restart failed: {err:#}");
            std::process::exit(1);
        }
    }
}

// 2. 파일 로그 초기화 ---------------------------------------------------------
// 항상 <홈>/.config/database-monitor/logs (--config 로 다른 설정 파일을 고를 때도 같음), 홈이 없으면 현재 폴더 대체 없이 로그를 끈
fn init_file_log() {
    let Some(log_dir) = resolve_log_dir(home_dir().as_deref()) else {
        eprintln!("Log file disabled: no home directory (USERPROFILE or HOME is not set)");
        return;
    };
    if let Err(err) = applog::init(&log_dir) {
        eprintln!("Log file open failed (continuing without a log): {err}");
        return;
    }

    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        applog::error(&format!("Panic: {info}"));
        default_hook(info);
    }));
}
