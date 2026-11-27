//! tui_grid.rs
//! 탭별 카드 격자 검증: 크기·언어 행렬 배치, 차트 줄 표시, 경보 띠, 길게 누름·끌기 이동(시계 주입), Shift+방향키, 탭별 초기화, 포커스 색 예약.

#[allow(dead_code)]
mod common;

use common::{bloat_snapshot, build_fixture_snapshot};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
use database_monitor::app::model::{MonitorSnapshot, RiskLevel};
use database_monitor::tui::cards::{CARD_HOLD_TIME, CardId, UiAction};
use database_monitor::tui::i18n::{self, Lang, Msg};
use database_monitor::tui::layout::CardLayout;
use database_monitor::tui::layout::grid::{DropTarget, TabGrid, apply_drop, default_rows};
use database_monitor::tui::state::{ActiveTab, AppState, UiCommand};
use database_monitor::tui::theme;
use database_monitor::tui::views::render_frame;
use ratatui::Terminal;
use ratatui::backend::TestBackend;
use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::text::Span;
use std::sync::Arc;
use std::time::{Duration, Instant};

const SIZES: [(u16, u16); 3] = [(60, 16), (100, 30), (130, 40)];
const LANGS: [Lang; 2] = [Lang::En, Lang::Ko];
const METRICS: [CardId; 4] = [
    CardId::Risk,
    CardId::Connections,
    CardId::Traffic,
    CardId::QueryRate,
];

// 1. 공용 도구 ----------------------------------------------------------------
// 위험 경보 띠 없는 스냅샷 (경고 등급, 행 수 확장)
fn calm_snapshot() -> Arc<MonitorSnapshot> {
    let mut snap = bloat_snapshot(&build_fixture_snapshot(true), 5);
    snap.overall_risk = RiskLevel::Warning;
    Arc::new(snap)
}

fn state_with(snap: &Arc<MonitorSnapshot>, tab: ActiveTab, lang: Lang) -> AppState {
    let mut state = AppState::new(1000);
    state.tab = tab;
    state.lang = lang;
    state.snapshot = Some(Arc::clone(snap));
    state.snapshot_at = Some(state.now);
    state
}

