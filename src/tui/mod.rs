//! mod.rs
//! ratatui 실시간 관제 화면: 터미널 복원 가드, 이벤트 루프, 실행 요청 전달, 창 최상단·콘솔 글꼴 제어.

pub mod cards;
pub mod format;
pub mod i18n;
pub mod interaction;
pub mod layout;
pub mod layout_store;
pub mod poller;
pub mod sort;
pub mod state;
pub mod state_writer;
pub mod theme;
pub mod views;

use std::io;
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use anyhow::Context;
use crossbeam_channel::{Receiver, Sender, unbounded};
use crossterm::cursor::{Hide, Show};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyEventKind, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, SetTitle, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use crate::app::applog;
use crate::collector::MonitorEngine;
use crate::app::config::{home_dir, resolve_tui_state_path};
use crate::tui::cards::CardId;
use crate::tui::layout::SplitGroup;
use crate::tui::poller::PollCommand;
use crate::tui::i18n::Msg;
use crate::tui::state::{AppState, ToastLevel, UiCommand, UiMessage};
use crate::tui::state_writer::{
    STATE_SAVE_JOIN_LIMIT, SaveFinish, StateWriter, spawn_state_writer,
};
use crate::platform::win_ctrl::{
    apply_console_font, capture_initial_terminal_window, elevate_and_flash_terminal,
    set_terminal_topmost,
};

// 입력 대기 단위, 변화가 있을 때만 다시 그림
const FRAME_INTERVAL: Duration = Duration::from_millis(100);
// 변화가 없어도 다시 그리는 최대 간격
const IDLE_REDRAW_INTERVAL: Duration = Duration::from_secs(1);

// 창 최상단 고정 상태 (종료·panic 복원 경로가 공유)
static TOPMOST_ACTIVE: AtomicBool = AtomicBool::new(false);

// 1. 터미널 복원 가드 ---------------------------------------------------------
// 정상 종료·오류·panic 어느 경로든 Drop 으로 원래 화면 복구, 이어서 상태 저장 스레드의 남은 요청을 제한 시간 안에 쓰고 합류 (마지막 확정 저장이 사라지지 않음)
struct TerminalGuard {
    writer: StateWriter,
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore_terminal();
        // UI 가 이미 닫혔으니 복원한 터미널에 한 줄로 알림 (마지막 쓰기 실패, 제한 시간 초과)
        match self.writer.finish_state_saves(STATE_SAVE_JOIN_LIMIT) {
            SaveFinish::Clean => {}
            SaveFinish::Failed(detail) => {
                eprintln!("database-monitor: UI state not saved: {detail}");
            }
            SaveFinish::TimedOut => {
                eprintln!(
                    "database-monitor: UI state save still running after {}s, giving up (the last change may not be saved)",
                    STATE_SAVE_JOIN_LIMIT.as_secs()
                );
            }
        }
    }
}

// 2. 터미널 복원 --------------------------------------------------------------
// 여러 번 불려도 안전 (실패는 무시), 켜 둔 최상단 고정도 해제
fn restore_terminal() {
    let _ = disable_raw_mode();
    let _ = execute!(io::stdout(), LeaveAlternateScreen, DisableMouseCapture, Show);
    if TOPMOST_ACTIVE.swap(false, Ordering::SeqCst) {
        set_terminal_topmost(false);
    }
}

