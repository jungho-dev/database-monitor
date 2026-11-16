//! tui.rs
//! TUI 화면 검증: 크기별 렌더 버퍼 셀, 히트 영역, 정렬·열 폭·스크롤바, 제스처(시계 주입), 언어 전환, 도움말·종료 확인.

#[allow(dead_code)]
mod common;

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};
use crossterm::event::{
    KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers, MouseButton, MouseEvent,
    MouseEventKind,
};
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier};
use ratatui::text::Span;
use database_monitor::app::model::{MonitorSnapshot, RiskLevel, RiskReason};
use database_monitor::tui::cards::{CARD_HOLD_TIME, CardId, UiAction, scroll_thumb};
use database_monitor::tui::i18n::{self, Lang, Msg, reason_text, text};
use database_monitor::tui::layout::SplitGroup;
use database_monitor::tui::sort::{self, SortKey, SortTable};
use database_monitor::tui::state::{ActiveTab, AppState, KillConfirm, ToastLevel, UiCommand, UiMessage};
use database_monitor::tui::state_writer::{STATE_SAVE_JOIN_LIMIT, SaveFinish, spawn_state_writer};
use database_monitor::tui::views::{render_frame, risk_cell_text};
use common::{bloat_snapshot, build_fixture_snapshot};

const SIZES: [(u16, u16); 3] = [(60, 16), (100, 30), (130, 40)];

// 1. 공용 도구 ----------------------------------------------------------------
// 위험 경보 카드 없는 스냅샷 (정상 등급, 행 수 확장)
fn calm_snapshot(extra_rows: usize) -> MonitorSnapshot {
    let mut snap = bloat_snapshot(&build_fixture_snapshot(true), extra_rows);
    snap.overall_risk = RiskLevel::Warning;
    snap
}

fn critical_snapshot() -> MonitorSnapshot {
    build_fixture_snapshot(true)
}

fn state_with(snap: MonitorSnapshot, tab: ActiveTab, lang: Lang) -> AppState {
    let mut state = AppState::new(1000);
    state.tab = tab;
    state.lang = lang;
    state.snapshot = Some(Arc::new(snap));
    state.snapshot_at = Some(state.now);
    state
}

fn draw(state: &mut AppState, width: u16, height: u16) -> Buffer {
    let mut terminal = Terminal::new(TestBackend::new(width, height)).expect("terminal");
    terminal
        .draw(|frame| render_frame(frame, state))
        .expect("draw");
    terminal.backend().buffer().clone()
}

// 넓은 글자의 이어지는 칸을 건너뛴 줄 목록
fn dump(buffer: &Buffer) -> Vec<String> {
    let area = buffer.area;
    (0..area.height)
        .map(|y| {
            let mut row = String::new();
            let mut x = 0;
            while x < area.width {
                let symbol = buffer[(x, y)].symbol();
                row.push_str(symbol);
                x += Span::raw(symbol).width().max(1) as u16;
            }
            row
        })
        .collect()
}

fn has_hangul(value: &str) -> bool {
    value.chars().any(|ch| ('\u{AC00}'..='\u{D7A3}').contains(&ch))
}

fn key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        code,
        modifiers: KeyModifiers::NONE,
        kind: KeyEventKind::Press,
        state: KeyEventState::NONE,
    }
}

fn shift_key(code: KeyCode) -> KeyEvent {
    KeyEvent {
        modifiers: KeyModifiers::SHIFT,
        ..key(code)
    }
}

fn mouse(kind: MouseEventKind, column: u16, row: u16) -> MouseEvent {
    MouseEvent {
        kind,
        column,
        row,
        modifiers: KeyModifiers::NONE,
    }
}

fn after(base: Instant, millis: u64) -> Instant {
    base + Duration::from_millis(millis)
}

fn card_rect(state: &AppState, card: CardId) -> Rect {
    state
        .hit_areas
        .card_rects
        .iter()
        .find(|(id, _)| *id == card)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("{card:?} 카드 영역 없음"))
}

fn button_rect(state: &AppState, action: UiAction) -> Rect {
    state
        .hit_areas
        .button_rects
        .iter()
        .find(|(candidate, _)| *candidate == action)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("{action:?} 칩 영역 없음"))
}

// 칩 줄의 영역 (경보 카드와 같은 동작이면 칩이 나중에 등록됨)
fn chip_rect(state: &AppState, action: UiAction) -> Rect {
    state
        .hit_areas
        .button_rects
        .iter()
        .rev()
        .find(|(candidate, _)| *candidate == action)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("{action:?} 칩 영역 없음"))
}

// 영역 안 셀 글자 (넓은 글자의 이어지는 칸 제외, 앞뒤 공백 제거)
fn cells_text(buffer: &Buffer, rect: Rect) -> String {
    let mut row = String::new();
    let mut x = rect.x;
    while x < rect.right() {
        let symbol = buffer[(x, rect.y)].symbol();
        row.push_str(symbol);
        x += Span::raw(symbol).width().max(1) as u16;
    }
    row.trim().to_string()
}

fn press_at(state: &mut AppState, column: u16, row: u16, now: Instant) -> UiCommand {
    state.handle_mouse_event_at(mouse(MouseEventKind::Down(MouseButton::Left), column, row), now)
}

fn drag_at(state: &mut AppState, column: u16, row: u16, now: Instant) -> UiCommand {
    state.handle_mouse_event_at(mouse(MouseEventKind::Drag(MouseButton::Left), column, row), now)
}

fn release_at(state: &mut AppState, column: u16, row: u16, now: Instant) -> UiCommand {
    state.handle_mouse_event_at(mouse(MouseEventKind::Up(MouseButton::Left), column, row), now)
}

fn click_at(state: &mut AppState, column: u16, row: u16) -> UiCommand {
    let base = Instant::now();
    let command = press_at(state, column, row, base);
    release_at(state, column, row, after(base, 10));
    command
}

fn perimeter(rect: Rect, x: u16, y: u16) -> bool {
    rect.contains(ratatui::layout::Position::new(x, y))
        && (x == rect.x || x == rect.right() - 1 || y == rect.y || y == rect.bottom() - 1)
}

// 2. 크기·언어 행렬 -----------------------------------------------------------
#[test]
fn every_tab_renders_in_both_languages_at_the_size_matrix_with_chrome_and_chips() {
    for critical in [false, true] {
        for lang in [Lang::En, Lang::Ko] {
            for tab in ActiveTab::ALL {
                for (width, height) in SIZES {
                    let snap = if critical { critical_snapshot() } else { calm_snapshot(5) };
                    let mut state = state_with(snap, tab, lang);
                    let buffer = draw(&mut state, width, height);
                    let rows = dump(&buffer);
                    let label = format!("critical={critical} {lang:?} {tab:?} {width}x{height}");

                    // 1. 탭 이름과 하단 칩 (도움말·종료·언어 칩은 최소 크기에서도 보임)
                    let screen = rows.join("\n");
                    assert!(screen.contains(tab.tab_label(lang)), "{label} 탭 이름");
                    let (help, quit, language) = match lang {
                        Lang::En => ("[?] Help", "[q] Quit", "[L] EN"),
                        Lang::Ko => ("[?] 도움말", "[q] 종료", "[L] KO"),
                    };
                    let footer = &rows[usize::from(height) - 2];
                    for chip in [help, quit, language] {
                        assert!(footer.contains(chip), "{label} 칩 {chip}: {footer}");
                    }

                    // 2. 앱 바·탭 바·하단 줄에는 영어 모드에서 한글이 없음, 한국어 모드에는 있음
                    let chrome = [&rows[0], &rows[1], &rows[usize::from(height) - 2]];
                    for row in chrome {
                        if lang == Lang::En {
                            assert!(!has_hangul(row), "{label} 영어 칩에 한글: {row}");
                        }
                    }
                    if lang == Lang::Ko {
                        assert!(has_hangul(&rows[1]), "{label} 탭 바 한글");
                    }

                    // 3. 클릭 영역: 탭 4개, 칩은 화면 안, 카드는 서로 겹치지 않음
                    let area = Rect::new(0, 0, width, height);
                    assert_eq!(state.hit_areas.tab_rects.len(), 4, "{label} 탭 영역");
                    for (_, rect) in &state.hit_areas.tab_rects {
                        assert!(area.contains(rect.as_position()) && rect.right() <= width, "{label}");
                    }
                    for (action, rect) in &state.hit_areas.button_rects {
                        assert!(!rect.is_empty() && rect.right() <= width, "{label} {action:?}");
                    }
                    let rects = state.hit_areas.card_rects.clone();
                    for (index, (id, rect)) in rects.iter().enumerate() {
                        for (other_id, other) in &rects[index + 1..] {
                            assert!(rect.intersection(*other).is_empty(), "{label} {id:?}/{other_id:?}");
                        }
                    }
                }
            }
        }
    }
}

// 3. 크기 미달 ----------------------------------------------------------------
#[test]
fn below_minimum_size_draws_only_the_catalog_gate_line_in_both_languages() {
    for lang in [Lang::En, Lang::Ko] {
        for (width, height) in [(59u16, 16u16), (60, 15), (40, 10), (20, 8)] {
            let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, lang);
            let buffer = draw(&mut state, width, height);
            let rows = dump(&buffer);
            let expected = match lang {
                Lang::En => format!("Terminal too small (min 60x16, now {width}x{height})"),
                Lang::Ko => format!("터미널 크기 부족 (최소 60x16, 현재 {width}x{height})"),
            };
            let noted: Vec<&String> = rows.iter().filter(|row| !row.trim().is_empty()).collect();
            assert_eq!(noted.len(), 1, "{lang:?} {width}x{height}");
            if Span::raw(expected.as_str()).width() <= usize::from(width) {
                assert!(noted[0].contains(&expected), "{lang:?} {width}x{height}: {}", noted[0]);
            }
            let line_y = rows.iter().position(|row| !row.trim().is_empty()).unwrap() as u16;
            assert_eq!(line_y, height / 2, "{lang:?} {width}x{height} 가운데 줄");
            let first_x = (0..width)
                .find(|x| buffer[(*x, line_y)].symbol() != " ")
                .unwrap();
            assert_eq!(buffer[(first_x, line_y)].fg, Color::Indexed(220));
            assert!(state.hit_areas.tab_rects.is_empty() && state.hit_areas.button_rects.is_empty());
            assert!(state.hit_areas.card_rects.is_empty());
        }
    }
    // 59 폭에서는 영어 문구 전체(41열)가 들어감
    let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, Lang::En);
    let rows = dump(&draw(&mut state, 59, 16));
    assert!(rows[8].contains("Terminal too small (min 60x16, now 59x16)"), "{}", rows[8]);
}

// 4. 포커스·호버·위험 테두리 --------------------------------------------------
#[test]
fn focus_hover_and_critical_borders_have_exact_corner_symbols_and_colors() {
    let corner = |buffer: &Buffer, rect: Rect| {
        let cell = &buffer[(rect.x, rect.y)];
        (cell.symbol().to_string(), cell.fg)
    };
    let focus = ("┏".to_string(), Color::Indexed(39));
    let idle = ("╭".to_string(), Color::Indexed(240));
    let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);

    // 1. 기본: 탭의 주 카드가 굵은 포커스, 나머지는 둥근 기본, 제목은 반전
    let buffer = draw(&mut state, 130, 40);
    let processes = card_rect(&state, CardId::Processes);
    assert_eq!(corner(&buffer, processes), focus);
    assert_eq!(corner(&buffer, card_rect(&state, CardId::Inspector)), idle);
    let title = &buffer[(processes.x + 2, processes.y)];
    assert_eq!((title.fg, title.bg), (Color::Indexed(16), Color::Indexed(39)));
    assert!(title.modifier.contains(Modifier::BOLD));

    // 2. 호버: 둥근선 유지, 중간 밝기
    state.hover_card = Some(CardId::Sockets);
    let buffer = draw(&mut state, 130, 40);
    assert_eq!(
        corner(&buffer, card_rect(&state, CardId::Sockets)),
        ("╭".to_string(), Color::Indexed(67))
    );

    // 3. 클릭 포커스 이동
    state.hover_card = None;
    state.focused_card = Some((ActiveTab::Overview, CardId::Inspector));
    let buffer = draw(&mut state, 130, 40);
    assert_eq!(corner(&buffer, card_rect(&state, CardId::Inspector)), focus);
    assert_eq!(corner(&buffer, card_rect(&state, CardId::Processes)), idle);

    // 4. 위험 점멸 고정(점멸 OFF): 위험색이 포커스·호버보다 우선하고 굵은 선은 유지
    let mut critical = state_with(critical_snapshot(), ActiveTab::Overview, Lang::En);
    critical.blink_enabled = false;
    let buffer = draw(&mut critical, 130, 40);
    assert_eq!(
        corner(&buffer, card_rect(&critical, CardId::Processes)),
        ("┏".to_string(), Color::Indexed(203))
    );
    assert_eq!(
        corner(&buffer, card_rect(&critical, CardId::Risk)),
        ("╭".to_string(), Color::Indexed(203))
    );
}