fn overview_state() -> AppState {
    state_with(&calm_snapshot(), ActiveTab::Overview, Lang::En)
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

fn after(base: Instant, millis: u64) -> Instant {
    base + Duration::from_millis(millis)
}

fn pointer(state: &mut AppState, kind: MouseEventKind, at: Position, now: Instant) -> UiCommand {
    let event = MouseEvent {
        kind,
        column: at.x,
        row: at.y,
        modifiers: KeyModifiers::NONE,
    };
    state.handle_mouse_event_at(event, now)
}

fn drawn_rect(state: &AppState, card: CardId) -> Rect {
    state
        .hit_areas
        .card_rects
        .iter()
        .find(|(drawn, _)| *drawn == card)
        .map(|(_, rect)| *rect)
        .unwrap_or_else(|| panic!("{card:?} 카드 영역 없음"))
}

// 카드가 그려진 화면 줄 영역
fn line_rect(state: &AppState, card: CardId) -> Rect {
    state
        .hit_areas
        .grid_lines
        .iter()
        .find(|line| line.cards.iter().any(|(placed, _)| *placed == card))
        .map(|line| line.rect)
        .unwrap_or_else(|| panic!("{card:?} 줄 영역 없음"))
}

// 탭 격자의 줄 목록 (빈 줄 제외)
fn grid_rows(state: &AppState, tab: ActiveTab) -> Vec<Vec<CardId>> {
    state
        .card_layout
        .grid_of(tab)
        .rows
        .iter()
        .map(|cells| cells.iter().flatten().copied().collect::<Vec<CardId>>())
        .filter(|cards| !cards.is_empty())
        .collect()
}

// 이번 프레임 화면 줄별 카드
fn drawn_lines(state: &AppState) -> Vec<Vec<CardId>> {
    state
        .hit_areas
        .grid_lines
        .iter()
        .map(|line| line.cards.iter().map(|(card, _)| *card).collect())
        .collect()
}

// 기본 격자의 보이는 화면 줄 (명세 D7: 차트 줄은 130x40, 스트림은 100x30 부터)
fn default_lines(tab: ActiveTab, height: u16) -> Vec<Vec<CardId>> {
    let metrics = METRICS.to_vec();
    match tab {
        ActiveTab::Overview => [
            (metrics, true),
            (vec![CardId::Processes, CardId::Sockets], true),
            (
                vec![CardId::ChartSqlMix, CardId::ChartTopTables],
                height >= 40,
            ),
            (vec![CardId::SqlStream], height >= 30),
            (vec![CardId::Inspector], true),
        ]
        .into_iter()
        .filter_map(|(cards, shown)| shown.then_some(cards))
        .collect(),
        ActiveTab::Processes => vec![metrics, vec![CardId::Processes], vec![CardId::Inspector]],
        ActiveTab::TopSql => vec![
            metrics,
            vec![CardId::TopSqlRanking],
            vec![CardId::SqlDetail],
        ],
        ActiveTab::Alerts => vec![metrics, vec![CardId::AlertLog], vec![CardId::AlertDetail]],
    }
}

// 카드 영역이 area 안에 있고 서로 겹치지 않음 (빈 영역은 겹침 판정 제외)
fn assert_disjoint_inside(placed: &[(CardId, Rect)], area: Rect, label: &str) {
    for (index, (card, rect)) in placed.iter().enumerate() {
        assert!(
            rect.is_empty() || area.union(*rect) == area,
            "{label} {card:?} {rect:?} 본문 밖"
        );
        for (other, other_rect) in &placed[index + 1..] {
            assert!(
                rect.is_empty() || other_rect.is_empty() || !rect.intersects(*other_rect),
                "{label} {card:?}/{other:?} 겹침"
            );
        }
    }
}

// 포커스 색(BORDER_FOCUS)은 포커스 카드 테두리(스크롤바 글자 제외)와 제목 배경에만
fn assert_focus_color_only_on(buffer: &Buffer, focused: Rect, label: &str) {
    for (index, cell) in buffer.content().iter().enumerate() {
        let (x, y) = buffer.pos_of(index);
        let on_border = focused.contains(Position::new(x, y))
            && (x == focused.x
                || x == focused.right() - 1
                || y == focused.y
                || y == focused.bottom() - 1);
        let scrollbar = matches!(cell.symbol(), "░" | "█");
        if cell.fg == theme::BORDER_FOCUS {
            assert!(
                on_border && !scrollbar,
                "{label} ({x},{y}) 포커스색 오용 {:?}",
                cell.symbol()
            );
        }
        if cell.bg == theme::BORDER_FOCUS {
            assert!(
                y == focused.y && x > focused.x && x < focused.right() - 1,
                "{label} ({x},{y}) 포커스 배경 오용"
            );
        }
    }
}

// 130x40 으로 그리고 card 제목을 눌러 이동 모드 진입 (시계 주입), 반환 = 누른 칸
fn lift_card(state: &mut AppState, card: CardId, base: Instant) -> Position {
    draw(state, 130, 40);
    let title = state
        .hit_areas
        .title_hits
        .iter()
        .find(|hit| hit.card == card)
        .map(|hit| hit.rect)
        .unwrap_or_else(|| panic!("{card:?} 제목 영역 없음"));
    let at = Position::new(title.x + 1, title.y);
    pointer(state, MouseEventKind::Down(MouseButton::Left), at, base);
    assert!(state.tick(base + CARD_HOLD_TIME), "{card:?} 이동 모드 진입");
    assert!(state.grid_move.is_some() && state.card_move.is_some());
    at
}

fn drag_grid(state: &mut AppState, at: Position, now: Instant) {
    pointer(state, MouseEventKind::Drag(MouseButton::Left), at, now);
}

fn release_grid(state: &mut AppState, at: Position, now: Instant) -> UiCommand {
    pointer(state, MouseEventKind::Up(MouseButton::Left), at, now)
}

// 2. 기본 격자 크기·언어 행렬 -------------------------------------------------
#[test]
fn default_grid_places_disjoint_cards_inside_the_body_with_the_spec_lines_at_every_size() {
    let snap = calm_snapshot();
    let overview_heights: [(u16, &[u16]); 3] = [
        (16, &[4, 5, 3]),
        (30, &[4, 12, 4, 6]),
        (40, &[4, 13, 6, 7, 6]),
    ];
    let combos = LANGS.iter().flat_map(|lang| {
        ActiveTab::ALL
            .iter()
            .flat_map(move |tab| SIZES.iter().map(move |size| (*lang, *tab, *size)))
    });
    for (lang, tab, (width, height)) in combos {
        let label = format!("{lang:?} {tab:?} {width}x{height}");
        let mut state = state_with(&snap, tab, lang);
        draw(&mut state, width, height);
        let body = Rect::new(0, 2, width, height - 4);

        // 1. 카드 영역: 비어 있지 않고 본문 안, 서로 겹치지 않음
        let rects = state.hit_areas.card_rects.clone();
        assert!(
            rects.iter().all(|(_, rect)| !rect.is_empty()),
            "{label} 빈 카드"
        );
        assert_disjoint_inside(&rects, body, &label);

        // 2. 화면 줄: 명세 D7 카드 구성, 줄 높이 합 = 본문 높이, 줄 안 카드
        assert_eq!(drawn_lines(&state), default_lines(tab, height), "{label}");
        let heights: Vec<u16> = state
            .hit_areas
            .grid_lines
            .iter()
            .map(|line| line.rect.height)
            .collect();
        assert_eq!(
            heights.iter().sum::<u16>(),
            body.height,
            "{label} 줄 높이 합"
        );
        for line in &state.hit_areas.grid_lines {
            assert_eq!(line.tab, tab, "{label}");
            assert_disjoint_inside(&line.cards, line.rect, &label);
        }

        // 3. 개요 줄 높이 (명세 D7)
        let expected = overview_heights
            .iter()
            .find(|(rows, _)| *rows == height)
            .map(|(_, lines)| lines.to_vec());
        if tab == ActiveTab::Overview {
            assert_eq!(Some(heights), expected, "{label} 개요 줄 높이");
        }
    }
}

// 3. 차트 줄 제목 -------------------------------------------------------------
#[test]
fn chart_row_titles_show_in_both_languages_only_at_130x40() {
    let snap = calm_snapshot();
    let titles = [
        (Lang::En, ["SQL Mix", "Top Tables by RX"]),
        (Lang::Ko, ["SQL 구성", "수신량 상위 테이블"]),
    ];
    for (lang, names) in titles {
        assert_eq!(i18n::text(lang, Msg::CardSqlMix), names[0]);
        assert_eq!(i18n::text(lang, Msg::CardTopTables), names[1]);
        for (width, height) in SIZES {
            let label = format!("{lang:?} {width}x{height}");
            let mut state = state_with(&snap, ActiveTab::Overview, lang);
            let screen = dump(&draw(&mut state, width, height)).join("\n");
            let shown = (width, height) == (130, 40);
            for name in names {
                assert_eq!(screen.contains(name), shown, "{label} {name}");
            }
            let charts = state
                .hit_areas
                .card_rects
                .iter()
                .filter(|(card, _)| matches!(card, CardId::ChartSqlMix | CardId::ChartTopTables))
                .count();
            assert_eq!(charts, if shown { 2 } else { 0 }, "{label} 차트 카드 수");
        }
    }
}

// 4. 위험 경보 띠 -------------------------------------------------------------
#[test]
fn critical_snapshot_draws_the_siren_band_above_the_grid_and_keeps_the_inspector_at_60x16() {
    let snap = Arc::new(build_fixture_snapshot(true));
    assert!(snap.overall_risk >= RiskLevel::Critical);
    let combos = LANGS.iter().flat_map(|lang| {
        ActiveTab::ALL.iter().flat_map(move |tab| {
            [(60u16, 16u16, 1u16), (130, 40, 3)]
                .into_iter()
                .map(move |size| (*lang, *tab, size))
        })
    });
    for (lang, tab, (width, height, band)) in combos {
        let label = format!("{lang:?} {tab:?} {width}x{height}");
        let mut state = state_with(&snap, tab, lang);
        let rows = dump(&draw(&mut state, width, height));

        // 1. 경보 띠: 본문 맨 위 (경보 띠가 칩보다 먼저 등록)
        let siren = state
            .hit_areas
            .button_rects
            .iter()
            .find(|(action, _)| *action == UiAction::EmergencyStop)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("{label} 경보 띠 없음"));
        assert_eq!(siren, Rect::new(0, 2, width, band), "{label}");
        let action = i18n::text(lang, Msg::SirenAction);
        let lead = action.split(':').next().unwrap_or(action);
        let text_row = &rows[usize::from(2 + band / 2)];
        assert!(text_row.contains(lead), "{label} 경보 문구: {text_row}");

        // 2. 격자 줄: 경보 띠 바로 아래부터 남은 본문을 채움
        let lines = &state.hit_areas.grid_lines;
        assert_eq!(
            lines.first().map(|line| line.rect.y),
            Some(siren.bottom()),
            "{label}"
        );
        assert!(
            lines.iter().all(|line| line.rect.y >= siren.bottom()),
            "{label}"
        );
        let total: u16 = lines.iter().map(|line| line.rect.height).sum();
        assert_eq!(total, height - 4 - band, "{label} 줄 높이 합");
    }

    // 3. 60x16 개요: 지표 4 / 표 4 / 인스펙터 3 (명세 D7)
    for lang in LANGS {
        let mut state = state_with(&snap, ActiveTab::Overview, lang);
        draw(&mut state, 60, 16);
        let heights: Vec<u16> = state
            .hit_areas
            .grid_lines
            .iter()
            .map(|line| line.rect.height)
            .collect();
        assert_eq!(heights, [4, 4, 3], "{lang:?}");
        assert_eq!(
            drawn_lines(&state).last(),
            Some(&vec![CardId::Inspector]),
            "{lang:?}"
        );
        assert_eq!(drawn_rect(&state, CardId::Inspector).height, 3, "{lang:?}");
    }
}

