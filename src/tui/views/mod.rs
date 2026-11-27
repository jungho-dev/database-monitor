//! views/mod.rs
//! 화면 조립: 앱 바, 탭 바, 본문, 하단 칩·안내, 카드 경계 손잡이, 도움말·종료 확인 겹침.

pub mod alerts;
pub mod charts;
pub mod common;
pub mod overview;
pub mod processes;
pub mod top_sql;

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Clear, Padding, Paragraph};
use crate::app::model::RiskLevel;
use crate::analysis::risk::format_bytes;
use crate::tui::cards::{CardId, CardKind, UiAction, UiHitAreas};
use crate::tui::format::{fit_display, text_width};
use crate::tui::i18n::{self, Lang, Msg};
use crate::tui::state::{ActiveTab, AppState, KillConfirm, ToastLevel};
use crate::tui::theme;
use crate::tui::views::common::{
    render_column_handles, render_notice_card, render_split_handles,
};

pub const MIN_WIDTH: u16 = 60;
pub const MIN_HEIGHT: u16 = 16;
// 경보 카드를 테두리 있는 3행으로 그리는 최소 본문 높이
const SIREN_CARD_BODY_HEIGHT: u16 = 20;
// 긴급 중단 확인 창에 이름으로 보여 주는 대상 수 상한 (나머지는 외 N개)
const EMERGENCY_NAME_ROWS: usize = 5;

// 1. 전체 프레임 --------------------------------------------------------------
pub fn render_frame(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();
    // 최소 크기 미달이면 파괴적 키가 동작하지 않고 열려 있던 종료 확인도 취소됨
    state.apply_screen_size(area.width, area.height);
    state.hit_areas = UiHitAreas::default();
    let danger_tint = state.is_critical() && state.blink_on;
    let base_bg = if danger_tint {
        theme::BG_DANGER_TINT
    } else {
        theme::BG_BASE
    };
    frame.render_widget(
        Block::default().style(Style::default().bg(base_bg).fg(theme::FG_TEXT)),
        area,
    );

    // 1. 최소 크기 미달
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        let notice = Paragraph::new(Line::from(Span::styled(
            i18n::too_small(state.lang, MIN_WIDTH, MIN_HEIGHT, area.width, area.height),
            theme::STYLE_WARN,
        )))
        .alignment(Alignment::Center);
        frame.render_widget(
            notice,
            Rect::new(area.x, area.y + area.height / 2, area.width, 1),
        );
        return;
    }

    // 2. 앱 바, 탭 바, 본문, 하단 두 줄
    let chunks = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Min(3),
        Constraint::Length(2),
    ])
    .split(area);
    render_app_bar(frame, chunks[0], state);
    render_tab_bar(frame, chunks[1], state);
    render_body(frame, chunks[2], state);
    render_footer(frame, chunks[3], state);

    // 3. 카드·열 경계 손잡이와 겹침
    render_split_handles(frame, state);
    render_column_handles(frame, state);
    if state.show_help {
        render_help_overlay(frame, area, state.lang);
    }
    if let Some(target) = state.kill_confirm.clone() {
        render_kill_confirm(frame, area, state, &target);
    }
}

// 2. 본문 ---------------------------------------------------------------------
// 위험 시 맨 위 경보 카드, 탭 격자 (0행 지표 카드, 첫 집계 전에는 대기 안내 카드)
fn render_body(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let Some(snap) = state.snapshot.clone() else {
        render_notice_card(
            frame,
            state,
            CardId::Gate,
            area,
            i18n::text(state.lang, Msg::CardGate),
            (Msg::PanelPending, Msg::PanelPendingHint),
        );
        return;
    };

    // 1. 경보 카드 높이 (위험일 때만, 낮은 본문은 한 줄)
    let siren_height: u16 = if !state.is_critical() {
        0
    } else if area.height >= SIREN_CARD_BODY_HEIGHT {
        3
    } else {
        1
    };
    let rows = Layout::vertical([Constraint::Length(siren_height), Constraint::Min(0)]).split(area);

    // 2. 경보 카드와 탭 격자
    if siren_height > 0 {
        let summary = i18n::summary_text(state.lang, &snap.risk_summary);
        render_siren(frame, rows[0], state, &summary);
    }
    match state.tab {
        ActiveTab::Overview => overview::render_overview_tab(frame, rows[1], state, &snap),
        ActiveTab::Processes => processes::render_processes_tab(frame, rows[1], state, &snap),
        ActiveTab::TopSql => top_sql::render_top_sql_tab(frame, rows[1], state, &snap),
        ActiveTab::Alerts => alerts::render_alerts_tab(frame, rows[1], state, &snap),
    }
}