// 5. 포커스 색 예약 -----------------------------------------------------------
#[test]
fn focus_color_appears_only_on_the_focused_card_border_and_title() {
    for tab in ActiveTab::ALL {
        for (width, height) in SIZES {
            let mut state = state_with(calm_snapshot(40), tab, Lang::En);
            let buffer = draw(&mut state, width, height);
            let focused = card_rect(&state, state.focused_card_id());
            for y in 0..height {
                for x in 0..width {
                    let cell = &buffer[(x, y)];
                    let scrollbar = matches!(cell.symbol(), "░" | "█");
                    if cell.fg == Color::Indexed(39) {
                        assert!(
                            perimeter(focused, x, y) && !scrollbar,
                            "{tab:?} {width}x{height} ({x},{y}) 포커스색 오용 {:?}",
                            cell.symbol()
                        );
                    }
                    if cell.bg == Color::Indexed(39) {
                        assert!(y == focused.y && x > focused.x && x < focused.right() - 1);
                    }
                }
            }
        }
    }
}

// 6. 스크롤바 -----------------------------------------------------------------
#[test]
fn scrollbar_thumb_and_track_sit_in_a_gray_gutter_beside_an_untouched_border() {
    // 1. 포커스 카드(경보 로그 100x30)
    let mut state = state_with(calm_snapshot(60), ActiveTab::Alerts, Lang::En);
    let buffer = draw(&mut state, 100, 30);
    let log = card_rect(&state, CardId::AlertLog);
    let hit = state
        .hit_areas
        .scroll_hits
        .iter()
        .find(|hit| hit.card == CardId::AlertLog)
        .copied()
        .expect("경보 로그 스크롤바 영역");
    let gutter_x = log.right() - 2;
    assert_eq!(hit.track.x, gutter_x);
    assert_eq!(hit.track.width, 2);
    let thumb = &buffer[(gutter_x, hit.track.y)];
    assert_eq!(thumb.symbol(), "█");
    assert_eq!(thumb.fg, Color::Indexed(250));
    let track = &buffer[(gutter_x, hit.track.bottom() - 1)];
    assert_eq!(track.symbol(), "░");
    assert_eq!(track.fg, Color::Indexed(238));
    let border = &buffer[(log.right() - 1, hit.track.y)];
    assert_eq!((border.symbol(), border.fg), ("┃", Color::Indexed(39)));
    assert_ne!(thumb.fg, border.fg);

    // 2. 포커스 아닌 카드(통합관제 소켓 표 130x40)는 한 단계 어두운 썸
    let mut overview = state_with(calm_snapshot(60), ActiveTab::Overview, Lang::En);
    let buffer = draw(&mut overview, 130, 40);
    let sockets = card_rect(&overview, CardId::Sockets);
    let hit = overview
        .hit_areas
        .scroll_hits
        .iter()
        .find(|hit| hit.card == CardId::Sockets)
        .copied()
        .expect("소켓 표 스크롤바 영역");
    let thumb = &buffer[(sockets.right() - 2, hit.track.y)];
    assert_eq!((thumb.symbol(), thumb.fg), ("█", Color::Indexed(245)));
    assert_eq!(buffer[(sockets.right() - 1, hit.track.y)].symbol(), "│");

    // 3. 트랙 글자 왼쪽 칸은 스크롤바 글자가 아님 (본문 여백 유지)
    assert!(!matches!(buffer[(sockets.right() - 3, hit.track.y)].symbol(), "█" | "░"));

    // 4. 어느 크기·언어에서나 스크롤바 거터 글자는 포커스색·강조색을 쓰지 않음
    for (width, height) in SIZES {
        let mut small = state_with(calm_snapshot(60), ActiveTab::Overview, Lang::Ko);
        let buffer = draw(&mut small, width, height);
        assert!(!small.hit_areas.scroll_hits.is_empty(), "{width}x{height} 스크롤바 없음");
        for track in &small.hit_areas.scroll_hits {
            for y in track.track.y..track.track.bottom() {
                let cell = &buffer[(track.track.x, y)];
                assert!(matches!(cell.symbol(), "░" | "█"), "{width}x{height} {:?}", cell.symbol());
                assert_ne!(cell.fg, Color::Indexed(39));
                assert_ne!(cell.fg, Color::Indexed(81));
            }
        }
    }
}

// 7. 선택 행 띠 ---------------------------------------------------------------
#[test]
fn selected_row_band_uses_the_selection_background_and_follows_the_selection() {
    let mut state = state_with(calm_snapshot(40), ActiveTab::Overview, Lang::En);
    let buffer = draw(&mut state, 130, 40);
    let processes = card_rect(&state, CardId::Processes);
    let rows = state
        .hit_areas
        .row_hits
        .iter()
        .find(|hit| hit.card == CardId::Processes)
        .copied()
        .expect("프로세스 행 영역");
    let band_y = rows.rect.y;
    for x in processes.x + 1..processes.right() - 2 {
        assert_eq!(buffer[(x, band_y)].bg, Color::Indexed(24), "x={x}");
    }
    assert_eq!(buffer[(processes.x, band_y)].bg, Color::Indexed(235));
    assert_eq!(buffer[(processes.x + 3, band_y + 1)].bg, Color::Indexed(235));

    // 아래 화살표: 띠가 한 줄 아래로, 이전 줄은 패널 바탕
    state.handle_key_event(key(KeyCode::Down));
    let buffer = draw(&mut state, 130, 40);
    assert_eq!(buffer[(processes.x + 3, band_y + 1)].bg, Color::Indexed(24));
    assert_eq!(buffer[(processes.x + 3, band_y)].bg, Color::Indexed(235));

    // 포커스가 다른 카드로 가면 선택 띠는 바탕만 (글자색 유지)
    state.focused_card = Some((ActiveTab::Overview, CardId::Inspector));
    let buffer = draw(&mut state, 130, 40);
    assert_eq!(buffer[(processes.x + 3, band_y + 1)].bg, Color::Indexed(237));
}

// 8. 알림 ---------------------------------------------------------------------
#[test]
fn only_the_error_toast_is_a_red_fill_and_expires_after_four_seconds() {
    for (width, height) in [(60u16, 16u16), (100, 30)] {
        let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, Lang::En);
        let base = state.now;
        let hint_row = height - 1;
        let count_bg = |buffer: &Buffer, color: u8| {
            (0..width)
                .filter(|x| buffer[(*x, hint_row)].bg == Color::Indexed(color))
                .count()
        };

        state.show_toast("Saved", ToastLevel::Info);
        let buffer = draw(&mut state, width, height);
        assert_eq!(count_bg(&buffer, 160), 0, "{width}x{height} 정보 알림은 적색 채움 아님");
        assert!(count_bg(&buffer, 238) >= 7, "{width}x{height} 정보 알림 칩 톤");
        assert!(dump(&buffer)[usize::from(hint_row)].contains("Saved"));

        state.show_toast("Disk full", ToastLevel::Error);
        let buffer = draw(&mut state, width, height);
        let filled = count_bg(&buffer, 160);
        assert!(filled >= "Disk full".len() + 3, "{width}x{height} 오류 알림 채움 {filled}");
        let dot = (0..width)
            .find(|x| buffer[(*x, hint_row)].symbol() == "●")
            .expect("알림 점");
        assert_eq!(buffer[(dot, hint_row)].fg, Color::Indexed(231));
        assert_eq!(buffer[(dot, hint_row)].bg, Color::Indexed(160));
        // 다른 줄에는 적색 채움 없음
        for y in 0..hint_row {
            assert_eq!(
                (0..width).filter(|x| buffer[(*x, y)].bg == Color::Indexed(160)).count(),
                0
            );
        }

        // 4초 만료 (주입한 시계)
        assert!(!state.expire_toast(after(base, 3_999)));
        assert!(state.expire_toast(after(base, 4_000)));
        let buffer = draw(&mut state, width, height);
        assert_eq!(count_bg(&buffer, 160), 0);
    }
}

// 9. 언어 전환 ----------------------------------------------------------------
#[test]
fn upper_l_switches_language_live_keeps_rows_in_place_and_lower_l_keeps_the_language() {
    let mut state = state_with(calm_snapshot(8), ActiveTab::Processes, Lang::En);
    draw(&mut state, 130, 40);
    state.handle_key_event(key(KeyCode::Down));
    state.handle_key_event(key(KeyCode::Down));
    let order_of = |state: &mut AppState| -> Vec<String> {
        let rows = dump(&draw(state, 130, 40));
        let table = card_rect(state, CardId::Processes);
        rows[usize::from(table.y) + 2..usize::from(table.bottom()) - 1]
            .iter()
            .map(|row| row.split_whitespace().nth(1).unwrap_or("").to_string())
            .collect()
    };
    let before = order_of(&mut state);
    let selected_before = state.card_views[CardId::Processes as usize].selected;
    assert_eq!(selected_before, 2);

    // 1. L: 언어 전환, 칩과 화면 문구가 즉시 바뀌고 행 순서·선택은 그대로
    let command = state.handle_key_event(key(KeyCode::Char('L')));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(state.lang, Lang::Ko);
    let rows = dump(&draw(&mut state, 60, 16));
    assert!(rows[14].contains("[L] KO"), "{}", rows[14]);
    let after = order_of(&mut state);
    assert_eq!(before, after);
    assert_eq!(state.card_views[CardId::Processes as usize].selected, selected_before);

    // 2. l: 배치 초기화만, 언어 유지
    let command = state.handle_key_event(key(KeyCode::Char('l')));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(state.lang, Lang::Ko);

    // 3. 다시 L: 영어
    state.handle_key_event(key(KeyCode::Char('L')));
    assert_eq!(state.lang, Lang::En);
    let rows = dump(&draw(&mut state, 60, 16));
    assert!(rows[14].contains("[L] EN"), "{}", rows[14]);
}

// 10. 칩 클릭 영역 ------------------------------------------------------------
#[test]
fn every_footer_chip_is_clickable_and_runs_its_key_action() {
    for lang in [Lang::En, Lang::Ko] {
        for (width, height) in SIZES {
            let fresh = || {
                let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, lang);
                draw(&mut state, width, height);
                state
            };
            let chip_click = |action: UiAction| {
                let mut state = fresh();
                let rect = button_rect(&state, action);
                let command = click_at(&mut state, rect.x + 1, rect.y);
                (state, command)
            };
            let label = format!("{lang:?} {width}x{height}");

            let (state, _) = chip_click(UiAction::ShowHelp);
            assert!(state.show_help, "{label} 도움말");
            let (state, _) = chip_click(UiAction::Quit);
            assert!(state.should_quit, "{label} 종료");
            let (state, command) = chip_click(UiAction::ToggleLanguage);
            assert_eq!(state.lang, lang.toggled(), "{label} 언어");
            assert!(matches!(command, UiCommand::SaveState), "{label}");
            // 긴급 중단 칩은 즉시 실행하지 않고 확인 창을 염 (Y 를 눌러야 실행)
            let (state, command) = chip_click(UiAction::EmergencyStop);
            assert!(matches!(command, UiCommand::None), "{label} 중단");
            assert!(matches!(state.kill_confirm, Some(KillConfirm::Emergency(ref list)) if !list.is_empty()));
        }
    }

    // 폭이 넉넉하면 나머지 칩도 같은 동작
    let fresh = || {
        let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, Lang::En);
        draw(&mut state, 130, 40);
        state
    };
    let mut state = fresh();
    let rect = button_rect(&state, UiAction::TogglePause);
    let command = click_at(&mut state, rect.x + 1, rect.y);
    assert!(matches!(command, UiCommand::SetPaused(true)) && state.paused);
    let mut state = fresh();
    let rect = button_rect(&state, UiAction::Refresh);
    assert!(matches!(click_at(&mut state, rect.x + 1, rect.y), UiCommand::RefreshNow));
    let mut state = fresh();
    let rect = button_rect(&state, UiAction::TogglePin);
    assert!(matches!(click_at(&mut state, rect.x + 1, rect.y), UiCommand::SetTopmost(true)));
    let mut state = fresh();
    let rect = button_rect(&state, UiAction::ToggleBlink);
    click_at(&mut state, rect.x + 1, rect.y);
    assert!(!state.blink_enabled);
    let mut state = fresh();
    let rect = button_rect(&state, UiAction::ResetLayout);
    assert!(matches!(click_at(&mut state, rect.x + 1, rect.y), UiCommand::SaveState));
}

// 11. 칩 스타일과 줄어드는 순서 -----------------------------------------------
#[test]
fn chips_use_tonal_style_quit_key_is_red_text_only_and_language_chip_drops_last() {
    // 1. 칩 셀 스타일 (최소 크기)
    let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, Lang::En);
    let buffer = draw(&mut state, 60, 16);
    let language = chip_rect(&state, UiAction::ToggleLanguage);
    let quit = chip_rect(&state, UiAction::Quit);
    let help = chip_rect(&state, UiAction::ShowHelp);
    let y = language.y;
    assert_eq!(y, 14);
    assert_eq!(buffer[(language.x + 1, y)].symbol(), "[");
    assert_eq!(buffer[(language.x + 2, y)].fg, Color::Indexed(81));
    assert_eq!(buffer[(language.x + 2, y)].bg, Color::Indexed(238));
    assert_eq!(buffer[(quit.x + 2, y)].fg, Color::Indexed(203));
    for x in quit.x..quit.right() {
        assert_eq!(buffer[(x, y)].bg, Color::Indexed(238), "종료 칩은 적색 채움 아님");
    }
    for x in help.x..help.right() {
        assert_eq!(buffer[(x, y)].bg, Color::Indexed(238));
    }
    // 도움말·종료는 오른쪽 끝에 고정 (종료 칩 오른쪽 1열 여백)
    assert_eq!(quit.right(), 59);
    assert!(help.right() < quit.x);

    // 2. 위험 중에만 중단 칩이 적색 채움
    let mut critical = state_with(critical_snapshot(), ActiveTab::Overview, Lang::En);
    let buffer = draw(&mut critical, 100, 30);
    let stop = chip_rect(&critical, UiAction::EmergencyStop);
    assert_eq!(buffer[(stop.x + 2, stop.y)].bg, Color::Indexed(203));
    let mut calm = state_with(calm_snapshot(3), ActiveTab::Overview, Lang::En);
    let buffer = draw(&mut calm, 100, 30);
    let stop = chip_rect(&calm, UiAction::EmergencyStop);
    assert_eq!(buffer[(stop.x + 2, stop.y)].bg, Color::Indexed(238));

    // 3. 폭을 줄이면 왼쪽 칩부터 사라지고 언어 칩이 마지막, 도움말·종료는 항상 유지
    let left_chips = |width: u16| -> Vec<UiAction> {
        let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, Lang::Ko);
        draw(&mut state, width, 20);
        let actions: Vec<UiAction> = state
            .hit_areas
            .button_rects
            .iter()
            .map(|(action, _)| *action)
            .collect();
        assert!(actions.contains(&UiAction::ShowHelp) && actions.contains(&UiAction::Quit));
        actions
            .into_iter()
            .filter(|action| !matches!(action, UiAction::ShowHelp | UiAction::Quit))
            .collect()
    };
    let mut previous = left_chips(60);
    assert!(previous.contains(&UiAction::ToggleLanguage));
    for width in 61..=140 {
        let current = left_chips(width);
        assert!(current.contains(&UiAction::ToggleLanguage), "{width}");
        for action in &previous {
            assert!(current.contains(action), "{width} 에서 {action:?} 사라짐");
        }
        previous = current;
    }
    assert!(previous.len() >= 6, "넓은 화면에서 모든 칩 표시");
}