// 5. 길게 누름·끌기 이동 ------------------------------------------------------
#[test]
fn grid_drag_into_another_row_joins_it_and_release_saves() {
    let mut state = overview_state();
    let base = Instant::now();
    lift_card(&mut state, CardId::Inspector, base);
    let sockets = drawn_rect(&state, CardId::Sockets);
    let tables_line = line_rect(&state, CardId::Sockets);
    assert!(tables_line.height >= 3);

    // 1. 소켓 카드 왼쪽 안: 그 카드 앞에 합류, 빈 인스펙터 줄 제거
    let drop = Position::new(sockets.x + 1, tables_line.y + 2);
    drag_grid(&mut state, drop, after(base, 450));
    let rows = grid_rows(&state, ActiveTab::Overview);
    assert_eq!(
        rows[1],
        [CardId::Processes, CardId::Inspector, CardId::Sockets]
    );
    assert_eq!(rows.len(), 4);

    // 2. 놓으면 확정·알림·저장 요청
    let command = release_grid(&mut state, drop, after(base, 460));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(
        state.toast.as_ref().map(|toast| toast.text.as_str()),
        Some(i18n::text(Lang::En, Msg::ToastCardMoved))
    );
    assert!(state.grid_move.is_none() && state.card_move.is_none());

    // 3. 다음 프레임: 같은 줄
    draw(&mut state, 130, 40);
    assert_eq!(
        drawn_rect(&state, CardId::Inspector).y,
        drawn_rect(&state, CardId::Sockets).y
    );
}