// 3. TUI 실행 -----------------------------------------------------------------
pub fn run_tui(engine: MonitorEngine) -> anyhow::Result<()> {
    // 1. 창 핸들 보관과 콘솔 글꼴 (클래식 콘솔만 적용, 가드가 종료 때 원래 글꼴로 복원)
    capture_initial_terminal_window();
    let console_font = engine.config.console_font.clone();
    let _font_guard = match apply_console_font(&console_font) {
        Ok(guard) => {
            applog::info(&format!("Console font applied: {console_font}"));
            Some(guard)
        }
        Err(reason) => {
            applog::info(&format!("Console font kept: {reason}"));
            None
        }
    };

    // 2. 상태와 상태 파일 복원 (홈의 tui.toml, 시작할 때 한 번)
    let mut app = AppState::new(engine.config.poll_interval_ms);
    app.state_file = resolve_tui_state_path(home_dir().as_deref());
    app.load_tui_state();
    if app.toast.is_none() {
        let notice = match &engine.config.source_path {
            Some(path) => i18n::config_loaded(app.lang, &path.display().to_string()),
            None => i18n::text(app.lang, Msg::ToastConfigNone).to_string(),
        };
        app.show_toast(&notice, ToastLevel::Info);
    }

    // 3. 수집 스레드 (엔진 소유, 결과는 채널로)
    let (ui_tx, ui_rx) = unbounded::<UiMessage>();
    let writer = spawn_state_writer(ui_tx.clone()).context("state writer thread start failed")?;
    let poll_tx = poller::spawn_poller(engine, ui_tx).context("collector thread start failed")?;

    // 4. panic 시 화면 복구 후 메시지 출력
    let previous_hook = panic::take_hook();
    panic::set_hook(Box::new(move |panic_info| {
        restore_terminal();
        previous_hook(panic_info);
    }));

    // 5. 터미널 진입 (마우스 캡처 포함)
    enable_raw_mode().context("raw mode enter failed")?;
    let guard = TerminalGuard { writer };
    execute!(
        io::stdout(),
        EnterAlternateScreen,
        EnableMouseCapture,
        Hide,
        SetTitle("database-monitor")
    )
    .context("alternate screen enter failed")?;
    let mut terminal =
        Terminal::new(CrosstermBackend::new(io::stdout())).context("terminal init failed")?;
    terminal.clear()?;

    // 6. 이벤트 루프
    run_event_loop(&mut terminal, &mut app, &ui_rx, &poll_tx, &guard.writer)
}

// 4. 이벤트 루프 --------------------------------------------------------------
fn run_event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut AppState,
    ui_rx: &Receiver<UiMessage>,
    poll_tx: &Sender<PollCommand>,
    writer: &StateWriter,
) -> anyhow::Result<()> {
    let mut dirty = true;
    let mut last_draw = Instant::now();
    while !app.should_quit {
        // 1. 배경 스레드 메시지 반영, 카드 제목 길게 누름 시계, 알림 만료, 경보 점멸
        let now = Instant::now();
        app.now = now;
        while let Ok(message) = ui_rx.try_recv() {
            let command = app.apply_ui_message(message, now);
            dispatch_ui_command(app, command, poll_tx, writer);
            dirty = true;
        }
        dirty |= app.tick(now);
        dirty |= app.expire_toast(now);
        dirty |= app.tick_blink(now);

        // 2. 그리기 (변화가 있거나 최대 간격 경과, 크기 변경은 draw 가 자동 반영)
        if dirty || last_draw.elapsed() >= IDLE_REDRAW_INTERVAL {
            terminal.draw(|frame| views::render_frame(frame, app))?;
            dirty = false;
            last_draw = Instant::now();
        }

        // 3. 입력 대기 (알림 만료·길게 누름 진입·위험 점멸 중 가장 이른 시각까지)
        let mut wait = FRAME_INTERVAL;
        if let Some(toast) = &app.toast {
            wait = wait.min(toast.expires_at.saturating_duration_since(Instant::now()));
        }
        if let Some(remaining) = app.press_remaining(Instant::now()) {
            wait = wait.min(remaining);
        }
        if app.blink_enabled && app.is_critical() {
            wait = wait.min(
                state::BLINK_GAP.saturating_sub(Instant::now().saturating_duration_since(app.last_blink)),
            );
        }
        if !event::poll(wait)? {
            continue;
        }

        // 4. 입력 처리 (밀린 입력은 한 번에 소화해 다시 그리는 횟수 억제, 키 뗌은 무시)
        loop {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => {
                    let command = app.handle_key_event(key);
                    dispatch_ui_command(app, command, poll_tx, writer);
                    dirty = true;
                }
                Event::Mouse(mouse_event) => {
                    let hover_before = hover_signature(app);
                    let arranging_before = app.card_move.is_some() || app.card_press.is_some();
                    let command = app.handle_mouse_event(mouse_event);
                    dispatch_ui_command(app, command, poll_tx, writer);
                    // 이동은 강조 대상이 바뀐 때나 카드 이동·길게 누름 중이었을 때(확정·취소 가능)만 다시 그림
                    let hover_moved = hover_signature(app) != hover_before;
                    if mouse_event.kind != MouseEventKind::Moved || hover_moved || arranging_before {
                        dirty = true;
                    }
                }
                // 크기 변경 시 경계·칸 위치가 바뀌므로 카드 이동 취소와 강조 해제, 끌던 폭은 저장
                Event::Resize(width, height) => {
                    let command = app.handle_resize(width, height);
                    dispatch_ui_command(app, command, poll_tx, writer);
                    dirty = true;
                }
                _ => {}
            }
            if app.should_quit || !event::poll(Duration::ZERO)? {
                break;
            }
        }
    }
    Ok(())
}