// 3. 위험 경보 카드 -----------------------------------------------------------
// 점멸 OFF면 한 위상 고정, 클릭하면 위험 프로세스 일괄 중단
fn render_siren(frame: &mut Frame, area: Rect, state: &mut AppState, summary: &str) {
    state
        .hit_areas
        .button_rects
        .push((UiAction::EmergencyStop, area));
    let alternate = state.blink_enabled && !state.blink_on;
    let (bg, fg, border) = if alternate {
        (theme::ALARM_FLASH, theme::FG_ON_ACCENT, theme::ALARM_FILL)
    } else {
        (theme::ALARM_FILL, theme::FG_STRONG, theme::ALARM_FLASH)
    };
    let text = Line::from(vec![
        Span::styled(
            format!(" {} ", i18n::text(state.lang, Msg::SirenAction)),
            Style::default()
                .fg(theme::FG_STRONG)
                .bg(theme::ALARM_LABEL_BG)
                .add_modifier(Modifier::BOLD),
        ),
        Span::raw(" "),
        Span::styled(
            format!(" {} {summary} ", i18n::text(state.lang, Msg::SirenBadge)),
            Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
        ),
    ]);
    if area.height < 3 {
        frame.render_widget(Paragraph::new(text).style(Style::default().bg(bg)), area);
        return;
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border).add_modifier(Modifier::BOLD))
        .style(Style::default().bg(bg));
    frame.render_widget(Paragraph::new(text).block(block), area);
}