#[test]
fn grid_drop_on_a_bottom_or_top_border_creates_a_row_there() {
    let base = Instant::now();

    // 1. 아래 테두리: 표 줄 아래 새 줄 (지표 카드)
    let mut state = overview_state();
    lift_card(&mut state, CardId::Traffic, base);
    let tables_line = line_rect(&state, CardId::Processes);
    let drop = Position::new(tables_line.x + 3, tables_line.bottom() - 1);
    drag_grid(&mut state, drop, after(base, 450));
    let command = release_grid(&mut state, drop, after(base, 460));
    assert!(matches!(command, UiCommand::SaveState));
    let rows = grid_rows(&state, ActiveTab::Overview);
    assert_eq!(rows.len(), 6);
    assert_eq!(
        rows[0],
        [CardId::Risk, CardId::Connections, CardId::QueryRate]
    );
    assert_eq!(rows[1], [CardId::Processes, CardId::Sockets]);
    assert_eq!(rows[2], [CardId::Traffic]);

    // 2. 위 테두리: 차트 줄 위 새 줄 (표 카드)
    let mut state = overview_state();
    lift_card(&mut state, CardId::Sockets, base);
    let chart_line = line_rect(&state, CardId::ChartSqlMix);
    let drop = Position::new(chart_line.x + 3, chart_line.y);
    drag_grid(&mut state, drop, after(base, 450));
    let command = release_grid(&mut state, drop, after(base, 460));
    assert!(matches!(command, UiCommand::SaveState));
    let rows = grid_rows(&state, ActiveTab::Overview);
    assert_eq!(rows.len(), 6);
    let chart = rows
        .iter()
        .position(|row| row.contains(&CardId::ChartSqlMix))
        .expect("chart row");
    assert_eq!(rows[chart - 1], [CardId::Sockets]);
    assert_eq!(rows[chart - 2], [CardId::Processes]);
}

#[test]
fn grid_drop_that_empties_a_row_removes_the_row() {
    let mut state = overview_state();
    let base = Instant::now();
    let before = grid_rows(&state, ActiveTab::Overview);
    lift_card(&mut state, CardId::SqlStream, base);
    let inspector_line = line_rect(&state, CardId::Inspector);
    assert!(inspector_line.height >= 3);

    // 인스펙터 줄 오른쪽 끝 안: 마지막 카드 뒤, 스트림 줄은 비어 사라짐
    let drop = Position::new(inspector_line.right() - 2, inspector_line.y + 1);
    drag_grid(&mut state, drop, after(base, 450));
    let command = release_grid(&mut state, drop, after(base, 460));
    assert!(matches!(command, UiCommand::SaveState));
    let rows = grid_rows(&state, ActiveTab::Overview);
    assert_eq!(rows.len(), before.len() - 1);
    assert_eq!(rows[..3], before[..3]);
    assert_eq!(
        rows.last().map(Vec::as_slice),
        Some([CardId::Inspector, CardId::SqlStream].as_slice())
    );
    draw(&mut state, 130, 40);
    assert_eq!(
        drawn_rect(&state, CardId::SqlStream).y,
        drawn_rect(&state, CardId::Inspector).y
    );
}