// 12. 탭 바 -------------------------------------------------------------------
#[test]
fn tab_bar_compacts_without_clipping_and_clicking_a_tab_switches_it() {
    for lang in [Lang::En, Lang::Ko] {
        for (width, height) in SIZES {
            let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, lang);
            let buffer = draw(&mut state, width, height);
            assert_eq!(state.hit_areas.tab_rects.len(), 4, "{lang:?} {width}");

            // 활성 탭: 패널 바탕에 굵은 밑줄 이름
            let (active, rect) = state.hit_areas.tab_rects[0];
            assert_eq!(active, ActiveTab::Overview);
            let name_x = rect.x + 3;
            assert!(buffer[(name_x, rect.y)].modifier.contains(Modifier::UNDERLINED));
            assert_eq!(buffer[(name_x, rect.y)].bg, Color::Indexed(235));

            // 클릭: 각 탭으로 전환
            for (tab, rect) in state.hit_areas.tab_rects.clone() {
                let mut clicked = state_with(calm_snapshot(3), ActiveTab::Alerts, lang);
                draw(&mut clicked, width, height);
                click_at(&mut clicked, rect.x + 1, rect.y);
                assert_eq!(clicked.tab, tab, "{lang:?} {width}x{height}");
            }
        }
    }
}

// 13. 앱 바 신선도 ------------------------------------------------------------
#[test]
fn app_bar_shows_waiting_fresh_and_stale_dots_after_three_poll_intervals() {
    // 1. 첫 집계 전: 대기 점과 안내 카드
    let mut waiting = AppState::new(1000);
    let buffer = draw(&mut waiting, 100, 30);
    let rows = dump(&buffer);
    assert!(rows[0].contains("○ Waiting"), "{}", rows[0]);
    assert!(rows.join("\n").contains("Waiting for first poll"));
    assert!(waiting.hit_areas.card_rects.iter().any(|(id, _)| *id == CardId::Gate));

    // 2. 정상: 녹색 점과 수집 시각
    let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, Lang::En);
    let base = state.now;
    state.now = after(base, 3_000);
    let buffer = draw(&mut state, 100, 30);
    let dot = (0..100u16).find(|x| buffer[(*x, 0)].symbol() == "●").expect("점");
    assert_eq!(buffer[(dot, 0)].fg, Color::Indexed(78));
    let snap_time = state.snapshot.as_ref().unwrap().captured_at.clone();
    assert!(dump(&buffer)[0].contains(&snap_time));

    // 3. 폴링 주기 3배 초과: 경고 점과 경과 초
    state.now = after(base, 3_001);
    let buffer = draw(&mut state, 100, 30);
    let dot = (0..100u16).find(|x| buffer[(*x, 0)].symbol() == "●").expect("점");
    assert_eq!(buffer[(dot, 0)].fg, Color::Indexed(220));
    assert!(dump(&buffer)[0].contains("stale 3s"));

    // 4. 한국어: 지연 표시도 카탈로그
    state.lang = Lang::Ko;
    let rows = dump(&draw(&mut state, 100, 30));
    assert!(rows[0].contains("지연 3초"), "{}", rows[0]);

    // 5. 범위는 폭이 모자라면 수집 방식부터 생략, 배지는 유지
    state.lang = Lang::En;
    let rows = dump(&draw(&mut state, 60, 16));
    assert!(rows[0].contains("DB-MONITOR"));
    assert!(rows[0].contains("stale 3s"));
    assert!(!rows[0].contains("Passive"), "{}", rows[0]);
    let rows = dump(&draw(&mut state, 130, 40));
    assert!(rows[0].contains("Mode Passive"), "{}", rows[0]);
}

// 14. 경보 카드 ---------------------------------------------------------------
#[test]
fn siren_card_holds_one_phase_when_blink_is_off_and_click_requests_a_stop() {
    // (점멸 사용, 점멸 위상) -> 경보 카드 바탕
    for (blink_enabled, blink_on, expected) in [(true, false, 226u8), (true, true, 196), (false, false, 196)] {
        let mut state = state_with(critical_snapshot(), ActiveTab::Overview, Lang::En);
        state.blink_enabled = blink_enabled;
        state.blink_on = blink_on;
        let buffer = draw(&mut state, 100, 30);
        let siren = button_rect(&state, UiAction::EmergencyStop);
        assert!(siren.height == 3 && siren.width == 100);
        assert_eq!(
            buffer[(siren.x, siren.y + 1)].bg,
            Color::Indexed(expected),
            "enabled={blink_enabled} on={blink_on}"
        );
    }
    let mut state = state_with(critical_snapshot(), ActiveTab::Overview, Lang::En);
    let buffer = draw(&mut state, 100, 30);
    assert!(dump(&buffer).join("\n").contains("Click or X: stop risky processes (confirm)"));
    let siren = button_rect(&state, UiAction::EmergencyStop);
    let command = click_at(&mut state, siren.x + 5, siren.y + 1);
    assert!(matches!(command, UiCommand::None));
    assert!(matches!(state.kill_confirm, Some(KillConfirm::Emergency(_))));
    let command = state.handle_key_event(key(KeyCode::Char('y')));
    assert!(matches!(command, UiCommand::EmergencyStop(ref list) if !list.is_empty()));

    // 최소 크기: 한 줄 경보 (테두리 없음), 한국어 문구
    let mut small = state_with(critical_snapshot(), ActiveTab::Overview, Lang::Ko);
    let rows = dump(&draw(&mut small, 60, 16));
    let siren = button_rect(&small, UiAction::EmergencyStop);
    assert_eq!(siren.height, 1);
    assert!(rows[usize::from(siren.y)].contains("클릭 또는 X 키"), "{}", rows[usize::from(siren.y)]);

    // 위험이 아니면 경보 카드 없음
    let mut calm = state_with(calm_snapshot(3), ActiveTab::Overview, Lang::En);
    draw(&mut calm, 100, 30);
    assert!(
        !calm
            .hit_areas
            .button_rects
            .iter()
            .any(|(action, rect)| *action == UiAction::EmergencyStop && rect.height >= 1 && rect.width == 100)
    );
}

// 15. 표 머리글 정렬 ----------------------------------------------------------
#[test]
fn header_click_cycles_numeric_descending_first_text_ascending_first_then_resets() {
    let mut state = state_with(calm_snapshot(5), ActiveTab::TopSql, Lang::En);
    let header = |state: &AppState, column: usize| {
        state
            .hit_areas
            .header_cells
            .iter()
            .find(|hit| hit.card == CardId::TopSqlRanking && hit.column == column)
            .copied()
            .unwrap_or_else(|| panic!("머리글 {column} 영역 없음"))
    };
    let counts = |state: &AppState| -> Vec<u64> {
        let snap = state.snapshot.as_ref().unwrap();
        sort::top_sql(snap, &state.sorts).iter().map(|row| row.exec_count).collect()
    };
    let buffer = draw(&mut state, 160, 45);
    let hit = header(&state, 3);
    assert_eq!(cells_text(&buffer, hit.rect), "Executions");
    let table = card_rect(&state, CardId::TopSqlRanking);
    let title_row = |buffer: &Buffer| dump(buffer)[usize::from(table.y)].clone();
    assert!(!title_row(&buffer).contains("Sort"));

    // 1. 숫자 열: 내림차순 -> 오름차순 -> 기본 순위
    let default_counts = counts(&state);
    click_at(&mut state, hit.rect.x, hit.rect.y);
    assert_eq!(state.sorts.get(SortTable::TopSql), Some(SortKey { column: 3, descending: true }));
    assert!(counts(&state).windows(2).all(|pair| pair[0] >= pair[1]));
    let buffer = draw(&mut state, 160, 45);
    assert!(title_row(&buffer).contains("· Sort Executions↓"), "{}", title_row(&buffer));
    let sorted_rect = header(&state, 3).rect;
    assert_eq!(cells_text(&buffer, sorted_rect), "Executions↓");
    let header_cell = buffer[(sorted_rect.right() - 1, sorted_rect.y)].clone();
    assert_eq!(header_cell.symbol(), "↓");
    assert_eq!(header_cell.fg, Color::Indexed(81));

    click_at(&mut state, hit.rect.x, hit.rect.y);
    assert_eq!(state.sorts.get(SortTable::TopSql), Some(SortKey { column: 3, descending: false }));
    assert!(counts(&state).windows(2).all(|pair| pair[0] <= pair[1]));
    assert!(title_row(&draw(&mut state, 160, 45)).contains("· Sort Executions↑"));

    click_at(&mut state, hit.rect.x, hit.rect.y);
    assert_eq!(state.sorts.get(SortTable::TopSql), None);
    assert_eq!(counts(&state), default_counts);
    assert!(!title_row(&draw(&mut state, 160, 45)).contains("Sort"));

    // 2. 문자 열(작업, 1열): 오름차순 -> 내림차순 -> 기본 순위
    let op = header(&state, 1);
    click_at(&mut state, op.rect.x, op.rect.y);
    assert_eq!(state.sorts.get(SortTable::TopSql), Some(SortKey { column: 1, descending: false }));
    click_at(&mut state, op.rect.x, op.rect.y);
    assert_eq!(state.sorts.get(SortTable::TopSql), Some(SortKey { column: 1, descending: true }));
    click_at(&mut state, op.rect.x, op.rect.y);
    assert_eq!(state.sorts.get(SortTable::TopSql), None);

    // 3. 한국어 제목: 정렬 키는 열 번호라 언어를 바꿔도 순서가 그대로
    click_at(&mut state, hit.rect.x, hit.rect.y);
    let numeric_order = counts(&state);
    state.lang = Lang::Ko;
    let buffer = draw(&mut state, 160, 45);
    assert!(title_row(&buffer).contains("· 정렬 실행횟수↓"), "{}", title_row(&buffer));
    assert_eq!(counts(&state), numeric_order);

    // 4. s / S 키도 같은 순환
    let mut keys = state_with(calm_snapshot(5), ActiveTab::TopSql, Lang::En);
    draw(&mut keys, 160, 45);
    keys.handle_key_event(key(KeyCode::Char('s')));
    assert_eq!(keys.sorts.get(SortTable::TopSql), Some(SortKey { column: 0, descending: true }));
    keys.handle_key_event(key(KeyCode::Char('S')));
    assert_eq!(keys.sorts.get(SortTable::TopSql), Some(SortKey { column: 0, descending: false }));
    keys.handle_key_event(key(KeyCode::Char('S')));
    assert_eq!(keys.sorts.get(SortTable::TopSql), None);
}

// 16. 행 선택·클릭 영역 -------------------------------------------------------
#[test]
fn row_and_header_hit_areas_match_the_drawn_cells_and_clicking_a_row_selects_it() {
    let mut state = state_with(calm_snapshot(10), ActiveTab::Processes, Lang::En);
    let buffer = draw(&mut state, 130, 40);
    let table = card_rect(&state, CardId::Processes);
    let rows = state
        .hit_areas
        .row_hits
        .iter()
        .find(|hit| hit.card == CardId::Processes)
        .copied()
        .unwrap();
    let headers: Vec<_> = state
        .hit_areas
        .header_cells
        .iter()
        .filter(|hit| hit.card == CardId::Processes)
        .copied()
        .collect();
    assert_eq!(headers.len(), 9, "130 폭에서는 모든 열이 보임");
    let header_y = headers[0].rect.y;
    assert_eq!(rows.rect.y, header_y + 1);
    assert_eq!(cells_text(&buffer, headers[1].rect), "PID");
    assert_eq!(cells_text(&buffer, headers[2].rect), "Process");
    assert_eq!(rows.len, state.snapshot.as_ref().unwrap().processes.len());
    assert_eq!(rows.rect.x, table.x + 2);

    // 행 클릭: 선택 이동, 같은 위치 줄은 띠 바탕
    click_at(&mut state, rows.rect.x + 3, rows.rect.y + 2);
    assert_eq!(state.card_views[CardId::Processes as usize].selected, 2);
    let buffer = draw(&mut state, 130, 40);
    assert_eq!(buffer[(rows.rect.x + 3, rows.rect.y + 2)].bg, Color::Indexed(24));

    // 행 아래 빈 곳 클릭은 선택을 바꾸지 않음
    let outside = Rect::new(rows.rect.x, rows.rect.bottom(), 1, 1);
    click_at(&mut state, outside.x, outside.y);
    assert_eq!(state.card_views[CardId::Processes as usize].selected, 2);
}

