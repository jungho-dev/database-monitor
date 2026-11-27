//! views/top_sql.rs
//! 과조회SQL 탭: 핑거프린트 랭킹 표와 선택 SQL 상세 (위아래, 순서 변경 가능).

use std::sync::Arc;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::text::{Line, Span};
use crate::app::model::MonitorSnapshot;
use crate::analysis::risk::format_bytes;
use crate::tui::cards::CardId;
use crate::tui::i18n::{self, Msg};
use crate::tui::layout::grid::{CardSlot, SlotHeight};
use crate::tui::sort;
use crate::tui::state::{ActiveTab, AppState};
use crate::tui::theme;
use crate::tui::views::common::{
    ColumnFit, CardTable, TableRow, render_card_table, render_text_card,
};
use crate::tui::views::overview::{metric_slots, render_metric_card};

// 1. 과조회SQL 탭 -------------------------------------------------------------
// 탭 격자: 0행 지표, 핑거프린트 랭킹 표, 선택 SQL 상세
pub fn render_top_sql_tab(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    // 1. 카드 조건 (지표 줄을 뺼 높이 기준 상세 높이)
    let rest_h = area.height.saturating_sub(4);
    let detail_h = (rest_h / 2).clamp(7, 14).min(rest_h.saturating_sub(5));
    let mut slots = metric_slots().to_vec();
    slots.extend([
        CardSlot {
            card: CardId::TopSqlRanking,
            min_w: 24,
            min_h: 4,
            height: SlotHeight::Fill,
            weight: 1,
            rank: 0,
            reserve: 0,
        },
        CardSlot {
            card: CardId::SqlDetail,
            min_w: 20,
            min_h: 3,
            height: SlotHeight::Fixed(detail_h),
            weight: 1,
            rank: 0,
            reserve: 0,
        },
    ]);

    // 2. 격자 배치와 카드 그리기
    let layout = state.card_layout;
    let placed = layout.arrange_tab(&mut state.hit_areas, ActiveTab::TopSql, area, &slots);
    for (card, rect) in placed {
        match card {
            CardId::Risk | CardId::Connections | CardId::Traffic | CardId::QueryRate => {
                render_metric_card(frame, rect, state, snap, card)
            }
            CardId::TopSqlRanking => render_ranking_table(frame, rect, state, snap),
            CardId::SqlDetail => render_sql_detail(frame, rect, state, snap),
            CardId::Processes
            | CardId::Sockets
            | CardId::SqlStream
            | CardId::Inspector
            | CardId::AlertLog
            | CardId::AlertDetail
            | CardId::Gate
            | CardId::ChartSqlMix
            | CardId::ChartTopTables => {}
        }
    }
}

// 2. 핑거프린트 랭킹 표 -------------------------------------------------------
fn render_ranking_table(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    let lang = state.lang;
    let columns = vec![
        (Msg::ColRisk, 6, ColumnFit::Pinned, Alignment::Left),
        (Msg::ColOp, 6, ColumnFit::Fixed, Alignment::Left),
        (Msg::ColTable, 0, ColumnFit::Shrink, Alignment::Left),
        (Msg::ColExecCount, 6, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColRepeat10s, 6, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColTotalRx, 8, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColMaxSingle, 8, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColLastProcess, 0, ColumnFit::Shrink, Alignment::Left),
        (Msg::ColDiagnosis, 0, ColumnFit::Grow, Alignment::Left),
    ];
    let view = sort::top_sql(snap, &state.sorts);
    let rows: Vec<TableRow> = view
        .iter()
        .map(|item| {
            let note = item
                .risk_reason
                .map_or_else(|| "-".to_string(), |reason| i18n::reason_text(lang, &reason));
            TableRow {
                cells: vec![
                    vec![Span::raw(i18n::risk_label(lang, item.risk_level))],
                    vec![Span::raw(item.op_kind.label())],
                    vec![Span::raw(item.target_table.clone())],
                    vec![Span::raw(i18n::times(lang, item.exec_count))],
                    vec![Span::raw(i18n::times(lang, item.repeat_10s))],
                    vec![Span::raw(format_bytes(item.total_rx_bytes))],
                    vec![Span::raw(format_bytes(item.max_single_rx_bytes))],
                    vec![Span::raw(format!("{}({})", item.last_process, item.last_pid))],
                    vec![Span::raw(format!("{note} | {}", item.sample_sql))],
                ],
                style: theme::risk_text_style(item.risk_level),
            }
        })
        .collect();
    render_card_table(
        frame,
        state,
        area,
        CardTable {
            card: CardId::TopSqlRanking,
            tone: theme::BORDER_DIM,
            title: i18n::text(lang, Msg::CardTopSql).to_string(),
            hint: i18n::text(lang, Msg::HintTopSql).to_string(),
            columns,
            rows,
            empty: (Msg::EmptyTopSql, Msg::EmptyTopSqlHint),
        },
    );
}