// 마우스 이동으로 바뀔 수 있는 강조 상태: 카드, 카드 경계, 열 경계, 스크롤바 트랙
type HoverSignature = (
    Option<CardId>,
    Option<(SplitGroup, usize)>,
    Option<(CardId, usize)>,
    Option<CardId>,
);

fn hover_signature(app: &AppState) -> HoverSignature {
    (
        app.hover_card,
        app.split_hover,
        app.column_hover,
        app.scroll_hover,
    )
}

// 5. 실행 요청 전달 -----------------------------------------------------------
// 모든 I/O 는 수집 스레드·저장 스레드로, 결과는 UiMessage 로 돌아옴 (창 제어는 즉시 반환 호출)
fn dispatch_ui_command(
    app: &mut AppState,
    command: UiCommand,
    poll_tx: &Sender<PollCommand>,
    writer: &StateWriter,
) {
    match command {
        UiCommand::None => {}
        UiCommand::RefreshNow => {
            let _ = poll_tx.send(PollCommand::RefreshNow);
        }
        UiCommand::SetInterval(millis) => {
            let _ = poll_tx.send(PollCommand::SetInterval(millis));
        }
        UiCommand::SetPaused(paused) => {
            let _ = poll_tx.send(PollCommand::SetPaused(paused));
        }
        UiCommand::EmergencyStop(processes) => {
            let _ = poll_tx.send(PollCommand::EmergencyStop(processes));
        }
        UiCommand::KillProcess { pid, name } => {
            let _ = poll_tx.send(PollCommand::Kill { pid, name });
        }
        UiCommand::SetTopmost(enabled) => {
            TOPMOST_ACTIVE.store(enabled, Ordering::SeqCst);
            set_terminal_topmost(enabled);
        }
        UiCommand::Elevate => {
            TOPMOST_ACTIVE.store(true, Ordering::SeqCst);
            elevate_and_flash_terminal(app.blink_enabled);
        }
        // 확정 이벤트에서는 파일 내용만 만들어 저장 스레드로 보냄 (쓰기와 실패 알림은 저장 스레드와 UiMessage 가 맡아 입력 스레드는 기다리지 않음)
        UiCommand::SaveState => match app.build_state_save() {
            Ok(Some(save)) => {
                if writer.queue_state_save(save).is_err() {
                    let text = i18n::state_save_failed(app.lang, "state writer stopped");
                    app.show_toast(&text, ToastLevel::Error);
                }
            }
            Ok(None) => {}
            Err(detail) => {
                let text = i18n::state_save_failed(app.lang, &detail);
                app.show_toast(&text, ToastLevel::Error);
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // 1. 요청 전달
    #[test]
    fn dispatch_forwards_collector_commands_without_touching_other_state() {
        let mut app = AppState::new(1000);
        let (poll_tx, poll_rx) = unbounded::<PollCommand>();
        let (ui_tx, _ui_rx) = unbounded::<UiMessage>();
        let writer = spawn_state_writer(ui_tx).unwrap();
        dispatch_ui_command(&mut app, UiCommand::RefreshNow, &poll_tx, &writer);
        dispatch_ui_command(&mut app, UiCommand::SetInterval(1500), &poll_tx, &writer);
        dispatch_ui_command(&mut app, UiCommand::SetPaused(true), &poll_tx, &writer);
        dispatch_ui_command(
            &mut app,
            UiCommand::KillProcess {
                pid: 7,
                name: "x.exe".to_string(),
            },
            &poll_tx,
            &writer,
        );
        assert!(matches!(poll_rx.try_recv().unwrap(), PollCommand::RefreshNow));
        assert!(matches!(poll_rx.try_recv().unwrap(), PollCommand::SetInterval(1500)));
        assert!(matches!(poll_rx.try_recv().unwrap(), PollCommand::SetPaused(true)));
        assert!(matches!(
            poll_rx.try_recv().unwrap(),
            PollCommand::Kill { pid: 7, .. }
        ));
        assert!(poll_rx.try_recv().is_err());
    }

    // 2. 저장 요청은 저장 스레드가 주입한 경로에만 쓰고, 실패는 UiMessage 로 돌아와 오류 알림이 됨
    #[test]
    fn save_state_command_queues_to_the_writer_thread_and_a_failure_returns_as_an_error_toast() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut app = AppState::new(1000);
        app.state_file = Some(dir.path().join("tui.toml"));
        let (poll_tx, _poll_rx) = unbounded::<PollCommand>();
        let (ui_tx, ui_rx) = unbounded::<UiMessage>();
        let mut writer = spawn_state_writer(ui_tx.clone()).unwrap();

        // 요청만 보내고 돌아옴, 합류가 끝난 뒤 파일이 있고 실패 메시지는 없음
        dispatch_ui_command(&mut app, UiCommand::SaveState, &poll_tx, &writer);
        assert!(app.toast.is_none(), "{:?}", app.toast);
        assert_eq!(writer.finish_state_saves(STATE_SAVE_JOIN_LIMIT), SaveFinish::Clean);
        let text = std::fs::read_to_string(dir.path().join("tui.toml")).unwrap();
        assert!(text.contains("version = 1") && text.contains("language = \"en\""), "{text}");
        assert!(ui_rx.try_recv().is_err());

        // 저장할 수 없는 위치(파일이 폴더 자리를 차지)는 UiMessage 로 돌아와 오류 알림이 됨
        let mut writer = spawn_state_writer(ui_tx).unwrap();
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, "x").unwrap();
        app.state_file = Some(blocker.join("tui.toml"));
        dispatch_ui_command(&mut app, UiCommand::SaveState, &poll_tx, &writer);
        assert!(app.toast.is_none(), "{:?}", app.toast);
        let finish = writer.finish_state_saves(STATE_SAVE_JOIN_LIMIT);
        let message = ui_rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(matches!(message, UiMessage::StateSaveFailed { .. }));
        assert!(matches!(finish, SaveFinish::Failed(_)), "{finish:?}");
        app.apply_ui_message(message, Instant::now());
        let toast = app.toast.as_ref().unwrap();
        assert_eq!(toast.level, ToastLevel::Error);
        assert!(toast.text.starts_with("State save failed:"), "{}", toast.text);

        // 저장 스레드가 이미 끝났으면 요청을 보낼 수 없어 오류 알림
        app.toast = None;
        app.state_file = Some(dir.path().join("late.toml"));
        dispatch_ui_command(&mut app, UiCommand::SaveState, &poll_tx, &writer);
        assert_eq!(app.toast.as_ref().unwrap().level, ToastLevel::Error);
    }
}
