//! poller.rs
//! 수집 배경 스레드: 엔진 소유, 주기 수집, 일시정지·주기 변경·즉시 수집, 긴급 중단·프로세스 종료 실행, 결과는 UiMessage 로 전달.

use std::io;
use std::thread;
use std::time::{Duration, Instant};
use crossbeam_channel::{Receiver, RecvTimeoutError, Sender, unbounded};
use crate::app::applog;
use crate::collector::MonitorEngine;
use crate::collector::process::terminate_local_process;
use crate::app::model::ProcessSummary;
use crate::tui::state::UiMessage;

// 일시정지 중 명령 대기 상한
const PAUSED_WAIT: Duration = Duration::from_millis(500);
const SLOW_COLLECT: Duration = Duration::from_secs(1);

// 1. UI → 수집 스레드 명령 ----------------------------------------------------
#[derive(Debug, Clone)]
pub enum PollCommand {
    RefreshNow,
    SetInterval(u64),
    SetPaused(bool),
    EmergencyStop(Vec<ProcessSummary>),
    Kill { pid: u32, name: String },
}

// 2. 수집 스레드 시작 ---------------------------------------------------------
// 엔진을 스레드가 소유, 첫 수집은 즉시, 이후 주기마다 (명령 채널이 닫히면 종료)
pub fn spawn_poller(
    mut engine: MonitorEngine,
    ui_tx: Sender<UiMessage>,
) -> io::Result<Sender<PollCommand>> {
    let (command_tx, command_rx) = unbounded::<PollCommand>();
    thread::Builder::new()
        .name("collector".to_string())
        .spawn(move || run_poller(&mut engine, &ui_tx, &command_rx))?;
    Ok(command_tx)
}

// 3. 수집 루프 ----------------------------------------------------------------
fn run_poller(
    engine: &mut MonitorEngine,
    ui_tx: &Sender<UiMessage>,
    command_rx: &Receiver<PollCommand>,
) {
    let mut paused = false;
    let mut interval = Duration::from_millis(engine.config.poll_interval_ms);
    if !send_snapshot(engine, ui_tx) {
        return;
    }
    let mut next_tick = Instant::now() + interval;

    loop {
        // 1. 다음 수집 시각까지 명령 대기 (일시정지 중에는 짧게 끊어 반복)
        let wait = if paused {
            PAUSED_WAIT
        } else {
            next_tick.saturating_duration_since(Instant::now())
        };
        match command_rx.recv_timeout(wait) {
            Ok(PollCommand::RefreshNow) => {
                if !send_snapshot(engine, ui_tx) {
                    return;
                }
                next_tick = Instant::now() + interval;
            }
            Ok(PollCommand::SetInterval(millis)) => {
                engine.config.poll_interval_ms = millis;
                interval = Duration::from_millis(millis);
                next_tick = Instant::now() + interval;
            }
            Ok(PollCommand::SetPaused(value)) => {
                paused = value;
                next_tick = Instant::now() + interval;
            }
            Ok(PollCommand::EmergencyStop(processes)) => {
                let result = engine.emergency_stop_checked(&processes);
                if ui_tx.send(UiMessage::StopDone(result)).is_err() || !send_snapshot(engine, ui_tx)
                {
                    return;
                }
                next_tick = Instant::now() + interval;
            }
            Ok(PollCommand::Kill { pid, name }) => {
                let result = terminate_local_process(pid, &name);
                if ui_tx
                    .send(UiMessage::KillDone { pid, name, result })
                    .is_err()
                    || !send_snapshot(engine, ui_tx)
                {
                    return;
                }
                next_tick = Instant::now() + interval;
            }
            Err(RecvTimeoutError::Timeout) => {
                if !paused && Instant::now() >= next_tick {
                    if !send_snapshot(engine, ui_tx) {
                        return;
                    }
                    next_tick = Instant::now() + interval;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

// 4. 한 번 수집해 전달 --------------------------------------------------------
// 반환 = UI 가 살아 있는지, 느린 수집은 로그만 남김
fn send_snapshot(engine: &mut MonitorEngine, ui_tx: &Sender<UiMessage>) -> bool {
    let started = Instant::now();
    let snapshot = engine.tick();
    let cost = started.elapsed();
    if cost >= SLOW_COLLECT {
        applog::warn(&format!(
            "Slow collection {}ms (input may lag), {} sockets",
            cost.as_millis(),
            snapshot.connections.len()
        ));
    }
    ui_tx.send(UiMessage::Snapshot(Box::new(snapshot))).is_ok()
}