// 4. 앱 바 --------------------------------------------------------------------
// 왼쪽 배지·상태 배지, 오른쪽 범위(수집 방식·주기)와 갱신 신선도 (폭이 모자라면 범위부터 생략)
fn render_app_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let lang = state.lang;
    let bar = Style::default().bg(theme::BG_BAR);

    // 1. 왼쪽 배지
    let mut left_spans = vec![Span::styled(" DB-MONITOR ", theme::STYLE_BADGE)];
    if state.paused {
        left_spans.push(Span::raw("  "));
        left_spans.push(Span::styled(
            format!(" {} ", i18n::text(lang, Msg::BadgePaused)),
            theme::STYLE_BADGE_WARN,
        ));
    }
    if state.always_on_top {
        left_spans.push(Span::raw("  "));
        left_spans.push(Span::styled(
            format!(" {} ", i18n::text(lang, Msg::BadgePinned)),
            theme::STYLE_BADGE_WARN,
        ));
    }
    if let Some(snap) = &state.snapshot
        && snap.capture_dropped_frames > 0
    {
        left_spans.push(Span::raw("  "));
        left_spans.push(Span::styled(
            format!(" {} ", i18n::capture_drops(lang, snap.capture_dropped_frames)),
            theme::STYLE_BADGE_WARN,
        ));
    }
    let left = Line::from(left_spans);

    // 2. 신선도 (정상 점, 지연 경고 점과 경과 초, 대기)
    let freshness: Vec<Span<'static>> = match (&state.snapshot, state.stale_age_secs()) {
        (Some(_), Some(age)) => vec![
            Span::styled("● ", theme::STYLE_WARN),
            Span::styled(i18n::stale(lang, age), theme::STYLE_WARN),
        ],
        (Some(snap), None) => vec![
            Span::styled("● ", theme::STYLE_OK),
            Span::styled(snap.captured_at.clone(), theme::STYLE_DIM),
        ],
        (None, _) => vec![Span::styled(
            format!("○ {}", i18n::text(lang, Msg::AppBarWaiting)),
            theme::STYLE_DIM,
        )],
    };

    // 3. 오른쪽 범위: 수집 방식 + 주기, 주기만, 없음 순으로 폭에 맞춤
    let room = usize::from(area.width).saturating_sub(left.width() + 1);
    let poll = format!("{}ms", state.poll_interval_ms);
    let mode = state.snapshot.as_ref().map(|snap| snap.capture_mode.clone());
    let mut candidates: Vec<Vec<Span<'static>>> = Vec::new();
    if let Some(mode) = mode {
        candidates.push(vec![
            Span::styled(format!("{} ", i18n::text(lang, Msg::AppBarMode)), theme::STYLE_DIM),
            Span::styled(mode, theme::STYLE_TEXT),
            Span::styled(
                format!("  {} {poll}  ", i18n::text(lang, Msg::AppBarPoll)),
                theme::STYLE_DIM,
            ),
        ]);
    }
    candidates.push(vec![Span::styled(
        format!("{} {poll}  ", i18n::text(lang, Msg::AppBarPoll)),
        theme::STYLE_DIM,
    )]);
    candidates.push(Vec::new());
    let scope = candidates
        .into_iter()
        .find(|spans| {
            let width: usize = spans.iter().map(Span::width).sum::<usize>()
                + freshness.iter().map(Span::width).sum::<usize>();
            width <= room
        })
        .unwrap_or_default();
    let mut right_spans = scope;
    right_spans.extend(freshness);
    right_spans.push(Span::raw(" "));
    let right = Line::from(right_spans);
    let right_width = (right.width() as u16).min(area.width);
    let columns = Layout::horizontal([Constraint::Min(0), Constraint::Length(right_width)]).split(area);
    frame.render_widget(Paragraph::new(left).style(bar), columns[0]);
    frame.render_widget(Paragraph::new(right).style(bar), columns[1]);
}

// 5. 탭 바 --------------------------------------------------------------------
// 탭마다 클릭 영역 등록, 전체 폭을 넘으면 간격 0, 그래도 넘으면 번호 붙임·여백 없는 압축
fn render_tab_bar(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let row_end = area.x.saturating_add(area.width);
    let mut cursor = area.x.saturating_add(1);

    // 1. 압축 단계 (0 전체, 1 간격 0, 2 번호 붙임·여백 없음·간격 1)
    let tab_count = ActiveTab::ALL.len();
    let full_labels: usize = ActiveTab::ALL
        .iter()
        .map(|tab| text_width(tab.tab_label(state.lang)))
        .sum();
    let full_width = full_labels + 4 * tab_count + tab_count;
    let compact_width = full_labels + 4 * tab_count + 1;
    let level = if full_width <= usize::from(area.width) {
        0
    } else if compact_width <= usize::from(area.width) {
        1
    } else {
        2
    };
    let gap = u16::from(level != 1);

    // 2. 탭 그리기
    let tab_order = state.tab_order;
    for (index, tab) in tab_order.iter().enumerate() {
        let (key_style, name_style, background) = if state.tab == *tab {
            (
                Style::default()
                    .fg(theme::FG_ACCENT)
                    .add_modifier(Modifier::BOLD),
                Style::default()
                    .fg(theme::FG_STRONG)
                    .add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
                theme::BG_PANEL,
            )
        } else {
            (
                theme::STYLE_DIM,
                Style::default().fg(theme::TAB_IDLE_NAME),
                theme::BG_BASE,
            )
        };
        let label = tab.tab_label(state.lang);
        let line = match level {
            2 => Line::from(vec![
                Span::styled(format!("{}", index + 1), key_style),
                Span::styled(label, name_style),
            ]),
            _ => Line::from(vec![
                Span::styled(format!(" {} ", index + 1), key_style),
                Span::styled(label, name_style),
                Span::raw(" "),
            ]),
        };
        let width = line.width() as u16;
        if cursor.saturating_add(width) > row_end {
            break;
        }
        let rect = Rect::new(cursor, area.y, width, area.height);
        frame.render_widget(
            Paragraph::new(line).style(Style::default().bg(background)),
            rect,
        );
        state.hit_areas.tab_rects.push((*tab, rect));
        cursor = cursor.saturating_add(width + gap);
    }
}