// 17. 열 폭 -------------------------------------------------------------------
#[test]
fn dragging_a_header_boundary_sets_the_width_double_click_resets_it_and_l_clears_all_widths() {
    let mut state = state_with(calm_snapshot(5), ActiveTab::Processes, Lang::Ko);
    draw(&mut state, 130, 40);
    let grab = state
        .hit_areas
        .column_hits
        .iter()
        .find(|hit| hit.card == CardId::Processes && hit.column == 2)
        .copied()
        .expect("프로세스 열 경계");
    let base = Instant::now();

    // 1. 끌기: 폭 + 이동량, 놓으면 저장 요청
    press_at(&mut state, grab.grab.x, grab.grab.y, base);
    drag_at(&mut state, grab.grab.x + 4, grab.grab.y, after(base, 20));
    let command = release_at(&mut state, grab.grab.x + 4, grab.grab.y, after(base, 40));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(
        state.column_widths[CardId::Processes as usize][2],
        Some(grab.width + 4)
    );
    let buffer = draw(&mut state, 130, 40);
    let moved = state
        .hit_areas
        .column_hits
        .iter()
        .find(|hit| hit.card == CardId::Processes && hit.column == 2)
        .copied()
        .unwrap();
    assert_eq!(moved.width, grab.width + 4);
    assert_eq!(moved.grab.x, grab.grab.x + 4);

    // 2. 끄는 동안 세로 손잡이는 표 높이 전체, 평소에는 없음
    let idle_cell = &buffer[(moved.grab.x, moved.grab.y + 3)];
    assert_ne!(idle_cell.symbol(), "┃");
    press_at(&mut state, moved.grab.x, moved.grab.y, after(base, 2_000));
    drag_at(&mut state, moved.grab.x + 1, moved.grab.y, after(base, 2_010));
    let buffer = draw(&mut state, 130, 40);
    assert_eq!(buffer[(moved.grab.x + 1, moved.grab.y + 3)].symbol(), "┃");
    assert_eq!(buffer[(moved.grab.x + 1, moved.grab.y + 3)].fg, Color::Indexed(231));
    release_at(&mut state, moved.grab.x + 1, moved.grab.y, after(base, 2_020));

    // 3. 더블클릭(500ms 안): 자동 폭 복원
    draw(&mut state, 130, 40);
    let boundary = state
        .hit_areas
        .column_hits
        .iter()
        .find(|hit| hit.card == CardId::Processes && hit.column == 2)
        .copied()
        .unwrap();
    press_at(&mut state, boundary.grab.x, boundary.grab.y, after(base, 5_000));
    release_at(&mut state, boundary.grab.x, boundary.grab.y, after(base, 5_050));
    let command = press_at(&mut state, boundary.grab.x, boundary.grab.y, after(base, 5_300));
    assert!(matches!(command, UiCommand::SaveState));
    assert!(state.column_widths[CardId::Processes as usize].iter().all(Option::is_none));

    // 4. 500ms 넘으면 더블클릭 아님
    draw(&mut state, 130, 40);
    state.column_widths[CardId::Processes as usize][2] = Some(20);
    press_at(&mut state, boundary.grab.x, boundary.grab.y, after(base, 8_000));
    release_at(&mut state, boundary.grab.x, boundary.grab.y, after(base, 8_050));
    press_at(&mut state, boundary.grab.x, boundary.grab.y, after(base, 8_700));
    assert_eq!(state.column_widths[CardId::Processes as usize][2], Some(20));

    // 5. l: 열 폭·카드 크기·순서 복원, 언어 유지
    let command = state.handle_key_event(key(KeyCode::Char('l')));
    assert!(matches!(command, UiCommand::SaveState));
    assert!(state.column_widths[CardId::Processes as usize].iter().all(Option::is_none));
    assert_eq!(state.lang, Lang::Ko);
}

// 18. 자연 폭 맞춤 ------------------------------------------------------------
#[test]
fn table_cells_keep_natural_widths_without_ellipsis_when_room_exists() {
    let mut state = state_with(calm_snapshot(2), ActiveTab::Processes, Lang::En);
    let rows = dump(&draw(&mut state, 130, 40));
    let table = card_rect(&state, CardId::Processes);
    for row in &rows[usize::from(table.y) + 1..usize::from(table.bottom()) - 1] {
        assert!(!row.contains('…'), "여유가 있는데 말줄임: {row}");
    }
    // 좁으면 글자 열부터 줄고 말줄임, 오른쪽 열 숨김은 마지막
    let mut narrow = state_with(calm_snapshot(2), ActiveTab::Processes, Lang::En);
    let buffer = draw(&mut narrow, 60, 16);
    let headers = narrow
        .hit_areas
        .header_cells
        .iter()
        .filter(|hit| hit.card == CardId::Processes)
        .count();
    assert!((1..9).contains(&headers), "좁은 폭에서 일부 열만 표시: {headers}");
    let _ = buffer;
}

// 19. 위치 표시 ---------------------------------------------------------------
#[test]
fn bottom_border_shows_the_visible_range_and_total_and_follows_scrolling() {
    let mut state = state_with(calm_snapshot(60), ActiveTab::Alerts, Lang::En);
    let total = state.snapshot.as_ref().unwrap().alerts.len();
    draw(&mut state, 100, 30);
    let log = card_rect(&state, CardId::AlertLog);
    let visible = state.card_views[CardId::AlertLog as usize].visible;
    let bottom = |state: &mut AppState| dump(&draw(state, 100, 30))[usize::from(log.bottom()) - 1].clone();
    assert!(bottom(&mut state).contains(&format!(" 1-{visible} / {total} ")), "{}", bottom(&mut state));

    state.handle_key_event(key(KeyCode::End));
    let line = bottom(&mut state);
    assert!(line.contains(&format!(" {}-{total} / {total} ", total - visible + 1)), "{line}");

    // 한국어에서도 같은 형식, 내용이 들어가는 카드는 표시 없음
    let mut few = state_with(calm_snapshot(0), ActiveTab::Alerts, Lang::Ko);
    let rows = dump(&draw(&mut few, 100, 30));
    let log = card_rect(&few, CardId::AlertLog);
    assert!(!rows[usize::from(log.bottom()) - 1].contains(" / "));
}

// 20. 휠·썸 드래그·트랙 페이지 ------------------------------------------------
#[test]
fn wheel_scrolls_the_card_under_the_pointer_and_thumb_drag_and_track_clicks_move_the_view() {
    let mut state = state_with(calm_snapshot(60), ActiveTab::Overview, Lang::En);
    draw(&mut state, 130, 40);
    let sockets = card_rect(&state, CardId::Sockets);
    let base = Instant::now();

    // 1. 휠: 포인터 아래 카드만
    state.handle_mouse_event_at(mouse(MouseEventKind::ScrollDown, sockets.x + 4, sockets.y + 3), base);
    assert_eq!(state.card_views[CardId::Sockets as usize].scroll, 3);
    assert_eq!(state.card_views[CardId::Processes as usize].scroll, 0);
    state.handle_mouse_event_at(mouse(MouseEventKind::ScrollUp, sockets.x + 4, sockets.y + 3), base);
    assert_eq!(state.card_views[CardId::Sockets as usize].scroll, 0);

    // 2. 트랙 클릭: 썸 아래는 한 화면 아래로, 테두리 열도 같은 트랙
    draw(&mut state, 130, 40);
    let hit = state
        .hit_areas
        .scroll_hits
        .iter()
        .find(|hit| hit.card == CardId::Sockets)
        .copied()
        .unwrap();
    click_at(&mut state, hit.track.x + 1, hit.track.bottom() - 1);
    let page = state.card_views[CardId::Sockets as usize].visible;
    assert_eq!(state.card_views[CardId::Sockets as usize].scroll, page);
    assert_eq!(state.focused_card_id(), CardId::Sockets);

    // 3. 썸 드래그: 끝까지 끌면 최대 스크롤, 놓으면 끝남
    draw(&mut state, 130, 40);
    let hit = state
        .hit_areas
        .scroll_hits
        .iter()
        .find(|hit| hit.card == CardId::Sockets)
        .copied()
        .unwrap();
    let thumb = scroll_thumb(hit.total, hit.visible, hit.scroll, hit.track.height).unwrap();
    press_at(&mut state, hit.track.x, hit.track.y + thumb.start, base);
    drag_at(&mut state, hit.track.x, hit.track.bottom() + 5, after(base, 20));
    let view = state.card_views[CardId::Sockets as usize];
    assert_eq!(view.scroll, view.total - view.visible);
    release_at(&mut state, hit.track.x, hit.track.bottom() + 5, after(base, 40));
    assert!(state.scroll_drag.is_none());

    // 4. 호버한 트랙은 썸이 밝아짐 (회색 계열)
    state.handle_mouse_event_at(mouse(MouseEventKind::Moved, hit.track.x, hit.track.y), after(base, 60));
    let buffer = draw(&mut state, 130, 40);
    let track = state
        .hit_areas
        .scroll_hits
        .iter()
        .find(|hit| hit.card == CardId::Sockets)
        .copied()
        .unwrap();
    let thumb = (track.track.y..track.track.bottom())
        .map(|y| &buffer[(track.track.x, y)])
        .find(|cell| cell.symbol() == "█")
        .unwrap();
    assert_eq!(thumb.fg, Color::Indexed(254));
}

// 21. 카드 길게 누름 이동 -----------------------------------------------------
#[test]
fn long_press_on_a_card_title_enters_move_mode_previews_live_and_release_commits() {
    let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
    draw(&mut state, 130, 40);
    let title = state
        .hit_areas
        .title_hits
        .iter()
        .find(|hit| hit.card == CardId::Processes)
        .copied()
        .unwrap();
    let slots_before = state
        .hit_areas
        .slot_groups
        .iter()
        .find(|slots| slots.group == SplitGroup::OverviewCols)
        .cloned()
        .unwrap();
    let base = Instant::now();

    // 1. 누른 채 가만히 있어도 400ms 에 진입 (움직임 이벤트 없음)
    press_at(&mut state, title.rect.x + 1, title.rect.y, base);
    assert!(state.card_press.is_some() && state.card_move.is_none());
    assert_eq!(state.press_remaining(after(base, 100)), Some(Duration::from_millis(300)));
    assert!(!state.tick(after(base, 399)));
    assert!(state.tick(base + CARD_HOLD_TIME));
    assert!(state.card_move.is_some() && state.card_press.is_none());

    // 2. 같은 칸 흔들림은 상관없음, 다른 칸 슬롯으로 끌면 미리보기 순서가 바뀜
    let target = slots_before.slots[1];
    drag_at(&mut state, target.x + target.width / 2, target.y + 3, after(base, 500));
    assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [1, 0]);
    let buffer = draw(&mut state, 130, 40);
    let processes = card_rect(&state, CardId::Processes);
    let sockets = card_rect(&state, CardId::Sockets);
    assert!(sockets.x < processes.x, "미리보기에서 위치 교환");
    // 이동 중 카드: 점선(LightDoubleDashed)과 강조색 반전 제목, 제목에 이동 표시 (이중선은 겹침 창 전용)
    assert_eq!(buffer[(processes.x, processes.y)].symbol(), "┌");
    assert_eq!(buffer[(processes.x, processes.y)].fg, Color::Indexed(81));
    assert_eq!(buffer[(processes.x, processes.y + 2)].symbol(), "╎", "왼쪽 테두리 점선");
    assert_eq!(buffer[(processes.right() - 1, processes.y + 2)].symbol(), "╎");
    assert_eq!(buffer[(processes.x + 1, processes.bottom() - 1)].symbol(), "╌", "아래 테두리 점선");
    for y in processes.y..processes.bottom() {
        for x in processes.x..processes.right() {
            let symbol = buffer[(x, y)].symbol();
            assert!(!matches!(symbol, "╔" | "╗" | "╚" | "╝" | "═" | "║"), "이동 중 카드에 이중선 사용: {x},{y}");
        }
    }
    assert!(dump(&buffer)[usize::from(processes.y)].contains("· moving"));
    // 칸 크기는 칸에 남음: 칸 영역 불변
    let slots_after = state
        .hit_areas
        .slot_groups
        .iter()
        .find(|slots| slots.group == SplitGroup::OverviewCols)
        .cloned()
        .unwrap();
    assert_eq!(slots_after.slots, slots_before.slots);
    assert_eq!(sockets, slots_before.slots[0]);
    assert_eq!(processes, slots_before.slots[1]);

    // 3. 놓으면 확정과 저장 요청
    let command = release_at(&mut state, target.x + 2, target.y + 3, after(base, 600));
    assert!(matches!(command, UiCommand::SaveState));
    assert!(state.card_move.is_none());
    assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [1, 0]);
    draw(&mut state, 130, 40);
    assert!(card_rect(&state, CardId::Sockets).x < card_rect(&state, CardId::Processes).x);
}

