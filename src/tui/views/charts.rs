//! views/charts.rs
//! 막대 차트 카드(SQL 구성, 수신량 상위 테이블)와 지표 카드 스파크라인 확장.

use std::cmp::Reverse;
use std::collections::BTreeMap;
use std::sync::Arc;
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Padding, Paragraph};
use crate::app::model::{MonitorSnapshot, RiskLevel, SqlOpKind};
use crate::analysis::risk::format_bytes;
use crate::tui::cards::CardId;
use crate::tui::format::{fit_display, pad_left_display, text_width};
use crate::tui::i18n::{self, Msg};
use crate::tui::state::AppState;
use crate::tui::theme;
use crate::tui::views::common::{interactive_card, render_notice_card};

// 막대를 그리는 최소 폭 (이름과 값 뒤 남는 폭)
const BAR_MIN_WIDTH: usize = 3;

// 막대 한 줄 (value = 막대 길이 기준, flag = 경고 꼬리)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BarRow {
    pub label: String,
    pub value: u64,
    pub value_text: String,
    pub flag: String,
}

// 가로 막대 카드 입력 (rows = 그릴 순서)
struct BarCard {
    card: CardId,
    title: &'static str,
    color: Color,
    rows: Vec<BarRow>,
}

// 1. SQL 구성 행 --------------------------------------------------------------
// 작업 종류별 실행 수 합, 많은 순 (같으면 선언 순서)
pub fn sql_mix_rows(snap: &MonitorSnapshot) -> Vec<BarRow> {
    // 1. 작업 종류별 합
    let kinds = [
        SqlOpKind::Select,
        SqlOpKind::Insert,
        SqlOpKind::Update,
        SqlOpKind::Delete,
        SqlOpKind::Ddl,
        SqlOpKind::ShowMeta,
        SqlOpKind::Transaction,
        SqlOpKind::PrepareExec,
        SqlOpKind::PingQuit,
        SqlOpKind::EncryptedFlow,
        SqlOpKind::SocketConnect,
    ];
    let mut sums: Vec<(SqlOpKind, u64)> = kinds
        .iter()
        .map(|kind| {
            let sum: u64 = snap
                .top_sql
                .iter()
                .filter(|stats| stats.op_kind == *kind)
                .map(|stats| stats.exec_count)
                .sum();
            (*kind, sum)
        })
        .filter(|(_, sum)| *sum > 0)
        .collect();
    sums.sort_by_key(|(_, sum)| Reverse(*sum));

    // 2. 행 구성 (값 문구 = 실행 수와 비율)
    let total = sums.iter().map(|(_, sum)| *sum).sum::<u64>().max(1);
    sums.into_iter()
        .map(|(kind, sum)| {
            let share = sum as f64 * 100.0 / total as f64;
            BarRow {
                label: kind.label().to_string(),
                value: sum,
                value_text: format!("{sum} {share:>3.0}%"),
                flag: String::new(),
            }
        })
        .collect()
}

// 2. 수신량 상위 테이블 행 ----------------------------------------------------
// 테이블별 수신 바이트 합, 많은 순 (같으면 이름 순), 경고 이상 지문 수는 꼬리
pub fn top_table_rows(snap: &MonitorSnapshot) -> Vec<BarRow> {
    // 1. 테이블별 합 (이름 없음은 -)
    let mut tables: BTreeMap<String, (u64, usize)> = BTreeMap::new();
    for stats in &snap.top_sql {
        let name = if stats.target_table.is_empty() {
            "-"
        } else {
            stats.target_table.as_str()
        };
        let entry = tables.entry(name.to_string()).or_default();
        entry.0 = entry.0.saturating_add(stats.total_rx_bytes);
        entry.1 += usize::from(stats.risk_level >= RiskLevel::Warning);
    }

    // 2. 행 구성 (이름 순 정렬 뒤 수신량 순 안정 정렬)
    let mut rows: Vec<BarRow> = tables
        .into_iter()
        .map(|(name, (rx_bytes, warnings))| BarRow {
            label: name,
            value: rx_bytes,
            value_text: format_bytes(rx_bytes),
            flag: if warnings > 0 {
                format!("!{warnings}")
            } else {
                String::new()
            },
        })
        .collect();
    rows.sort_by_key(|row| Reverse(row.value));
    rows
}

// 3. SQL 구성 카드 ------------------------------------------------------------
pub fn render_sql_mix(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    let title = i18n::text(state.lang, Msg::CardSqlMix);
    let rows = sql_mix_rows(snap);
    if rows.is_empty() {
        let notice = (Msg::EmptyTopSql, Msg::EmptyTopSqlHint);
        render_notice_card(frame, state, CardId::ChartSqlMix, area, title, notice);
        return;
    }
    let card = BarCard {
        card: CardId::ChartSqlMix,
        title,
        color: theme::CHART_SQL_MIX,
        rows,
    };
    render_bar_card(frame, state, area, card);
}