#[test]
fn grid_drop_of_a_seventh_card_into_a_full_row_keeps_the_preview() {
    // 1. 지표 네 장 + 표 두 장 = 여섯 장 첫 줄
    let mut state = overview_state();
    let base = Instant::now();
    let five = apply_drop(
        &default_rows(ActiveTab::Overview),
        CardId::Processes,
        DropTarget::After(CardId::QueryRate),
    )
    .expect("five");
    let full: TabGrid =
        apply_drop(&five, CardId::Sockets, DropTarget::After(CardId::Processes)).expect("full");
    state.card_layout.set_grid(ActiveTab::Overview, full);
    assert_eq!(grid_rows(&state, ActiveTab::Overview)[0].len(), 6);
    let before = state.card_layout;

    // 2. 차트 줄 위 테두리: 미리보기 새 줄
    lift_card(&mut state, CardId::SqlStream, base);
    let full_line = line_rect(&state, CardId::Risk);
    let chart_line = line_rect(&state, CardId::ChartSqlMix);
    drag_grid(
        &mut state,
        Position::new(chart_line.x + 3, chart_line.y),
        after(base, 450),
    );
    let preview = state.card_layout.grid_of(ActiveTab::Overview);
    assert_ne!(preview, full);

    // 3. 여섯 장 줄 안: 거부, 미리보기 그대로
    drag_grid(
        &mut state,
        Position::new(full_line.x + 3, full_line.y + 1),
        after(base, 460),
    );
    assert_eq!(state.card_layout.grid_of(ActiveTab::Overview), preview);
    assert!(state.grid_move.is_some());

    // 4. Esc: 누르기 전 격자
    let command = state.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    assert!(matches!(command, UiCommand::None));
    assert!(state.grid_move.is_none() && state.card_move.is_none());
    assert_eq!(state.card_layout, before);
}

#[test]
fn grid_move_cancel_by_escape_resize_or_tab_switch_restores_the_grid_and_ratios() {
    let base = Instant::now();
    for cancel in ["esc", "resize", "tab"] {
        let mut state = overview_state();
        assert!(
            state
                .card_layout
                .restore_grid_ratios("overview_rows_5", &[1_000, 2_000, 3_000, 4_000])
        );
        let before = state.card_layout;
        lift_card(&mut state, CardId::Inspector, base);
        let chart_line = line_rect(&state, CardId::ChartSqlMix);
        let drop = Position::new(chart_line.x + 3, chart_line.y);
        drag_grid(&mut state, drop, after(base, 450));
        assert_ne!(state.card_layout, before, "{cancel}");

        // 1. 취소 경로별 복원
        let command = match cancel {
            "esc" => state.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)),
            "resize" => state.handle_resize(130, 40),
            _ => state.switch_tab(ActiveTab::Alerts),
        };
        assert!(matches!(command, UiCommand::None), "{cancel}");
        assert_eq!(state.card_layout, before, "{cancel}");
        assert!(state.grid_move.is_none(), "{cancel}");
        assert!(state.card_move.is_none(), "{cancel}");
        assert!(state.card_press.is_none(), "{cancel}");

        // 2. 늦게 온 놓기는 아무 일도 하지 않음
        let command = release_grid(&mut state, drop, after(base, 510));
        assert!(matches!(command, UiCommand::None), "{cancel}");
        assert_eq!(state.card_layout, before, "{cancel}");
    }
}

// 놓기 놓침(버튼 없는 움직임)과 새 누름은 놓은 것과 같이 미리보기 격자를 확정
#[test]
fn grid_lost_release_and_new_press_commit_the_preview_like_a_release() {
    let base = Instant::now();
    let away = Position::new(3, 3);
    for ending in [
        MouseEventKind::Moved,
        MouseEventKind::Down(MouseButton::Left),
    ] {
        let mut state = overview_state();
        lift_card(&mut state, CardId::Inspector, base);
        let chart_line = line_rect(&state, CardId::ChartSqlMix);
        let drop = Position::new(chart_line.x + 3, chart_line.y);
        drag_grid(&mut state, drop, after(base, 450));
        let preview = state.card_layout.grid_of(ActiveTab::Overview);
        assert_ne!(preview, default_rows(ActiveTab::Overview), "{ending:?}");

        // 1. 확정: 알림과 저장 요청, 그 누름은 새 대기를 만들지 않음
        let command = pointer(&mut state, ending, away, after(base, 500));
        assert!(matches!(command, UiCommand::SaveState), "{ending:?}");
        assert_eq!(
            state.card_layout.grid_of(ActiveTab::Overview),
            preview,
            "{ending:?}"
        );
        assert!(state.grid_move.is_none(), "{ending:?}");
        assert!(state.card_move.is_none(), "{ending:?}");
        assert!(state.card_press.is_none(), "{ending:?}");
        assert!(state.toast.is_some(), "{ending:?}");

        // 2. 늦게 온 놓기는 아무 일도 하지 않음
        let command = release_grid(&mut state, drop, after(base, 510));
        assert!(matches!(command, UiCommand::None), "{ending:?}");
        assert_eq!(
            state.card_layout.grid_of(ActiveTab::Overview),
            preview,
            "{ending:?}"
        );
    }
}