#[test]
fn move_mode_cancel_restores_the_order_and_ignores_every_key_but_escape_and_ctrl_c() {
    let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
    draw(&mut state, 130, 40);
    let title = state
        .hit_areas
        .title_hits
        .iter()
        .find(|hit| hit.card == CardId::Processes)
        .copied()
        .unwrap();
    let target = state
        .hit_areas
        .slot_groups
        .iter()
        .find(|slots| slots.group == SplitGroup::OverviewCols)
        .unwrap()
        .slots[1];
    let base = Instant::now();
    press_at(&mut state, title.rect.x + 1, title.rect.y, base);
    state.tick(after(base, 400));
    drag_at(&mut state, target.x + 5, target.y + 3, after(base, 450));
    assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [1, 0]);

    // 이동 중에는 다른 키가 아무 일도 하지 않음 (종료·탭 전환·언어)
    state.handle_key_event(key(KeyCode::Char('q')));
    state.handle_key_event(key(KeyCode::Tab));
    state.handle_key_event(key(KeyCode::Char('L')));
    assert!(!state.should_quit);
    assert_eq!(state.tab, ActiveTab::Overview);
    assert_eq!(state.lang, Lang::En);
    assert!(state.card_move.is_some());

    // Esc: 누르기 전 순서 복원
    state.handle_key_event(key(KeyCode::Esc));
    assert!(state.card_move.is_none());
    assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [0, 1]);
    assert!(!state.should_quit);

    // Ctrl+C 는 이동 중에도 종료
    press_at(&mut state, title.rect.x + 1, title.rect.y, after(base, 1_000));
    state.tick(after(base, 1_400));
    state.handle_key_event(KeyEvent {
        modifiers: KeyModifiers::CONTROL,
        ..key(KeyCode::Char('c'))
    });
    assert!(state.should_quit);
}

#[test]
fn lost_release_commits_on_the_first_buttonless_move_or_a_new_press_and_is_consumed() {
    let setup = || {
        let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
        draw(&mut state, 130, 40);
        let title = state
            .hit_areas
            .title_hits
            .iter()
            .find(|hit| hit.card == CardId::Processes)
            .copied()
            .unwrap();
        let target = state
            .hit_areas
            .slot_groups
            .iter()
            .find(|slots| slots.group == SplitGroup::OverviewCols)
            .unwrap()
            .slots[1];
        (state, title, target)
    };
    let base = Instant::now();

    // 1. 버튼 없는 움직임: 보이는 대로 확정과 저장
    let (mut state, title, target) = setup();
    press_at(&mut state, title.rect.x + 1, title.rect.y, base);
    state.tick(after(base, 400));
    drag_at(&mut state, target.x + 5, target.y + 3, after(base, 450));
    let command = state.handle_mouse_event_at(
        mouse(MouseEventKind::Moved, target.x + 6, target.y + 3),
        after(base, 500),
    );
    assert!(matches!(command, UiCommand::SaveState));
    assert!(state.card_move.is_none());
    assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [1, 0]);

    // 2. 새 누름: 확정만 하고 클릭으로는 실행하지 않음 (포커스·선택 그대로)
    let (mut state, title, target) = setup();
    let focus_before = state.focused_card_id();
    press_at(&mut state, title.rect.x + 1, title.rect.y, base);
    state.tick(after(base, 400));
    drag_at(&mut state, target.x + 5, target.y + 3, after(base, 450));
    let command = press_at(&mut state, target.x + 6, target.y + 4, after(base, 500));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(state.focused_card_id(), CardId::Processes);
    assert_eq!(focus_before, CardId::Processes);
    assert_eq!(state.card_views[CardId::Sockets as usize].selected, 0);

    // 3. 원래 칸에 놓으면 아무 변화·저장 없음
    let (mut state, title, _) = setup();
    press_at(&mut state, title.rect.x + 1, title.rect.y, base);
    state.tick(after(base, 400));
    let command = release_at(&mut state, title.rect.x + 1, title.rect.y, after(base, 450));
    assert!(matches!(command, UiCommand::None));
    assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [0, 1]);

    // 4. 시간 전 움직임(버튼 없음)은 일반 클릭으로 끝, 이동 모드 진입 안 함
    let (mut state, title, _) = setup();
    press_at(&mut state, title.rect.x + 1, title.rect.y, base);
    state.handle_mouse_event_at(
        mouse(MouseEventKind::Moved, title.rect.x + 2, title.rect.y),
        after(base, 100),
    );
    assert!(state.card_press.is_none() && state.card_move.is_none());
    assert!(!state.tick(after(base, 800)));
}

#[test]
fn press_released_early_is_a_click_and_a_cell_change_cancels_while_jitter_in_the_cell_does_not() {
    let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
    draw(&mut state, 130, 40);
    let title = state
        .hit_areas
        .title_hits
        .iter()
        .find(|hit| hit.card == CardId::Sockets)
        .copied()
        .unwrap();
    let base = Instant::now();

    // 1. 400ms 전 놓음: 포커스만 이동
    press_at(&mut state, title.rect.x + 1, title.rect.y, base);
    assert_eq!(state.focused_card_id(), CardId::Sockets);
    let command = release_at(&mut state, title.rect.x + 1, title.rect.y, after(base, 399));
    assert!(matches!(command, UiCommand::None));
    assert!(state.card_press.is_none() && state.card_move.is_none());
    assert!(!state.tick(after(base, 900)));

    // 2. 같은 칸 안 끌림은 유지, 다른 칸으로 끌면 취소
    press_at(&mut state, title.rect.x + 1, title.rect.y, after(base, 1_000));
    drag_at(&mut state, title.rect.x + 1, title.rect.y, after(base, 1_100));
    assert!(state.card_press.is_some());
    drag_at(&mut state, title.rect.x + 2, title.rect.y, after(base, 1_200));
    assert!(state.card_press.is_none());
    assert!(!state.tick(after(base, 1_500)));
    assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [0, 1]);
}

#[test]
fn shift_arrows_move_the_focused_card_on_drawn_groups_and_l_restores_the_default_order() {
    // 1. 그려지기 전에는 아무 일도 하지 않음
    let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
    let command = state.handle_key_event(shift_key(KeyCode::Right));
    assert!(matches!(command, UiCommand::None));

    // 2. 가로 그룹은 좌우, 세로 방향키는 무시
    draw(&mut state, 130, 40);
    let command = state.handle_key_event(shift_key(KeyCode::Down));
    assert!(matches!(command, UiCommand::None));
    let command = state.handle_key_event(shift_key(KeyCode::Right));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [1, 0]);
    draw(&mut state, 130, 40);
    assert!(card_rect(&state, CardId::Sockets).x < card_rect(&state, CardId::Processes).x);

    // 3. 끝에서는 더 못 감, 반대로 한 칸
    assert!(matches!(state.handle_key_event(shift_key(KeyCode::Right)), UiCommand::None));
    assert!(matches!(state.handle_key_event(shift_key(KeyCode::Left)), UiCommand::SaveState));
    assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [0, 1]);

    // 4. 세로 그룹(에이전트·프로세스 탭): 표와 인스펙터가 위아래로 교환
    let mut rows = state_with(calm_snapshot(5), ActiveTab::Processes, Lang::En);
    draw(&mut rows, 130, 40);
    assert!(matches!(rows.handle_key_event(shift_key(KeyCode::Down)), UiCommand::SaveState));
    draw(&mut rows, 130, 40);
    assert!(card_rect(&rows, CardId::Inspector).y < card_rect(&rows, CardId::Processes).y);

    // 5. l: 순서·크기 복원, 언어 유지
    rows.lang = Lang::Ko;
    assert!(matches!(rows.handle_key_event(key(KeyCode::Char('l'))), UiCommand::SaveState));
    assert_eq!(rows.card_layout.order_of(SplitGroup::ProcessesRows)[..2], [0, 1]);
    assert_eq!(rows.lang, Lang::Ko);
}

#[test]
fn split_boundaries_drag_as_ratios_and_resize_or_tab_switch_during_a_move_restore_the_order() {
    // 1. 경계 드래그: 비율로 보관되어 크기가 바뀌어도 같은 비율
    let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
    draw(&mut state, 130, 40);
    let hit = state
        .hit_areas
        .split_hits
        .iter()
        .find(|hit| hit.group == SplitGroup::OverviewCols)
        .copied()
        .unwrap();
    let base = Instant::now();
    press_at(&mut state, hit.grab.x + 1, hit.grab.y + 5, base);
    drag_at(&mut state, hit.grab.x + 13, hit.grab.y + 5, after(base, 20));
    let command = release_at(&mut state, hit.grab.x + 13, hit.grab.y + 5, after(base, 40));
    assert!(matches!(command, UiCommand::SaveState));
    draw(&mut state, 130, 40);
    let moved = state
        .hit_areas
        .split_hits
        .iter()
        .find(|hit| hit.group == SplitGroup::OverviewCols)
        .copied()
        .unwrap();
    assert_eq!(moved.line.x, hit.line.x + 12);
    draw(&mut state, 100, 30);
    let narrower = state
        .hit_areas
        .split_hits
        .iter()
        .find(|hit| hit.group == SplitGroup::OverviewCols)
        .copied()
        .unwrap();
    let ratio = f64::from(moved.cuts[0]) / f64::from(moved.span_len);
    let expected = ratio * f64::from(narrower.span_len);
    assert!((f64::from(narrower.cuts[0]) - expected).abs() <= 1.0);

    // 2. 이동 중 크기 변경·탭 전환은 누르기 전 순서로 복원
    for resize in [true, false] {
        let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
        draw(&mut state, 130, 40);
        let title = state
            .hit_areas
            .title_hits
            .iter()
            .find(|hit| hit.card == CardId::Processes)
            .copied()
            .unwrap();
        let target = state
            .hit_areas
            .slot_groups
            .iter()
            .find(|slots| slots.group == SplitGroup::OverviewCols)
            .unwrap()
            .slots[1];
        press_at(&mut state, title.rect.x + 1, title.rect.y, base);
        state.tick(after(base, 400));
        drag_at(&mut state, target.x + 5, target.y + 3, after(base, 450));
        assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [1, 0]);
        if resize {
            state.handle_resize(130, 40);
        } else {
            state.switch_tab(ActiveTab::Alerts);
        }
        assert!(state.card_move.is_none());
        assert_eq!(state.card_layout.order_of(SplitGroup::OverviewCols)[..2], [0, 1]);
    }

    // 3. 지표 카드도 제목 길게 누름으로 순서 변경 (모든 탭 공통 묶음)
    let mut metric = state_with(calm_snapshot(5), ActiveTab::Alerts, Lang::En);
    draw(&mut metric, 100, 30);
    let risk_title = metric
        .hit_areas
        .title_hits
        .iter()
        .find(|hit| hit.card == CardId::Risk)
        .copied()
        .unwrap();
    let last_slot = metric
        .hit_areas
        .slot_groups
        .iter()
        .find(|slots| slots.group == SplitGroup::MetricCols)
        .unwrap()
        .slots[3];
    press_at(&mut metric, risk_title.rect.x + 1, risk_title.rect.y, base);
    metric.tick(after(base, 400));
    drag_at(&mut metric, last_slot.x + 2, last_slot.y + 1, after(base, 450));
    release_at(&mut metric, last_slot.x + 2, last_slot.y + 1, after(base, 500));
    assert_eq!(metric.card_layout.order_of(SplitGroup::MetricCols), [1, 2, 3, 0]);
    draw(&mut metric, 100, 30);
    assert!(card_rect(&metric, CardId::Risk).x > card_rect(&metric, CardId::QueryRate).x);
}

// 22. 도움말 겹침 -------------------------------------------------------------
#[test]
fn help_overlay_uses_a_double_border_dims_the_backdrop_and_truncates_when_short() {
    for lang in [Lang::En, Lang::Ko] {
        for (width, height) in SIZES {
            let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, lang);
            state.handle_key_event(key(KeyCode::Char('?')));
            assert!(state.show_help);
            let buffer = draw(&mut state, width, height);
            let rows = dump(&buffer);
            let label = format!("{lang:?} {width}x{height}");

            // 1. 이중선 상자 (가운데 정렬)와 어두운 바깥
            let corner = (0..height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .find(|(x, y)| buffer[(*x, *y)].symbol() == "╔")
                .unwrap_or_else(|| panic!("{label} 이중선 모서리 없음"));
            assert_eq!(buffer[corner].fg, Color::Indexed(81), "{label}");
            let box_width = width.saturating_sub(4).min(76);
            assert_eq!(corner.0, (width - box_width) / 2, "{label} 가운데");
            assert_eq!(buffer[(1, 0)].fg, Color::Indexed(240), "{label} 바깥 어둡게");

            // 2. 구역 제목과 닫기 안내 (하단 테두리)
            let screen = rows.join("\n");
            let (keyboard, mouse_title, close) = (
                text(lang, Msg::HelpSectionKeyboard),
                text(lang, Msg::HelpSectionMouse),
                text(lang, Msg::HelpClose),
            );
            assert!(screen.contains(keyboard), "{label}");
            assert!(screen.contains(close), "{label}");
            // 3. 높이가 모자라면 마지막 줄이 잘림 표시 (키보드·마우스 두 구역은 큰 화면에서 모두)
            let truncated = screen.contains(text(lang, Msg::HelpTruncated));
            if height <= 16 {
                assert!(truncated, "{label} 잘림 줄");
            } else {
                assert!(screen.contains(mouse_title), "{label} 마우스 구역");
                assert!(!truncated, "{label}");
            }
            // 4. 왼쪽 가장자리에 걸친 넓은 글자는 지움
            for y in corner.1..corner.1 + 4 {
                if corner.0 > 0 {
                    assert!(Span::raw(buffer[(corner.0 - 1, y)].symbol()).width() <= 1, "{label}");
                }
            }
        }
    }

    // 닫기: 아무 키 또는 클릭 한 번, 그 입력은 소비
    let mut state = state_with(calm_snapshot(3), ActiveTab::Overview, Lang::En);
    state.handle_key_event(key(KeyCode::Char('?')));
    state.handle_key_event(key(KeyCode::Char('L')));
    assert!(!state.show_help);
    assert_eq!(state.lang, Lang::En);
    state.handle_key_event(key(KeyCode::Char('?')));
    draw(&mut state, 100, 30);
    let command = click_at(&mut state, 50, 10);
    assert!(!state.show_help && matches!(command, UiCommand::None));
    assert_eq!(state.tab, ActiveTab::Overview);
}