// 4. 수신량 상위 테이블 카드 --------------------------------------------------
pub fn render_top_tables(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    let title = i18n::text(state.lang, Msg::CardTopTables);
    let rows = top_table_rows(snap);
    if rows.is_empty() {
        let notice = (Msg::EmptyTopSql, Msg::EmptyTopSqlHint);
        render_notice_card(frame, state, CardId::ChartTopTables, area, title, notice);
        return;
    }
    let card = BarCard {
        card: CardId::ChartTopTables,
        title,
        color: theme::CHART_TABLES,
        rows,
    };
    render_bar_card(frame, state, area, card);
}

// 5. 막대 ---------------------------------------------------------------------
// 채운 칸은 주어진 색, 빈 칸은 흐린 트랙, 0 보다 크면 최소 한 칸
pub fn build_bar(pct: Option<f64>, width: usize, fill_style: Style) -> Vec<Span<'static>> {
    let ratio = pct.map_or(0.0, |value| value.clamp(0.0, 100.0) / 100.0);
    let rounded = ((ratio * width as f64).round() as usize).min(width);
    let filled = if ratio > 0.0 && width > 0 {
        rounded.max(1)
    } else {
        rounded
    };
    vec![
        Span::styled("█".repeat(filled), fill_style),
        Span::styled("░".repeat(width - filled), theme::STYLE_FAINT),
    ]
}

// 6. 스파크라인 확장 ----------------------------------------------------------
// 최근 width 개 표본을 같은 칸 수로 반복 (결과 길이는 width 이하)
pub fn expand_trend(history: &[u64], width: u16) -> Vec<u64> {
    let width = usize::from(width);
    let tail = &history[history.len().saturating_sub(width)..];
    let repeat = (width / history.len().max(1)).max(1);
    tail.iter()
        .flat_map(|sample| std::iter::repeat_n(*sample, repeat))
        .collect()
}

// 7. 가로 막대 카드 (내부) ----------------------------------------------------
// 줄마다 이름, 막대, 값, 경고 꼬리 (폭이 모자라면 막대부터 생략)
fn render_bar_card(frame: &mut Frame, state: &mut AppState, area: Rect, card: BarCard) {
    // 1. 카드 테두리
    let block = interactive_card(state, card.card, area, card.title, "", theme::BORDER_DIM)
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.is_empty() {
        return;
    }

    // 2. 열 폭 (보이는 행 기준), 높이가 남으면 행 사이 빈 줄
    let width = usize::from(inner.width);
    let height = usize::from(inner.height);
    let shown = &card.rows[..card.rows.len().min(height)];
    let spaced = shown.len() * 2 <= height + 1;
    let mut label_natural = 0usize;
    let mut value_width = 0usize;
    let mut flag_width = 0usize;
    for row in shown {
        label_natural = label_natural.max(text_width(&row.label));
        value_width = value_width.max(text_width(&row.value_text));
        flag_width = flag_width.max(text_width(&row.flag));
    }
    let flag_room = if flag_width > 0 { flag_width + 1 } else { 0 };
    let fixed_width = 1 + value_width + flag_room;
    let label_width = label_natural.min(width.saturating_sub(fixed_width));
    let bar_width = width.saturating_sub(label_width + fixed_width + 1);
    let peak = shown.iter().map(|row| row.value).max().unwrap_or(0).max(1);

    // 3. 줄 조립
    let bar_style = Style::default().fg(card.color);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(shown.len());
    for row in shown {
        if spaced && !lines.is_empty() {
            lines.push(Line::default());
        }
        let mut spans = vec![Span::styled(
            format!("{} ", fit_display(&row.label, label_width)),
            theme::STYLE_DIM,
        )];
        if bar_width >= BAR_MIN_WIDTH {
            let pct = row.value as f64 * 100.0 / peak as f64;
            spans.extend(build_bar(Some(pct), bar_width, bar_style));
            spans.push(Span::raw(" "));
        }
        spans.push(Span::styled(
            pad_left_display(&row.value_text, value_width),
            theme::STYLE_TEXT,
        ));
        if !row.flag.is_empty() {
            spans.push(Span::styled(
                format!(" {}", row.flag),
                theme::STYLE_CRIT.add_modifier(Modifier::BOLD),
            ));
        }
        lines.push(Line::from(spans));
    }
    frame.render_widget(Paragraph::new(lines), inner);
}