#[test]
fn grid_release_at_the_start_or_back_over_its_own_card_saves_nothing() {
    let base = Instant::now();

    // 1. 누른 칸에서 놓기
    let mut state = overview_state();
    let title = lift_card(&mut state, CardId::Inspector, base);
    let command = release_grid(&mut state, title, after(base, 450));
    assert!(matches!(command, UiCommand::None));
    assert_eq!(state.card_layout, CardLayout::default());
    assert!(state.toast.is_none());

    // 2. 다른 줄에서 자기 카드 위로 복귀
    let mut state = overview_state();
    lift_card(&mut state, CardId::Inspector, base);
    let own = drawn_rect(&state, CardId::Inspector);
    let chart_line = line_rect(&state, CardId::ChartSqlMix);
    drag_grid(
        &mut state,
        Position::new(chart_line.x + 3, chart_line.y),
        after(base, 450),
    );
    assert_ne!(
        state.card_layout.grid_of(ActiveTab::Overview),
        default_rows(ActiveTab::Overview)
    );
    let back = Position::new(own.x + 2, own.y + 1);
    drag_grid(&mut state, back, after(base, 460));
    assert_eq!(
        state.card_layout.grid_of(ActiveTab::Overview),
        default_rows(ActiveTab::Overview)
    );
    let command = release_grid(&mut state, back, after(base, 470));
    assert!(matches!(command, UiCommand::None));
    assert_eq!(state.card_layout, CardLayout::default());
    assert!(state.toast.is_none());
}

// 6. Shift+방향키와 l ---------------------------------------------------------
#[test]
fn shift_arrows_move_a_drawn_grid_card_in_all_four_directions_and_save() {
    let shift = |code: KeyCode| KeyEvent::new(code, KeyModifiers::SHIFT);
    let mut state = overview_state();
    state.focused_card = Some((ActiveTab::Overview, CardId::Connections));
    draw(&mut state, 130, 40);
    let base = grid_rows(&state, ActiveTab::Overview);

    // 1. 오른쪽·왼쪽: 줄 안 교환
    let command = state.handle_key_event(shift(KeyCode::Right));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(
        grid_rows(&state, ActiveTab::Overview)[0],
        [
            CardId::Risk,
            CardId::Traffic,
            CardId::Connections,
            CardId::QueryRate
        ]
    );
    draw(&mut state, 130, 40);
    let command = state.handle_key_event(shift(KeyCode::Left));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(grid_rows(&state, ActiveTab::Overview), base);

    // 2. 아래: 자기 새 줄, 다음은 아래 줄 끝 합류
    draw(&mut state, 130, 40);
    let command = state.handle_key_event(shift(KeyCode::Down));
    assert!(matches!(command, UiCommand::SaveState));
    let rows = grid_rows(&state, ActiveTab::Overview);
    assert_eq!(rows.len(), 6);
    assert_eq!(rows[1], [CardId::Connections]);
    draw(&mut state, 130, 40);
    let command = state.handle_key_event(shift(KeyCode::Down));
    assert!(matches!(command, UiCommand::SaveState));
    let rows = grid_rows(&state, ActiveTab::Overview);
    assert_eq!(rows.len(), 5);
    assert_eq!(
        rows[1],
        [CardId::Processes, CardId::Sockets, CardId::Connections]
    );

    // 3. 위: 위에 새 줄, 다음은 위 줄 끝 합류
    draw(&mut state, 130, 40);
    let command = state.handle_key_event(shift(KeyCode::Up));
    assert!(matches!(command, UiCommand::SaveState));
    let rows = grid_rows(&state, ActiveTab::Overview);
    assert_eq!(rows[1], [CardId::Connections]);
    assert_eq!(rows[2], [CardId::Processes, CardId::Sockets]);
    draw(&mut state, 130, 40);
    let command = state.handle_key_event(shift(KeyCode::Up));
    assert!(matches!(command, UiCommand::SaveState));
    let rows = grid_rows(&state, ActiveTab::Overview);
    assert_eq!(rows.len(), 5);
    assert_eq!(
        rows[0],
        [
            CardId::Risk,
            CardId::Traffic,
            CardId::QueryRate,
            CardId::Connections
        ]
    );

    // 4. 다음 프레임: 지표 줄 끝에 그려짐
    draw(&mut state, 130, 40);
    let connections = drawn_rect(&state, CardId::Connections);
    assert_eq!(connections.y, drawn_rect(&state, CardId::Risk).y);
    assert!(connections.x > drawn_rect(&state, CardId::QueryRate).x);
}