// 23. 종료 확인 겹침 ----------------------------------------------------------
#[test]
fn kill_confirm_overlay_has_critical_tone_filled_yes_tonal_no_and_clickable_buttons() {
    for lang in [Lang::En, Lang::Ko] {
        for (width, height) in SIZES {
            let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, lang);
            draw(&mut state, width, height);
            state.handle_key_event(key(KeyCode::Char('K')));
            let Some(KillConfirm::Selected(target)) = state.kill_confirm.clone() else {
                panic!("selected-process confirm window expected");
            };
            let buffer = draw(&mut state, width, height);
            let rows = dump(&buffer);
            let label = format!("{lang:?} {width}x{height}");

            let corner = (0..height)
                .flat_map(|y| (0..width).map(move |x| (x, y)))
                .find(|(x, y)| buffer[(*x, *y)].symbol() == "╔")
                .unwrap_or_else(|| panic!("{label} 확인 창 없음"));
            assert_eq!(buffer[corner].fg, Color::Indexed(203), "{label} 위험 톤");
            let screen = rows.join("\n");
            assert!(screen.contains(text(lang, Msg::KillTitle)), "{label}");
            assert!(screen.contains(&format!("(PID {})", target.pid)), "{label}");

            let yes = button_rect(&state, UiAction::ConfirmKill);
            let no = button_rect(&state, UiAction::CancelKill);
            assert_eq!(buffer[(yes.x + 1, yes.y)].bg, Color::Indexed(203), "{label} 채움 Y");
            assert_eq!(buffer[(no.x + 1, no.y)].bg, Color::Indexed(238), "{label} 톤 N");
            assert_eq!(yes.y, no.y);
            assert!(yes.right() < no.x);

            // 취소 버튼 클릭: 확인 창 닫힘, 종료 요청 없음
            let mut cancel = state_with(calm_snapshot(5), ActiveTab::Overview, lang);
            draw(&mut cancel, width, height);
            cancel.handle_key_event(key(KeyCode::Char('K')));
            draw(&mut cancel, width, height);
            let no = button_rect(&cancel, UiAction::CancelKill);
            let command = click_at(&mut cancel, no.x + 1, no.y);
            assert!(cancel.kill_confirm.is_none() && matches!(command, UiCommand::None), "{label}");
            assert!(cancel.toast.is_some());

            // 확인 버튼 클릭: 종료 요청 (프로세스 이름·PID 전달)
            let mut confirm = state_with(calm_snapshot(5), ActiveTab::Overview, lang);
            draw(&mut confirm, width, height);
            confirm.handle_key_event(key(KeyCode::Char('K')));
            draw(&mut confirm, width, height);
            let yes = button_rect(&confirm, UiAction::ConfirmKill);
            let command = click_at(&mut confirm, yes.x + 1, yes.y);
            assert!(
                matches!(command, UiCommand::KillProcess { pid, .. } if pid == target.pid),
                "{label} {command:?}"
            );
            assert!(confirm.kill_confirm.is_none());
        }
    }

    // 키: Y 만 실행, 그 밖의 키는 취소 (Esc 포함)
    let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
    draw(&mut state, 100, 30);
    state.handle_key_event(key(KeyCode::Char('K')));
    assert!(matches!(
        state.handle_key_event(key(KeyCode::Char('Y'))),
        UiCommand::KillProcess { .. }
    ));
    state.handle_key_event(key(KeyCode::Char('K')));
    assert!(matches!(state.handle_key_event(key(KeyCode::Esc)), UiCommand::None));
    assert!(state.kill_confirm.is_none() && !state.should_quit);

    // 확인 창 뒤 카드 클릭은 무시 (Esc 로 닫기 전까지)
    state.handle_key_event(key(KeyCode::Char('K')));
    draw(&mut state, 100, 30);
    let sockets = card_rect(&state, CardId::Sockets);
    click_at(&mut state, sockets.x + 3, sockets.y + 3);
    assert!(state.kill_confirm.is_some());
    assert_eq!(state.focused_card_id(), CardId::Processes);
}

// 24. 빈 상태·결과 안내 -------------------------------------------------------
#[test]
fn empty_tables_show_a_headline_and_next_step_in_the_active_language() {
    for lang in [Lang::En, Lang::Ko] {
        let mut snap = calm_snapshot(0);
        snap.processes.clear();
        snap.connections.clear();
        snap.recent_events.clear();
        snap.top_sql.clear();
        snap.alerts.clear();
        for (tab, headline, hint) in [
            (ActiveTab::Processes, Msg::EmptyProcesses, Msg::EmptyProcessesHint),
            (ActiveTab::TopSql, Msg::EmptyTopSql, Msg::EmptyTopSqlHint),
            (ActiveTab::Alerts, Msg::EmptyAlerts, Msg::EmptyAlertsHint),
        ] {
            let mut state = state_with(snap.clone(), tab, lang);
            let screen = dump(&draw(&mut state, 130, 40)).join("\n");
            assert!(screen.contains(text(lang, headline)), "{lang:?} {tab:?}");
            assert!(screen.contains(text(lang, hint)), "{lang:?} {tab:?}");
            assert!(state.hit_areas.row_hits.iter().all(|hit| hit.len == 0));
        }
    }
}

// 25. 렌더·호버는 저장하지 않음 -----------------------------------------------
#[test]
fn rendering_and_hovering_never_write_the_state_file_but_a_commit_does_into_the_injected_path() {
    let dir = tempfile::TempDir::new().unwrap();
    let path = dir.path().join("tui.toml");
    let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
    state.state_file = Some(path.clone());
    let base = Instant::now();
    for (index, (width, height)) in SIZES.iter().enumerate() {
        draw(&mut state, *width, *height);
        state.handle_mouse_event_at(
            mouse(MouseEventKind::Moved, 20 + index as u16, 10),
            after(base, 10 * index as u64),
        );
    }
    state.handle_key_event(key(KeyCode::Down));
    state.handle_mouse_event_at(mouse(MouseEventKind::ScrollDown, 20, 10), base);
    assert!(!path.exists(), "렌더·호버·스크롤·선택은 저장하지 않음");

    // 확정 이벤트(언어 전환)는 SaveState 를 요청하고, 저장하면 주입한 경로에만 기록
    let command = state.handle_key_event(key(KeyCode::Char('L')));
    assert!(matches!(command, UiCommand::SaveState));
    assert!(!path.exists());
    let (ui_tx, _ui_rx) = crossbeam_channel::unbounded::<UiMessage>();
    let mut writer = spawn_state_writer(ui_tx).unwrap();
    writer.queue_state_save(state.build_state_save().unwrap().unwrap()).unwrap();
    assert_eq!(writer.finish_state_saves(STATE_SAVE_JOIN_LIMIT), SaveFinish::Clean);
    let saved = fs::read_to_string(&path).unwrap();
    assert!(saved.contains("version = 1") && saved.contains("language = \"ko\""), "{saved}");
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 1);
}

// 26. 위험 열 우선 표시 -------------------------------------------------------
fn header_text(state: &AppState, buffer: &Buffer, card: CardId, column: usize) -> Option<String> {
    state
        .hit_areas
        .header_cells
        .iter()
        .find(|hit| hit.card == card && hit.column == column)
        .map(|hit| cells_text(buffer, hit.rect))
}

#[test]
fn risk_column_stays_visible_at_100x30_and_60x16_in_both_languages() {
    let snap = critical_snapshot();
    let agent = &snap.processes[0];
    for lang in [Lang::En, Lang::Ko] {
        for (width, height) in [(60u16, 16u16), (100, 30)] {
            let label = format!("{lang:?} {width}x{height}");

            // 1. 프로세스 표 (통합관제·에이전트 탭 공통): 위험 머리글과 첫 행 값
            for tab in [ActiveTab::Overview, ActiveTab::Processes] {
                let mut state = state_with(snap.clone(), tab, lang);
                let buffer = draw(&mut state, width, height);
                let header = header_text(&state, &buffer, CardId::Processes, 8);
                assert_eq!(header.as_deref(), Some(text(lang, Msg::ColRisk)), "{label} {tab:?} 위험 머리글");
                let hit = state
                    .hit_areas
                    .header_cells
                    .iter()
                    .find(|hit| hit.card == CardId::Processes && hit.column == 8)
                    .unwrap();
                let cell = cells_text(&buffer, Rect::new(hit.rect.x, hit.rect.y + 1, hit.rect.width, 1));
                assert_eq!(cell, risk_cell_text(lang, agent.risk_level, agent.risk_score), "{label} {tab:?}");
            }

            // 2. 소켓 표: 위험 열은 끝까지 남음
            let mut state = state_with(snap.clone(), ActiveTab::Overview, lang);
            let buffer = draw(&mut state, width, height);
            let socket = header_text(&state, &buffer, CardId::Sockets, 5);
            assert_eq!(socket.as_deref(), Some(text(lang, Msg::ColRiskShort)), "{label} 소켓 위험 머리글");

            // 3. 과조회SQL·위험경보 표의 등급 열
            let mut state = state_with(snap.clone(), ActiveTab::TopSql, lang);
            let buffer = draw(&mut state, width, height);
            let top = header_text(&state, &buffer, CardId::TopSqlRanking, 0);
            assert_eq!(top.as_deref(), Some(text(lang, Msg::ColRisk)), "{label} 랭킹 위험 머리글");
            let mut state = state_with(snap.clone(), ActiveTab::Alerts, lang);
            let buffer = draw(&mut state, width, height);
            let level = header_text(&state, &buffer, CardId::AlertLog, 1);
            assert_eq!(level.as_deref(), Some(text(lang, Msg::ColLevel)), "{label} 경보 등급 머리글");
        }
    }
}

// 27. 사유 문구는 값에서 표시 언어로 ------------------------------------------
#[test]
fn inspector_and_alert_views_show_the_typed_reason_in_both_languages_at_60x16_and_130x40() {
    let snap = critical_snapshot();
    let reason = snap.processes[0].risk_reasons[0];
    assert!(matches!(reason, RiskReason::RxSustained { .. }));
    assert_eq!(snap.alerts[0].detail, snap.processes[0].risk_reasons);
    for lang in [Lang::En, Lang::Ko] {
        let expected = reason_text(lang, &reason);
        let (literal, sql_literal) = match lang {
            Lang::En => (
                "Sustained bulk query (10s avg 4.80 MB/s)",
                "12x in 10s | SELECT * FROM TB_PAYMENT_SETTLEMENT",
            ),
            Lang::Ko => (
                "대량 조회 지속 (10초 평균 4.80 MB/s)",
                "10초 내 12회 | SELECT * FROM TB_PAYMENT_SETTLEMENT",
            ),
        };
        assert_eq!(expected, literal, "{lang:?} 카탈로그 문구");
        let token = literal.split_whitespace().next().unwrap();
        for (width, height) in [(60u16, 16u16), (130, 40)] {
            let label = format!("{lang:?} {width}x{height}");

            // 1. 인스펙터: 진단 줄 (좁은 크기는 한 줄씩 스크롤해 줄 바꿈된 조각 확인)
            let mut state = state_with(snap.clone(), ActiveTab::Overview, lang);
            let mut inspector_rows = Vec::new();
            let mut screens = Vec::new();
            for scroll in 0..8 {
                state.card_views[CardId::Inspector as usize].scroll = scroll;
                let buffer = draw(&mut state, width, height);
                let inspector = card_rect(&state, CardId::Inspector);
                let rows = dump(&buffer);
                inspector_rows.extend_from_slice(
                    &rows[usize::from(inspector.y) + 1..usize::from(inspector.bottom()) - 1],
                );
                screens.push(rows.join("\n"));
            }
            let joined = inspector_rows.join("\n");
            if width >= 130 {
                let line = format!("{}  [{}] {literal}", text(lang, Msg::LabelDiagnosis), i18n_risk(lang));
                assert!(joined.contains(&line), "{label} 인스펙터 진단 줄: {joined}");
            } else {
                assert!(joined.contains(token), "{label} 인스펙터 사유 조각: {joined}");
            }

            // 2. 위험경보 탭: 경보 로그 행, 선택 경보 상세 (사유·최근 SQL)
            let mut alerts = state_with(snap.clone(), ActiveTab::Alerts, lang);
            let buffer = draw(&mut alerts, width, height);
            let rows = dump(&buffer);
            screens.push(rows.join("\n"));
            let table = card_rect(&alerts, CardId::AlertLog);
            let first_row = &rows[usize::from(table.y) + 2];
            assert!(first_row.contains("OVER_QUERY"), "{label} 규칙: {first_row}");
            assert!(first_row.contains(token), "{label} 경보 로그 사유: {first_row}");
            if width >= 130 {
                assert!(first_row.contains(literal), "{label} 경보 로그 전체 사유: {first_row}");
                let detail = card_rect(&alerts, CardId::AlertDetail);
                let detail_rows =
                    rows[usize::from(detail.y) + 1..usize::from(detail.bottom()) - 1].join("\n");
                let reason_line = format!("{}  {literal}", text(lang, Msg::LabelReason));
                assert!(detail_rows.contains(&reason_line), "{label} 상세 사유: {detail_rows}");
                assert!(detail_rows.contains(sql_literal), "{label} 상세 최근 SQL: {detail_rows}");
                let summary = i18n::summary_text(lang, &snap.risk_summary);
                assert!(rows.iter().any(|row| row.contains(&summary)), "{label} 사이렌 요약");
            }

            // 3. 영어 화면에는 한글이 없고 한국어 화면에는 영어 사유가 없음
            for screen in &screens {
                match lang {
                    Lang::En => assert!(!has_hangul(screen), "{label} 영어 화면에 한글"),
                    Lang::Ko => {
                        assert!(has_hangul(screen), "{label} 한국어 화면");
                        assert!(!screen.contains("Sustained"), "{label} 한국어 화면에 영어 사유");
                    }
                }
            }
        }
    }
}