// 3. 선택 SQL 상세: 진단, 실행 지표, 행위자, 핑거프린트, 최근 실행 원문 -------
fn render_sql_detail(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    let lang = state.lang;
    let view = sort::top_sql(snap, &state.sorts);
    let selected = state.card_views[CardId::TopSqlRanking as usize]
        .selected
        .min(view.len().saturating_sub(1));
    let label = |msg: Msg| Span::styled(format!("{}  ", i18n::text(lang, msg)), theme::STYLE_DIM);
    let lines = match view.get(selected) {
        Some(item) => vec![
            Line::from(vec![
                Span::styled(
                    format!(" {} ", i18n::risk_label(lang, item.risk_level)),
                    theme::risk_badge_style(item.risk_level),
                ),
                Span::raw(" "),
                Span::styled(
                    item.risk_reason.map_or_else(
                        || i18n::text(lang, Msg::NoteNone).to_string(),
                        |reason| i18n::reason_text(lang, &reason),
                    ),
                    theme::risk_text_style(item.risk_level),
                ),
            ]),
            Line::from(vec![
                label(Msg::LabelExecution),
                Span::styled(
                    format!(
                        "{} {} · {} · 10s {} · {}/s · {} · {} · {}",
                        item.op_kind.label(),
                        item.target_table,
                        i18n::times(lang, item.exec_count),
                        i18n::times(lang, item.repeat_10s),
                        format_bytes(item.avg_rx_bps),
                        format_bytes(item.total_rx_bytes),
                        format_bytes(item.max_single_rx_bytes),
                        item.last_seen
                    ),
                    theme::STYLE_TEXT,
                ),
            ]),
            Line::from(vec![
                label(Msg::LabelActor),
                Span::styled(
                    format!(
                        "[{}] {} (PID {})",
                        item.actor_kind.badge(),
                        item.last_process,
                        item.last_pid
                    ),
                    theme::actor_style(item.actor_kind),
                ),
                Span::styled(" -> ", theme::STYLE_DIM),
                Span::styled(item.last_endpoint.clone(), theme::STYLE_STRONG),
            ]),
            Line::from(vec![
                label(Msg::LabelFingerprint),
                Span::styled(item.fingerprint.clone(), theme::STYLE_DIM),
            ]),
            Line::from(vec![
                label(Msg::LabelFullSql),
                Span::styled(item.sample_sql.clone(), Style::default().fg(theme::FG_ACCENT)),
            ]),
        ],
        None => vec![
            Line::from(Span::styled(
                i18n::text(lang, Msg::EmptySqlDetail),
                theme::STYLE_TEXT,
            )),
            Line::from(Span::styled(
                i18n::text(lang, Msg::EmptySqlDetailHint),
                theme::STYLE_DIM,
            )),
        ],
    };
    render_text_card(
        frame,
        state,
        CardId::SqlDetail,
        area,
        i18n::text(lang, Msg::CardSqlDetail),
        lines,
    );
}