#[test]
fn shift_arrows_without_a_drawn_neighbor_return_none_and_show_no_toast() {
    let snap = calm_snapshot();
    let shift = |code: KeyCode| KeyEvent::new(code, KeyModifiers::SHIFT);

    // 1. 프로세스 탭: 혼자인 줄의 좌우, 줄 맨 앞 지표 카드의 왼쪽
    let mut state = state_with(&snap, ActiveTab::Processes, Lang::En);
    draw(&mut state, 130, 40);
    for (card, code) in [
        (CardId::Processes, KeyCode::Left),
        (CardId::Processes, KeyCode::Right),
        (CardId::Risk, KeyCode::Left),
        (CardId::QueryRate, KeyCode::Right),
    ] {
        state.focused_card = Some((ActiveTab::Processes, card));
        let command = state.handle_key_event(shift(code));
        assert!(matches!(command, UiCommand::None), "{card:?} {code:?}");
        assert_eq!(
            state.card_layout,
            CardLayout::default(),
            "{card:?} {code:?}"
        );
        assert!(state.toast.is_none(), "{card:?} {code:?}");
    }

    // 2. 100x30 개요: 이번 프레임에 그려지지 않은 차트 카드
    let mut state = state_with(&snap, ActiveTab::Overview, Lang::En);
    draw(&mut state, 100, 30);
    state.focused_card = Some((ActiveTab::Overview, CardId::ChartSqlMix));
    let command = state.handle_key_event(shift(KeyCode::Right));
    assert!(matches!(command, UiCommand::None));
    assert_eq!(state.card_layout, CardLayout::default());
    assert!(state.toast.is_none());
}

#[test]
fn l_resets_only_the_current_tab_grid_and_ratios_and_keeps_tab_order_and_language() {
    // 1. 개요·프로세스 탭 격자와 비율 변경, 탭 순서·언어 변경
    let mut state = overview_state();
    state.lang = Lang::Ko;
    let tab_order = [
        ActiveTab::Alerts,
        ActiveTab::TopSql,
        ActiveTab::Processes,
        ActiveTab::Overview,
    ];
    state.tab_order = tab_order;
    let overview_grid = apply_drop(
        &default_rows(ActiveTab::Overview),
        CardId::Risk,
        DropTarget::RowBelow(CardId::Inspector),
    )
    .expect("overview grid");
    let processes_grid = apply_drop(
        &default_rows(ActiveTab::Processes),
        CardId::Inspector,
        DropTarget::After(CardId::Processes),
    )
    .expect("processes grid");
    state
        .card_layout
        .set_grid(ActiveTab::Overview, overview_grid);
    state
        .card_layout
        .set_grid(ActiveTab::Processes, processes_grid);
    assert!(
        state
            .card_layout
            .restore_grid_ratios("overview_cols_1", &[3_000])
    );
    assert!(
        state
            .card_layout
            .restore_grid_ratios("overview_rows_5", &[1_000, 2_000, 3_000, 4_000])
    );
    assert!(
        state
            .card_layout
            .restore_grid_ratios("processes_cols_1", &[6_000])
    );
    let processes_entries: Vec<(String, Vec<u16>)> = state
        .card_layout
        .grid_ratio_entries()
        .into_iter()
        .filter(|(key, _)| key.starts_with("processes_"))
        .collect();
    assert_eq!(processes_entries.len(), 1);
    draw(&mut state, 130, 40);

    // 2. l: 개요만 기본 격자·비율, 프로세스 탭·탭 순서·언어 유지
    let command = state.handle_key_event(KeyEvent::new(KeyCode::Char('l'), KeyModifiers::NONE));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(
        state.card_layout.grid_of(ActiveTab::Overview),
        default_rows(ActiveTab::Overview)
    );
    let entries = state.card_layout.grid_ratio_entries();
    assert!(
        entries.iter().all(|(key, _)| !key.starts_with("overview_")),
        "{entries:?}"
    );
    assert_eq!(
        state.card_layout.grid_of(ActiveTab::Processes),
        processes_grid
    );
    let kept: Vec<(String, Vec<u16>)> = entries
        .into_iter()
        .filter(|(key, _)| key.starts_with("processes_"))
        .collect();
    assert_eq!(kept, processes_entries);
    assert_eq!(state.tab_order, tab_order);
    assert_eq!(state.lang, Lang::Ko);
}