// 6. 하단 두 줄 ---------------------------------------------------------------
// 1행 클릭 칩(왼쪽 기능, 오른쪽 도움말·종료), 2행 알림과 상황별 단축키 안내
fn render_footer(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).split(area);
    frame.render_widget(
        Block::default().style(Style::default().bg(theme::BG_BAR)),
        area,
    );
    render_footer_chips(frame, rows[0], state);
    render_footer_hints(frame, rows[1], state);
}

// 7. 하단 칩 줄 ---------------------------------------------------------------
// 칩 = 키 글자(강조색) + 이름, 폭이 모자라면 왼쪽 그룹의 뒤쪽부터 생략 (언어 칩은 마지막까지 유지)
fn render_footer_chips(frame: &mut Frame, row: Rect, state: &mut AppState) {
    // 1. 칩 문구 (켜짐 칩만 채움, 종료는 키 글자만 적색, 위험 중에만 중단 칩 채움)
    let lang = state.lang;
    let chip_name = |msg: Msg| format!(" {} ", i18n::text(lang, msg));
    let tonal = |key: &'static str, name: String| {
        Line::from(vec![
            Span::styled(key, theme::STYLE_CHIP_KEY),
            Span::styled(name, theme::STYLE_CHIP_NAME),
        ])
    };
    let on = |key: &'static str, name: String| {
        Line::from(vec![
            Span::styled(key, theme::STYLE_CHIP_ON),
            Span::styled(name, theme::STYLE_CHIP_ON),
        ])
    };
    let stop_chip = if state.is_critical() {
        let filled = theme::STYLE_CONFIRM_YES;
        Line::from(vec![
            Span::styled(" [x]", filled),
            Span::styled(chip_name(Msg::ChipStop), filled),
        ])
    } else {
        Line::from(vec![
            Span::styled(" [x]", theme::STYLE_CHIP_QUIT_KEY),
            Span::styled(chip_name(Msg::ChipStop), theme::STYLE_CHIP_NAME),
        ])
    };
    let pause_chip = if state.paused {
        on(" [Space]", chip_name(Msg::ChipResume))
    } else {
        tonal(" [Space]", chip_name(Msg::ChipPause))
    };
    let pin_chip = if state.always_on_top {
        on(" [t]", chip_name(Msg::ChipPinOn))
    } else {
        tonal(" [t]", chip_name(Msg::ChipPinOff))
    };
    let blink_chip = if state.blink_enabled {
        tonal(" [b]", chip_name(Msg::ChipBlinkOn))
    } else {
        tonal(" [b]", chip_name(Msg::ChipBlinkOff))
    };
    let mut left_chips: Vec<(UiAction, Line<'static>)> = vec![
        (UiAction::EmergencyStop, stop_chip),
        (UiAction::TogglePause, pause_chip),
        (UiAction::Refresh, tonal(" [r]", chip_name(Msg::ChipRefresh))),
        (UiAction::TogglePin, pin_chip),
        (UiAction::ToggleBlink, blink_chip),
        (
            UiAction::ResetLayout,
            tonal(" [l]", chip_name(Msg::ChipResetLayout)),
        ),
    ];
    // 새 빌드 감지 시 경고색 강제 재시작 칩 맨 앞
    if state.exe_replaced {
        let warn = Style::default().fg(theme::COLOR_WARN).bg(theme::BG_TONAL);
        left_chips.insert(
            0,
            (
                UiAction::ForceRestart,
                Line::from(vec![
                    Span::styled(" [!]", warn.add_modifier(Modifier::BOLD)),
                    Span::styled(chip_name(Msg::ChipForceRestart), warn),
                ]),
            ),
        );
    }
    let language_chip = (
        UiAction::ToggleLanguage,
        tonal(" [L]", chip_name(Msg::ChipLanguage)),
    );
    let right_chips: Vec<(UiAction, Line<'static>)> = vec![
        (UiAction::ShowHelp, tonal(" [?]", chip_name(Msg::ChipHelp))),
        (
            UiAction::Quit,
            Line::from(vec![
                Span::styled(" [q]", theme::STYLE_CHIP_QUIT_KEY),
                Span::styled(chip_name(Msg::ChipQuit), theme::STYLE_CHIP_NAME),
            ]),
        ),
    ];

    // 2. 배치 (오른쪽 그룹은 오른쪽 끝 1열 앞, 왼쪽 그룹은 남는 폭까지만, 언어 칩은 자리를 남긴 채 다른 칩부터 생략)
    let row_end = row.x.saturating_add(row.width);
    let right_width: u16 = right_chips
        .iter()
        .map(|(_, line)| line.width() as u16 + 1)
        .sum::<u16>()
        .saturating_sub(1);
    let right_start = row_end.saturating_sub(1 + right_width).max(row.x);
    let mut placed: Vec<(UiAction, Rect, Line<'static>)> = Vec::new();
    let mut cursor = row.x.saturating_add(1);
    let language_width = language_chip.1.width() as u16;
    for (action, line) in left_chips {
        let width = line.width() as u16;
        if cursor.saturating_add(width + 1 + language_width) >= right_start {
            break;
        }
        placed.push((action, Rect::new(cursor, row.y, width, row.height), line));
        cursor = cursor.saturating_add(width + 1);
    }
    if cursor.saturating_add(language_width) < right_start {
        let (action, line) = language_chip;
        placed.push((action, Rect::new(cursor, row.y, language_width, row.height), line));
    }
    let mut cursor = right_start;
    for (action, line) in right_chips {
        let width = line.width() as u16;
        placed.push((action, Rect::new(cursor, row.y, width, row.height), line));
        cursor = cursor.saturating_add(width + 1);
    }

    // 3. 그리기와 클릭 영역 등록
    let chip = Style::default().bg(theme::BG_TONAL);
    for (action, rect, line) in placed {
        frame.render_widget(Paragraph::new(line).style(chip), rect);
        state.hit_areas.button_rects.push((action, rect));
    }
}