fn i18n_risk(lang: Lang) -> &'static str {
    i18n::risk_label(lang, RiskLevel::Critical)
}

// 28. 사유 열 정렬은 값 기준 --------------------------------------------------
#[test]
fn diagnosis_columns_sort_by_typed_reason_so_the_language_never_reorders_rows() {
    let mut snap = critical_snapshot();
    let reasons = [
        Some(RiskReason::RepeatSql { repeat: 7 }),
        Some(RiskReason::DestructiveDdl),
        None,
        Some(RiskReason::RxSustained { avg_rx_bps: 9 }),
        Some(RiskReason::FullScan),
    ];
    let template = snap.top_sql[0].clone();
    snap.top_sql = reasons
        .iter()
        .enumerate()
        .map(|(index, reason)| {
            let mut item = template.clone();
            item.fingerprint = format!("fp-{index}");
            item.risk_reason = *reason;
            item
        })
        .collect();
    let mut state = state_with(snap, ActiveTab::TopSql, Lang::En);
    state.sorts.set(SortTable::TopSql, Some(SortKey { column: 8, descending: false }));
    let order = |state: &AppState| -> Vec<Option<RiskReason>> {
        let snap = state.snapshot.as_ref().unwrap();
        sort::top_sql(snap, &state.sorts).iter().map(|row| row.risk_reason).collect()
    };
    let english = order(&state);
    assert!(english.windows(2).all(|pair| pair[0] <= pair[1]), "{english:?}");
    state.lang = Lang::Ko;
    assert_eq!(order(&state), english);
    state.sorts.set(SortTable::TopSql, Some(SortKey { column: 8, descending: true }));
    let reversed = order(&state);
    assert!(reversed.windows(2).all(|pair| pair[0] >= pair[1]), "{reversed:?}");
}

// 29. 소스 규칙 ---------------------------------------------------------------
fn tui_sources(dir: &Path, found: &mut Vec<std::path::PathBuf>) {
    for entry in fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            tui_sources(&path, found);
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs") {
            found.push(path);
        }
    }
}

// 시험 모듈 앞까지의 본문 (#[cfg(test)], #[cfg(all(test, ...))])
fn production_part(content: &str) -> String {
    content
        .lines()
        .take_while(|line| {
            let trimmed = line.trim_start();
            !(trimmed.starts_with("#[cfg(test)]") || trimmed.starts_with("#[cfg(all(test"))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn indexed_colors_live_only_in_theme_and_view_modules_hold_no_inline_korean_text() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join("tui");
    let mut files = Vec::new();
    tui_sources(&root, &mut files);
    for path in files {
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let content = fs::read_to_string(&path).unwrap();
        let production = production_part(&content);
        let in_views = path.components().any(|part| part.as_os_str() == "views");

        // 1. 색 인덱스·RGB·이름 색은 theme.rs 에만
        if name != "theme.rs" {
            for (index, line) in production.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                assert!(
                    !code.contains("Color::Indexed(") && !code.contains("Color::Rgb(") && !code.contains("Color::Red"),
                    "{}:{} 테마 밖 색",
                    path.display(),
                    index + 1
                );
            }
        }

        // 2. 화면 문구는 카탈로그: 뷰·상태·상호작용·카드 모듈의 코드 줄에는 한글 리터럴이 없음
        if in_views || matches!(name.as_str(), "state.rs" | "interaction.rs" | "cards.rs") {
            for (index, line) in production.lines().enumerate() {
                let code = line.split("//").next().unwrap_or("");
                assert!(!has_hangul(code), "{}:{} 한글 리터럴: {line}", path.display(), index + 1);
            }
        }
    }
}

// 30. 도메인 문구 소스 규칙 ---------------------------------------------------
#[test]
fn risk_collector_and_every_other_source_hold_no_korean_literal_outside_the_catalog() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    tui_sources(&root, &mut files);
    let mut scanned = 0;
    for path in files {
        if path.file_name().is_some_and(|name| name == "i18n.rs") {
            continue;
        }
        let content = fs::read_to_string(&path).unwrap();
        for (index, line) in production_part(&content).lines().enumerate() {
            let code = line.split("//").next().unwrap_or("");
            assert!(!has_hangul(code), "{}:{} 카탈로그 밖 한글 리터럴: {line}", path.display(), index + 1);
        }
        scanned += 1;
    }
    let collector = root.join("collector");
    assert!(collector.join("mod.rs").is_file() && root.join("analysis").join("risk.rs").is_file());
    assert!(scanned >= 20, "검사한 파일 수 {scanned}");
}

// 31. 최소 크기 미달 파괴적 키 차단 -------------------------------------------
#[test]
fn below_the_size_gate_destructive_keys_are_ignored_and_a_shrink_cancels_an_open_confirm() {
    let nothing_acts = |state: &mut AppState, label: &str| {
        for code in [KeyCode::Char('K'), KeyCode::Char('x'), KeyCode::F(9), KeyCode::F(12)] {
            let command = state.handle_key_event(key(code));
            assert!(matches!(command, UiCommand::None), "{label} {code:?}");
            assert!(state.kill_confirm.is_none(), "{label} {code:?}");
        }
        let command = state.handle_key_event(key(KeyCode::Char('y')));
        assert!(matches!(command, UiCommand::None), "{label} y");
        assert!(state.toast.is_none(), "{label} {:?}", state.toast);
    };

    // 1. 59x15 등 한 쪽라도 모자라면 K · x · F9 · F12 · y 모두 무시 (버퍼에 게이트 줄만 그려짐)
    for lang in [Lang::En, Lang::Ko] {
        for (width, height) in [(59, 15), (59, 16), (60, 15)] {
            let label = format!("{lang:?} {width}x{height}");
            let mut state = state_with(critical_snapshot(), ActiveTab::Overview, lang);
            let buffer = draw(&mut state, width, height);
            let screen = dump(&buffer).join("\n");
            assert!(screen.contains(&i18n::too_small(lang, 60, 16, width, height)), "{label}");
            assert!(state.hit_areas.button_rects.is_empty(), "{label} 클릭 영역 없음");
            nothing_acts(&mut state, &label);
        }
    }

    // 2. 기준 크기 60x16 에서는 같은 키가 확인 창을 염
    let mut state = state_with(critical_snapshot(), ActiveTab::Overview, Lang::En);
    draw(&mut state, 60, 16);
    state.handle_key_event(key(KeyCode::Char('K')));
    assert!(matches!(state.kill_confirm, Some(KillConfirm::Selected(_))));
    state.handle_key_event(key(KeyCode::Esc));
    state.handle_key_event(key(KeyCode::Char('x')));
    assert!(matches!(state.kill_confirm, Some(KillConfirm::Emergency(_))));

    // 3. 확인 창이 열린 채 줄어들면 취소 (그리기 경로, 크기 변경 이벤트 경로), 이후 y 는 아무 일도 하지 않음
    for opener in ['K', 'x'] {
        for resize_event in [false, true] {
            let label = format!("{opener} resize_event={resize_event}");
            let mut state = state_with(critical_snapshot(), ActiveTab::Overview, Lang::En);
            draw(&mut state, 100, 30);
            state.handle_key_event(key(KeyCode::Char(opener)));
            assert!(state.kill_confirm.is_some(), "{label}");
            if resize_event {
                state.handle_resize(59, 15);
            } else {
                draw(&mut state, 59, 15);
            }
            assert!(state.kill_confirm.is_none(), "{label} 취소");
            let toast = state.toast.as_ref().expect("취소 알림");
            assert_eq!(toast.text, text(Lang::En, Msg::ToastKillCancelled), "{label}");
            let command = state.handle_key_event(key(KeyCode::Char('y')));
            assert!(matches!(command, UiCommand::None), "{label} y");

            // 다시 충분히 커지면 정상 동작
            draw(&mut state, 100, 30);
            state.handle_key_event(key(KeyCode::Char(opener)));
            assert!(state.kill_confirm.is_some(), "{label} 복구");
        }
    }

    // 4. 남아 있는 확인에 대한 y 는 실행하지 않고 취소
    let mut state = state_with(critical_snapshot(), ActiveTab::Overview, Lang::En);
    draw(&mut state, 59, 15);
    let target = state.snapshot.as_ref().unwrap().processes[0].clone();
    state.kill_confirm = Some(KillConfirm::Selected(Box::new(target)));
    let command = state.handle_key_event(key(KeyCode::Char('y')));
    assert!(matches!(command, UiCommand::None) && state.kill_confirm.is_none());
}

// 32. 긴급 중단 확인 ----------------------------------------------------------
#[test]
fn emergency_stop_opens_a_critical_confirm_with_targets_and_runs_only_after_y() {
    let snap = critical_snapshot();
    let targets: Vec<_> = snap
        .processes
        .iter()
        .filter(|process| process.risk_level >= RiskLevel::Critical)
        .cloned()
        .collect();
    assert!(!targets.is_empty());

    // 1. x · F9 · F12 · 경보 카드 · 칩 모두 같은 확인 창 (대상 수와 이름, 채움 Y 와 톤 N)
    for lang in [Lang::En, Lang::Ko] {
        for (width, height) in SIZES {
            for trigger in ["x", "F9", "F12", "siren", "chip"] {
                let label = format!("{lang:?} {width}x{height} {trigger}");
                let mut state = state_with(snap.clone(), ActiveTab::Overview, lang);
                draw(&mut state, width, height);
                let command = match trigger {
                    "x" => state.handle_key_event(key(KeyCode::Char('x'))),
                    "F9" => state.handle_key_event(key(KeyCode::F(9))),
                    "F12" => state.handle_key_event(key(KeyCode::F(12))),
                    "siren" => {
                        let siren = button_rect(&state, UiAction::EmergencyStop);
                        click_at(&mut state, siren.x + 5, siren.y + siren.height / 2)
                    }
                    _ => {
                        let chip = chip_rect(&state, UiAction::EmergencyStop);
                        click_at(&mut state, chip.x + 1, chip.y)
                    }
                };
                assert!(matches!(command, UiCommand::None), "{label}");
                let Some(KillConfirm::Emergency(list)) = state.kill_confirm.clone() else {
                    panic!("{label} emergency confirm expected");
                };
                assert_eq!(list.len(), targets.len(), "{label}");

                let buffer = draw(&mut state, width, height);
                let screen = dump(&buffer).join("\n");
                let corner = (0..height)
                    .flat_map(|y| (0..width).map(move |x| (x, y)))
                    .find(|(x, y)| buffer[(*x, *y)].symbol() == "╔")
                    .unwrap_or_else(|| panic!("{label} 확인 창 없음"));
                assert_eq!(buffer[corner].fg, Color::Indexed(203), "{label} 위험 톤");
                assert!(screen.contains(text(lang, Msg::StopTitle)), "{label}");
                assert!(screen.contains(&i18n::stop_target_count(lang, targets.len())), "{label}");
                assert!(screen.contains(&targets[0].process_name), "{label}");
                assert!(screen.contains(&format!("(PID {})", targets[0].pid)), "{label}");
                let yes = button_rect(&state, UiAction::ConfirmKill);
                let no = button_rect(&state, UiAction::CancelKill);
                assert_eq!(buffer[(yes.x + 1, yes.y)].bg, Color::Indexed(203), "{label} 채움 Y");
                assert_eq!(buffer[(no.x + 1, no.y)].bg, Color::Indexed(238), "{label} 톤 N");
            }
        }
    }

    // 2. Y 만 실행 (위험 등급 목록을 수집 스레드로), 확인 버튼 클릭도 같음
    let mut state = state_with(snap.clone(), ActiveTab::Overview, Lang::En);
    draw(&mut state, 100, 30);
    state.handle_key_event(key(KeyCode::Char('x')));
    let command = state.handle_key_event(key(KeyCode::Char('Y')));
    assert!(
        matches!(command, UiCommand::EmergencyStop(ref list) if list.len() == targets.len()
            && list.iter().all(|process| process.risk_level >= RiskLevel::Critical)),
        "{command:?}"
    );
    assert!(state.kill_confirm.is_none());
    assert_eq!(state.toast.as_ref().unwrap().text, text(Lang::En, Msg::ToastStopRequested));
    state.handle_key_event(key(KeyCode::Char('x')));
    draw(&mut state, 100, 30);
    let yes = button_rect(&state, UiAction::ConfirmKill);
    let command = click_at(&mut state, yes.x + 1, yes.y);
    assert!(matches!(command, UiCommand::EmergencyStop(_)), "{command:?}");

    // 3. 그 밖의 키(Esc · N · 다른 모든 키)와 N 버튼은 취소, 중단 요청은 나가지 않음
    for cancel in [KeyCode::Esc, KeyCode::Char('n'), KeyCode::Char('N'), KeyCode::Char('x'), KeyCode::Enter] {
        let mut state = state_with(snap.clone(), ActiveTab::Overview, Lang::En);
        draw(&mut state, 100, 30);
        state.handle_key_event(key(KeyCode::Char('x')));
        let command = state.handle_key_event(key(cancel));
        assert!(matches!(command, UiCommand::None), "{cancel:?}");
        assert!(state.kill_confirm.is_none(), "{cancel:?}");
    }
    let mut state = state_with(snap.clone(), ActiveTab::Overview, Lang::En);
    draw(&mut state, 100, 30);
    state.handle_key_event(key(KeyCode::Char('x')));
    draw(&mut state, 100, 30);
    let no = button_rect(&state, UiAction::CancelKill);
    let command = click_at(&mut state, no.x + 1, no.y);
    assert!(matches!(command, UiCommand::None) && state.kill_confirm.is_none());

    // 4. 위험 프로세스가 없으면 확인 창 없이 안내 알림, 첫 집계 전에는 아무 일도 없음
    let mut calm = snap.clone();
    for process in &mut calm.processes {
        process.risk_level = RiskLevel::Warning;
    }
    let mut state = state_with(calm, ActiveTab::Overview, Lang::En);
    draw(&mut state, 100, 30);
    let command = state.handle_key_event(key(KeyCode::Char('x')));
    assert!(matches!(command, UiCommand::None) && state.kill_confirm.is_none());
    assert_eq!(state.toast.as_ref().unwrap().text, text(Lang::En, Msg::ToastNoCritical));
    let mut waiting = AppState::new(1000);
    let command = waiting.handle_key_event(key(KeyCode::Char('x')));
    assert!(matches!(command, UiCommand::None) && waiting.kill_confirm.is_none() && waiting.toast.is_none());
}