// 7. 모든 놓기 위치 렌더 ------------------------------------------------------
#[test]
fn every_grid_card_renders_at_every_drop_target_at_60x16_and_130x40_with_disjoint_rects() {
    let snap = calm_snapshot();

    // 1. 격자의 모든 카드를 다른 모든 카드의 앞·뒤·위 새 줄·아래 새 줄로 옮긴 격자 목록
    let mut cases: Vec<(ActiveTab, TabGrid)> = Vec::new();
    for tab in ActiveTab::ALL {
        let base = default_rows(tab);
        let cards: Vec<CardId> = base.rows.iter().flatten().flatten().copied().collect();
        let targets = cards.iter().flat_map(|anchor| {
            [
                DropTarget::Before(*anchor),
                DropTarget::After(*anchor),
                DropTarget::RowAbove(*anchor),
                DropTarget::RowBelow(*anchor),
            ]
        });
        let moves = cards
            .iter()
            .flat_map(|moving| targets.clone().map(move |target| (*moving, target)));
        for (moving, target) in moves {
            cases.extend(apply_drop(&base, moving, target).map(|grid| (tab, grid)));
        }
    }
    assert!(cases.len() > 100);

    // 2. 두 크기에서 그린 카드는 본문·줄 영역 안, 서로 겹치지 않고 격자가 준 영역과 같음
    let renders = cases
        .iter()
        .flat_map(|case| [(60u16, 16u16), (130, 40)].map(|size| (*case, size)));
    for ((tab, grid), (width, height)) in renders {
        let mut state = state_with(&snap, tab, Lang::En);
        state.card_layout.set_grid(tab, grid);
        draw(&mut state, width, height);
        let label = format!("{tab:?} {width}x{height} {:?}", grid_rows(&state, tab));
        let body = Rect::new(0, 2, width, height - 4);
        assert_disjoint_inside(&state.hit_areas.card_rects, body, &label);
        let placed = state
            .hit_areas
            .grid_lines
            .iter()
            .flat_map(|line| line.cards.iter().map(move |card| (line, *card)));
        for (line, (card, rect)) in placed {
            assert_eq!(line.tab, tab, "{label}");
            assert_eq!(body.union(line.rect), body, "{label} 줄 영역");
            assert_eq!(line.rect.union(rect), line.rect, "{label} {card:?}");
            let drawn = state
                .hit_areas
                .card_rects
                .iter()
                .find(|(drawn, _)| *drawn == card)
                .map(|(_, drawn_rect)| *drawn_rect);
            assert!(
                drawn.is_none_or(|drawn_rect| drawn_rect == rect),
                "{label} {card:?}"
            );
        }
    }
}

// 8. 탭별 격자 ----------------------------------------------------------------
#[test]
fn moving_risk_on_overview_leaves_every_other_tab_grid_at_its_default() {
    // 1. 개요에서 위험 카드를 지표 줄 끝으로
    let mut state = overview_state();
    let base = Instant::now();
    lift_card(&mut state, CardId::Risk, base);
    let query_rate = drawn_rect(&state, CardId::QueryRate);
    let drop = Position::new(query_rate.right() - 2, query_rate.y + 1);
    drag_grid(&mut state, drop, after(base, 450));
    let command = release_grid(&mut state, drop, after(base, 460));
    assert!(matches!(command, UiCommand::SaveState));
    assert_eq!(
        grid_rows(&state, ActiveTab::Overview)[0],
        [
            CardId::Connections,
            CardId::Traffic,
            CardId::QueryRate,
            CardId::Risk
        ]
    );
    for tab in [ActiveTab::Processes, ActiveTab::TopSql, ActiveTab::Alerts] {
        assert_eq!(state.card_layout.grid_of(tab), default_rows(tab), "{tab:?}");
    }

    // 2. 프로세스 탭: 0행은 기본 순서로 그려짐
    state.switch_tab(ActiveTab::Processes);
    draw(&mut state, 130, 40);
    assert_eq!(drawn_lines(&state)[0], METRICS.to_vec());
    assert!(drawn_rect(&state, CardId::Risk).x < drawn_rect(&state, CardId::Connections).x);
}

// 9. 이동 뒤 포커스 색 예약 ---------------------------------------------------
#[test]
fn after_a_grid_move_the_focus_color_marks_only_the_focused_card_border_and_title() {
    // 1. 길게 누름 이동: 인스펙터를 표 줄로 (포커스 = 옮긴 카드)
    let base = Instant::now();
    let mut moved = overview_state();
    lift_card(&mut moved, CardId::Inspector, base);
    let sockets = drawn_rect(&moved, CardId::Sockets);
    let drop = Position::new(sockets.x + 1, line_rect(&moved, CardId::Sockets).y + 2);
    drag_grid(&mut moved, drop, after(base, 450));
    let command = release_grid(&mut moved, drop, after(base, 460));
    assert!(matches!(command, UiCommand::SaveState));

    // 2. Shift+아래: 소켓 카드를 자기 새 줄로
    let mut shifted = overview_state();
    shifted.focused_card = Some((ActiveTab::Overview, CardId::Sockets));
    draw(&mut shifted, 130, 40);
    let command = shifted.handle_key_event(KeyEvent::new(KeyCode::Down, KeyModifiers::SHIFT));
    assert!(matches!(command, UiCommand::SaveState));

    // 3. 크기·언어 행렬에서 포커스 색은 포커스 카드 테두리와 제목에만
    let combos = LANGS
        .iter()
        .flat_map(|lang| SIZES.iter().map(move |size| (*lang, *size)));
    for (name, mut state) in [("mouse", moved), ("shift", shifted)] {
        for (lang, (width, height)) in combos.clone() {
            state.lang = lang;
            let buffer = draw(&mut state, width, height);
            let focused = drawn_rect(&state, state.focused_card_id());
            let label = format!("{name} {lang:?} {width}x{height}");
            assert_focus_color_only_on(&buffer, focused, &label);
        }
    }
}
