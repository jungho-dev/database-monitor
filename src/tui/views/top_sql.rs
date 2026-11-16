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
use crate::tui::layout::{SplitAxis, SplitGroup};
use crate::tui::sort;
use crate::tui::state::AppState;
use crate::tui::theme;
use crate::tui::views::common::{
    ColumnFit, CardTable, TableRow, render_card_table, render_text_card,
};

// 1. 과조회SQL 탭 -------------------------------------------------------------
pub fn render_top_sql_tab(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    let lang = state.lang;
    let detail_h = (area.height / 2)
        .clamp(7, 14)
        .min(area.height.saturating_sub(5));
    let cards = state.card_layout.split_cards(
        &mut state.hit_areas,
        SplitGroup::TopSqlRows,
        SplitAxis::Rows,
        area,
        &[area.height.saturating_sub(detail_h)],
        &[5, 3],
    );

    // 1. 핑거프린트 랭킹 표
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
        cards[0],
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

    // 2. 선택 SQL 상세: 진단, 실행 지표, 행위자, 핑거프린트, 최근 실행 원문
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
        cards[1],
        i18n::text(lang, Msg::CardSqlDetail),
        lines,
    );
}