// 33. k · j 이동과 K 종료 -----------------------------------------------------
#[test]
fn k_and_j_move_the_selection_like_the_arrows_and_only_upper_k_opens_the_kill_confirm() {
    let mut state = state_with(calm_snapshot(5), ActiveTab::Processes, Lang::En);
    draw(&mut state, 100, 30);
    let selected = |state: &AppState| state.card_views[CardId::Processes as usize].selected;
    assert!(state.card_views[CardId::Processes as usize].total >= 3);
    assert_eq!(selected(&state), 0);
    state.handle_key_event(key(KeyCode::Char('j')));
    state.handle_key_event(key(KeyCode::Char('j')));
    assert_eq!(selected(&state), 2);
    state.handle_key_event(key(KeyCode::Char('k')));
    assert_eq!(selected(&state), 1);
    state.handle_key_event(key(KeyCode::Up));
    state.handle_key_event(key(KeyCode::Char('k')));
    assert_eq!(selected(&state), 0, "첫 행에서는 더 위로 가지 않음");
    assert!(state.kill_confirm.is_none(), "k 는 종료 확인을 열지 않음");

    // K (Shift 표시 있음·없음 둘 다)만 선택 프로세스 종료 확인
    state.handle_key_event(shift_key(KeyCode::Char('K')));
    assert!(matches!(state.kill_confirm, Some(KillConfirm::Selected(_))));
    state.handle_key_event(key(KeyCode::Esc));
    state.handle_key_event(key(KeyCode::Char('K')));
    assert!(matches!(state.kill_confirm, Some(KillConfirm::Selected(_))));

    // 도움말과 하단 안내가 새 키를 보여 줌 (두 언어, 130x40)
    for lang in [Lang::En, Lang::Ko] {
        let mut state = state_with(calm_snapshot(5), ActiveTab::Processes, lang);
        let screen = dump(&draw(&mut state, 130, 40)).join("\n");
        assert!(screen.contains(text(lang, Msg::HintKill)), "{lang:?} 하단 안내");
        state.handle_key_event(key(KeyCode::Char('?')));
        let screen = dump(&draw(&mut state, 130, 40)).join("\n");
        assert!(screen.contains(text(lang, Msg::HelpKill)), "{lang:?} 도움말 K");
        assert!(screen.contains(text(lang, Msg::HelpMoveKj)), "{lang:?} 도움말 k/j");
        assert!(screen.contains(text(lang, Msg::HelpStop)), "{lang:?} 도움말 x");
    }
}

// 34. 놓기를 놓친 드래그 ------------------------------------------------------
#[test]
fn lost_release_ends_split_column_and_scroll_drags_on_a_buttonless_move_with_an_injected_clock() {
    let base = Instant::now();
    let moved_event = |column: u16, row: u16| mouse(MouseEventKind::Moved, column, row);

    // 1. 카드 경계: 움직인 드래그는 버튼 없는 움직임에서 확정·저장 (Up 이 와도 같음)
    let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
    draw(&mut state, 130, 40);
    let hit = state
        .hit_areas
        .split_hits
        .iter()
        .find(|hit| hit.group == SplitGroup::OverviewCols)
        .copied()
        .unwrap();
    press_at(&mut state, hit.grab.x + 1, hit.grab.y + 5, base);
    drag_at(&mut state, hit.grab.x + 13, hit.grab.y + 5, after(base, 20));
    assert!(state.split_drag.is_some_and(|drag| drag.moved));
    let command = state.handle_mouse_event_at(moved_event(hit.grab.x + 14, hit.grab.y + 5), after(base, 40));
    assert!(matches!(command, UiCommand::SaveState));
    assert!(state.split_drag.is_none());
    draw(&mut state, 130, 40);
    let moved = state
        .hit_areas
        .split_hits
        .iter()
        .find(|hit| hit.group == SplitGroup::OverviewCols)
        .copied()
        .unwrap();
    assert_eq!(moved.line.x, hit.line.x + 12);
    // 끝난 뒤의 드래그 이벤트는 경계를 더 움직이지 못함
    drag_at(&mut state, hit.grab.x + 30, hit.grab.y + 5, after(base, 60));
    draw(&mut state, 130, 40);
    let after_drag = state
        .hit_areas
        .split_hits
        .iter()
        .find(|hit| hit.group == SplitGroup::OverviewCols)
        .copied()
        .unwrap();
    assert_eq!(after_drag.line.x, moved.line.x);
    // 움직이지 않은 경계 누름은 저장 요청 없이 끝남
    press_at(&mut state, moved.grab.x + 1, moved.grab.y + 5, after(base, 100));
    assert!(state.split_drag.is_some_and(|drag| !drag.moved));
    let command = state.handle_mouse_event_at(moved_event(moved.grab.x + 2, moved.grab.y + 5), after(base, 120));
    assert!(matches!(command, UiCommand::None) && state.split_drag.is_none());

    // 2. 열 폭: 움직인 드래그는 확정·저장, 이후 드래그는 폭을 바꾸지 못함
    let mut state = state_with(calm_snapshot(5), ActiveTab::Processes, Lang::Ko);
    draw(&mut state, 130, 40);
    let grab = state
        .hit_areas
        .column_hits
        .iter()
        .find(|hit| hit.card == CardId::Processes && hit.column == 2)
        .copied()
        .unwrap();
    press_at(&mut state, grab.grab.x, grab.grab.y, base);
    drag_at(&mut state, grab.grab.x + 4, grab.grab.y, after(base, 20));
    assert!(state.column_drag.is_some_and(|drag| drag.moved));
    let command = state.handle_mouse_event_at(moved_event(grab.grab.x + 5, grab.grab.y), after(base, 40));
    assert!(matches!(command, UiCommand::SaveState));
    assert!(state.column_drag.is_none());
    assert_eq!(state.column_widths[CardId::Processes as usize][2], Some(grab.width + 4));
    drag_at(&mut state, grab.grab.x + 9, grab.grab.y, after(base, 60));
    assert_eq!(state.column_widths[CardId::Processes as usize][2], Some(grab.width + 4));

    // 3. 스크롤바: 썸 드래그는 버튼 없는 움직임에서 끝남 (저장할 것 없음), 이후 드래그는 스크롤을 바꾸지 못함
    let mut state = state_with(calm_snapshot(60), ActiveTab::Overview, Lang::En);
    draw(&mut state, 130, 40);
    let track = state
        .hit_areas
        .scroll_hits
        .iter()
        .find(|hit| hit.card == CardId::Sockets)
        .copied()
        .unwrap();
    let thumb = scroll_thumb(track.total, track.visible, track.scroll, track.track.height).unwrap();
    press_at(&mut state, track.track.x, track.track.y + thumb.start, base);
    drag_at(&mut state, track.track.x, track.track.y + 3, after(base, 20));
    let scrolled = state.card_views[CardId::Sockets as usize].scroll;
    assert!(state.scroll_drag.is_some() && scrolled > 0);
    let command = state.handle_mouse_event_at(moved_event(track.track.x, track.track.y + 3), after(base, 40));
    assert!(matches!(command, UiCommand::None));
    assert!(state.scroll_drag.is_none());
    drag_at(&mut state, track.track.x, track.track.bottom() + 5, after(base, 60));
    assert_eq!(state.card_views[CardId::Sockets as usize].scroll, scrolled);
}

#[test]
fn a_new_press_resets_every_drag_state_first_and_a_tab_switch_commits_a_moved_drag() {
    let base = Instant::now();
    let split_setup = || {
        let mut state = state_with(calm_snapshot(5), ActiveTab::Overview, Lang::En);
        draw(&mut state, 130, 40);
        let hit = state
            .hit_areas
            .split_hits
            .iter()
            .find(|hit| hit.group == SplitGroup::OverviewCols)
            .copied()
            .unwrap();
        (state, hit)
    };

    // 1. 움직인 드래그 뒤 놓기를 놓친 새 누름: 저장 요청으로 끝내고 그 누름은 클릭으로 실행하지 않음 (탭 바꿈 안 됨)
    let (mut state, hit) = split_setup();
    press_at(&mut state, hit.grab.x + 1, hit.grab.y + 5, base);
    drag_at(&mut state, hit.grab.x + 13, hit.grab.y + 5, after(base, 20));
    let tab = state.hit_areas.tab_rects.iter().find(|(tab, _)| *tab == ActiveTab::Alerts).map(|(_, rect)| *rect).unwrap();
    let command = press_at(&mut state, tab.x + 1, tab.y, after(base, 40));
    assert!(matches!(command, UiCommand::SaveState));
    assert!(state.split_drag.is_none() && state.tab == ActiveTab::Overview);

    // 2. 움직이지 않은 드래그(스크롤 썸·열 경계)는 새 누름이 날려 버리고 그 누름은 평소처럼 동작
    let mut state = state_with(calm_snapshot(60), ActiveTab::Overview, Lang::En);
    draw(&mut state, 130, 40);
    let track = state
        .hit_areas
        .scroll_hits
        .iter()
        .find(|hit| hit.card == CardId::Sockets)
        .copied()
        .unwrap();
    let thumb = scroll_thumb(track.total, track.visible, track.scroll, track.track.height).unwrap();
    press_at(&mut state, track.track.x, track.track.y + thumb.start, base);
    assert!(state.scroll_drag.is_some());
    let title = state.hit_areas.title_hits.iter().find(|hit| hit.card == CardId::Processes).copied().unwrap();
    let command = press_at(&mut state, title.rect.x + 1, title.rect.y, after(base, 50));
    assert!(matches!(command, UiCommand::None));
    assert!(state.scroll_drag.is_none(), "제목 누름도 이전 썸 드래그를 남기지 않음");
    assert!(state.card_press.is_some());

    // 3. 탭 전환(키 Tab)이 이미 움직인 경계 드래그를 확정·저장
    let (mut state, hit) = split_setup();
    press_at(&mut state, hit.grab.x + 1, hit.grab.y + 5, base);
    drag_at(&mut state, hit.grab.x + 13, hit.grab.y + 5, after(base, 20));
    let command = state.handle_key_event(key(KeyCode::Tab));
    assert!(matches!(command, UiCommand::SaveState));
    assert!(state.split_drag.is_none() && state.tab == ActiveTab::Processes);

    // 4. 탭 전환이 움직인 열 폭 드래그도 확정·저장, 움직이지 않았으면 저장 없이 끝
    let mut state = state_with(calm_snapshot(5), ActiveTab::Processes, Lang::En);
    draw(&mut state, 130, 40);
    let grab = state
        .hit_areas
        .column_hits
        .iter()
        .find(|hit| hit.card == CardId::Processes && hit.column == 2)
        .copied()
        .unwrap();
    press_at(&mut state, grab.grab.x, grab.grab.y, base);
    drag_at(&mut state, grab.grab.x + 3, grab.grab.y, after(base, 20));
    let command = state.switch_tab(ActiveTab::Alerts);
    assert!(matches!(command, UiCommand::SaveState));
    assert!(state.column_drag.is_none() && state.tab == ActiveTab::Alerts);
    assert_eq!(state.column_widths[CardId::Processes as usize][2], Some(grab.width + 3));
    let mut state = state_with(calm_snapshot(5), ActiveTab::Processes, Lang::En);
    draw(&mut state, 130, 40);
    press_at(&mut state, grab.grab.x, grab.grab.y, base);
    let command = state.handle_key_event(key(KeyCode::Char('1')));
    assert!(matches!(command, UiCommand::None));
    assert!(state.column_drag.is_none() && state.tab == ActiveTab::Overview);
}