// 8. 하단 안내 줄 -------------------------------------------------------------
// 알림 먼저, 이어서 포인터·포커스 카드·탭 맥락 단축키 (꼬리 안내는 폭을 먼저 확보)
fn render_footer_hints(frame: &mut Frame, row: Rect, state: &AppState) {
    // 1. 알림 칩 (오류만 적색 채움, 최대 폭은 줄 폭 - 2)
    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    if let Some(toast) = &state.toast {
        let chip = Style::default().bg(theme::BG_TONAL);
        let (dot_style, text_style) = match toast.level {
            ToastLevel::Info => (chip.fg(theme::FG_ACCENT), chip.fg(theme::FG_TEXT)),
            ToastLevel::Success => (chip.fg(theme::COLOR_OK), chip.fg(theme::FG_TEXT)),
            ToastLevel::Error => (theme::STYLE_TOAST_ERROR, theme::STYLE_TOAST_ERROR),
        };
        let text_budget = usize::from(row.width).saturating_sub(2 + 4);
        let shown = if text_width(&toast.text) > text_budget {
            fit_display(&toast.text, text_budget).trim_end().to_string()
        } else {
            toast.text.clone()
        };
        spans.push(Span::styled(" ● ", dot_style));
        spans.push(Span::styled(shown, text_style));
        spans.push(Span::styled(" ", text_style));
    }

    // 2. 맥락 안내 (포인터 > 포커스 카드 종류 > 탭), 꼬리 안내는 항상 마지막
    let lang = state.lang;
    let hint_texts = |msgs: &[Msg]| -> Vec<&'static str> {
        msgs.iter().map(|msg| i18n::text(lang, *msg)).collect()
    };
    let mut context_hints: Vec<&str> = Vec::new();
    let tab_moving = state
        .tab_drag
        .is_some_and(|drag| drag.baseline != state.tab_order);
    if state.card_move.is_some() || tab_moving {
        // 카드 이동 중에는 놓기·취소 안내가 맨 앞, 다른 맥락 안내는 생략
        context_hints.extend(hint_texts(&[Msg::HintRelease, Msg::HintCancel]));
    } else {
        if state.split_hover.is_some() || state.split_drag.is_some() {
            context_hints.extend(hint_texts(&[Msg::HintDragCardSize, Msg::HintResetLayout]));
        }
        if state.column_hover.is_some() || state.column_drag.is_some() {
            context_hints.extend(hint_texts(&[
                Msg::HintDragColumnWidth,
                Msg::HintDoubleClickAuto,
                Msg::HintResetLayout,
            ]));
        }
        if state.scroll_hover.is_some() || state.scroll_drag.is_some() {
            context_hints.extend(hint_texts(&[Msg::HintDragScroll, Msg::HintTrackPage]));
        }
        match state.focused_card_id().kind() {
            CardKind::Selectable => {
                context_hints.extend(hint_texts(&[
                    Msg::HintSelectRow,
                    Msg::HintPage,
                    Msg::HintFirstLast,
                ]));
            }
            CardKind::Scrollable => {
                context_hints.extend(hint_texts(&[Msg::HintScroll, Msg::HintPage]));
            }
            CardKind::Static => {}
        }
        if state.focused_card_id().sort_table().is_some()
            || state.tab.main_card().sort_table().is_some()
        {
            context_hints.extend(hint_texts(&[Msg::HintSortColumn, Msg::HintSortOrder]));
        }
        if matches!(state.tab, ActiveTab::Overview | ActiveTab::Processes) {
            context_hints.extend(hint_texts(&[Msg::HintKill]));
        }
    }
    let tail_hints = hint_texts(&[Msg::HintNextTab, Msg::HintAllKeys]);

    // 3. 남은 폭에 들어가는 안내만 (키 글자 강조, 구분은 공백 2칸)
    let mut room = usize::from(row.width).saturating_sub(Line::from(spans.clone()).width());
    let tail_width: usize = tail_hints
        .iter()
        .map(|hint| 2 + text_width(hint))
        .sum();
    let reserved = if tail_width <= room { tail_width } else { 0 };
    let mut shown_hints: Vec<&str> = Vec::new();
    for hint in &context_hints {
        let piece_width = 2 + text_width(hint);
        if piece_width > room.saturating_sub(reserved) {
            continue;
        }
        shown_hints.push(hint);
        room -= piece_width;
    }
    for hint in &tail_hints {
        let piece_width = 2 + text_width(hint);
        if piece_width > room {
            continue;
        }
        shown_hints.push(hint);
        room -= piece_width;
    }
    let mut leading = state.toast.is_some();
    for hint in shown_hints {
        let (key, rest) = hint.split_once(' ').unwrap_or((hint, ""));
        if leading {
            spans.push(Span::raw("  "));
        }
        leading = true;
        spans.push(Span::styled(
            key.to_string(),
            Style::default()
                .fg(theme::HINT_KEY)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(format!(" {rest}"), theme::STYLE_DIM));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(theme::BG_BAR)),
        row,
    );
}

// 9. 겹침 공용: 바깥 어둡게, 왼쪽 밖 걸친 넓은 글자 지움 ----------------------
fn dim_backdrop_and_clear(frame: &mut Frame, area: Rect, rect: Rect) {
    let buf = frame.buffer_mut();
    buf.set_style(
        area,
        Style::default()
            .fg(theme::BORDER_DIM)
            .remove_modifier(Modifier::BOLD),
    );
    if rect.x > area.x {
        for row_y in rect.y..rect.bottom() {
            let cell = &mut buf[(rect.x - 1, row_y)];
            if text_width(cell.symbol()) > 1 {
                cell.set_symbol(" ");
            }
        }
    }
    frame.render_widget(Clear, rect);
}

// 10. 도움말 겹침 -------------------------------------------------------------
// 바깥은 어둡게, 이중선 상자에 키보드·마우스 두 구역
fn render_help_overlay(frame: &mut Frame, area: Rect, lang: Lang) {
    // 1. 도움말 내용 (뜻이 빈 줄 = 구역 제목)
    let help_rows = i18n::help_rows(lang);
    let width = area.width.saturating_sub(4).min(76);
    let height = (help_rows.len() as u16 + 4).min(area.height.saturating_sub(2));
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );

    // 2. 바깥 어둡게, 이중선 상자 (굵은 포커스 테두리와 구분)
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(theme::FG_ACCENT))
        .title(Span::styled(
            format!(" {} ", i18n::text(lang, Msg::HelpTitle)),
            theme::STYLE_STRONG,
        ))
        .title_bottom(
            Line::from(Span::styled(
                format!(" {} ", i18n::text(lang, Msg::HelpClose)),
                theme::STYLE_DIM,
            ))
            .right_aligned(),
        )
        .padding(Padding::horizontal(1))
        .style(Style::default().bg(theme::BG_PANEL));
    let inner = block.inner(rect);
    dim_backdrop_and_clear(frame, area, rect);
    frame.render_widget(block, rect);

    // 3. 줄 구성 (키 열 폭 = 가장 긴 키 + 2, 최대 24)
    let key_width = help_rows
        .iter()
        .filter(|(_, meaning)| !meaning.is_empty())
        .map(|(keys, _)| text_width(keys))
        .max()
        .map_or(0, |widest| (widest + 2).min(24));
    let mut lines: Vec<Line<'static>> = help_rows
        .iter()
        .map(|(keys, meaning)| {
            if meaning.is_empty() {
                return Line::from(Span::styled((*keys).to_string(), theme::STYLE_ACCENT));
            }
            Line::from(vec![
                Span::styled(
                    fit_display(keys, key_width),
                    Style::default()
                        .fg(theme::HINT_KEY)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::styled((*meaning).to_string(), theme::STYLE_TEXT),
            ])
        })
        .collect();

    // 4. 남는 높이가 줄 수보다 작으면 마지막 줄을 잘림 표시로 대체
    let room = usize::from(inner.height);
    if lines.len() > room {
        lines.truncate(room.saturating_sub(1));
        lines.push(Line::from(Span::styled(
            i18n::text(lang, Msg::HelpTruncated),
            theme::STYLE_DIM,
        )));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}

// 11. 종료 확인 겹침 ----------------------------------------------------------
// 위험 색 이중선 상자, 대상과 수치(긴급 중단은 대상 수와 이름 목록), 채움 Y 와 톤 N·Esc (둘 다 클릭 영역 등록)
fn render_kill_confirm(frame: &mut Frame, area: Rect, state: &mut AppState, confirm: &KillConfirm) {
    let lang = state.lang;
    let label = |msg: Msg| Span::styled(format!("{}  ", i18n::text(lang, msg)), theme::STYLE_DIM);

    // 1. 제목과 본문 줄 (프로세스 이름·PID·수치는 번역하지 않음)
    let (title_msg, lines): (Msg, Vec<Line<'static>>) = match confirm {
        KillConfirm::Selected(target) => {
            let risk = i18n::risk_label(lang, target.risk_level);
            let lines = vec![
                Line::from(vec![
                    label(Msg::KillTarget),
                    Span::styled(
                        format!(
                            "[{}] {} (PID {})",
                            target.actor_kind.badge(),
                            target.process_name,
                            target.pid
                        ),
                        theme::actor_style(target.actor_kind),
                    ),
                    Span::styled(format!(" · {risk}"), theme::risk_text_style(target.risk_level)),
                ]),
                Line::from(vec![
                    label(Msg::KillState),
                    Span::styled(
                        format!(
                            "ESTAB {} | RX {}/s | {} QPS",
                            target.established_conns,
                            format_bytes(target.avg_rx_bps),
                            target.avg_qps
                        ),
                        theme::STYLE_TEXT,
                    ),
                ]),
            ];
            (Msg::KillTitle, lines)
        }
        KillConfirm::Emergency(targets) => {
            let shown = targets.len().min(EMERGENCY_NAME_ROWS);
            let mut lines = vec![Line::from(vec![
                label(Msg::StopTargets),
                Span::styled(i18n::stop_target_count(lang, targets.len()), theme::STYLE_STRONG),
            ])];
            for target in &targets[..shown] {
                lines.push(Line::from(vec![
                    Span::raw("  "),
                    Span::styled(
                        format!(
                            "[{}] {} (PID {})",
                            target.actor_kind.badge(),
                            target.process_name,
                            target.pid
                        ),
                        theme::actor_style(target.actor_kind),
                    ),
                    Span::styled(
                        format!(" · {}", i18n::risk_label(lang, target.risk_level)),
                        theme::risk_text_style(target.risk_level),
                    ),
                ]));
            }
            if targets.len() > shown {
                lines.push(Line::from(Span::styled(
                    i18n::stop_more(lang, targets.len() - shown),
                    theme::STYLE_DIM,
                )));
            }
            (Msg::StopTitle, lines)
        }
    };

    // 2. 상자 (본문 줄 수 + 테두리 + 빈 줄 + 버튼 줄, 선택 종료는 9행 고정)
    let width = area.width.saturating_sub(4).min(68);
    let height = (lines.len() as u16 + 4).max(9).min(area.height.saturating_sub(2));
    let rect = Rect::new(
        area.x + (area.width - width) / 2,
        area.y + (area.height - height) / 2,
        width,
        height,
    );
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Double)
        .border_style(Style::default().fg(theme::COLOR_CRIT))
        .title(Span::styled(
            format!(" {} ", i18n::text(lang, title_msg)),
            Style::default()
                .fg(theme::COLOR_CRIT)
                .add_modifier(Modifier::BOLD),
        ))
        .padding(Padding::horizontal(1))
        .style(Style::default().bg(theme::BG_HEADER));
    let inner = block.inner(rect);
    dim_backdrop_and_clear(frame, area, rect);
    frame.render_widget(block, rect);
    frame.render_widget(Paragraph::new(lines), inner);

    // 3. 버튼 줄: 채움 Y, 명령 안내, 톤 N·Esc
    let button_y = inner.bottom().saturating_sub(1);
    let yes_text = format!(" {} ", i18n::text(lang, Msg::KillYes));
    let command_text = format!("  {}  ", i18n::text(lang, Msg::KillCommand));
    let no_text = format!(" {} ", i18n::text(lang, Msg::KillNo));
    let yes_rect = Rect::new(inner.x, button_y, text_width(&yes_text) as u16, 1).intersection(inner);
    let command_x = yes_rect.right();
    let no_x = command_x + text_width(&command_text) as u16;
    let no_rect = Rect::new(no_x, button_y, text_width(&no_text) as u16, 1).intersection(inner);
    frame.render_widget(
        Paragraph::new(Span::styled(yes_text, theme::STYLE_CONFIRM_YES)),
        yes_rect,
    );
    frame.render_widget(
        Paragraph::new(Span::styled(command_text, theme::STYLE_DIM)),
        Rect::new(command_x, button_y, no_x.saturating_sub(command_x), 1).intersection(inner),
    );
    frame.render_widget(
        Paragraph::new(Span::styled(no_text, theme::STYLE_CONFIRM_NO)),
        no_rect,
    );
    state.hit_areas.button_rects.push((UiAction::ConfirmKill, yes_rect));
    state.hit_areas.button_rects.push((UiAction::CancelKill, no_rect));
}

// 위험 등급 표 텍스트: 정상은 등급만, 그 외는 등급과 점수
pub fn risk_cell_text(lang: Lang, level: RiskLevel, score: u8) -> String {
    let label = i18n::risk_label(lang, level);
    if level == RiskLevel::Safe {
        label.to_string()
    } else {
        format!("{label} {score}")
    }
}
