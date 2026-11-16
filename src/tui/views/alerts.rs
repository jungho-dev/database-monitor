//! views/alerts.rs
//! 위험경보 탭: 경보 로그 표와 선택 경보 상세 (위아래, 순서 변경 가능).

use std::sync::Arc;
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use crate::app::model::MonitorSnapshot;
use crate::tui::cards::CardId;
use crate::tui::i18n::{self, Msg};
use crate::tui::layout::{SplitAxis, SplitGroup};
use crate::tui::sort;
use crate::tui::state::AppState;
use crate::tui::theme;
use crate::tui::views::common::{
    ColumnFit, CardTable, TableRow, render_card_table, render_text_card,
};

// 1. 위험경보 탭 --------------------------------------------------------------
pub fn render_alerts_tab(
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
        SplitGroup::AlertsRows,
        SplitAxis::Rows,
        area,
        &[area.height.saturating_sub(detail_h)],
        &[5, 3],
    );

    // 1. 경보 로그 표
    let columns = vec![
        (Msg::ColTime, 8, ColumnFit::Fixed, Alignment::Left),
        (Msg::ColLevel, 4, ColumnFit::Pinned, Alignment::Left),
        (Msg::ColRule, 0, ColumnFit::Shrink, Alignment::Left),
        (Msg::ColTitle, 0, ColumnFit::Shrink, Alignment::Left),
        (Msg::ColDetail, 0, ColumnFit::Grow, Alignment::Left),
    ];
    let view = sort::alerts(snap, &state.sorts);
    let rows: Vec<TableRow> = view
        .iter()
        .map(|alert| TableRow {
            cells: vec![
                vec![Span::raw(alert.timestamp.clone())],
                vec![Span::styled(
                    i18n::risk_label(lang, alert.level),
                    theme::risk_text_style(alert.level),
                )],
                vec![Span::raw(alert.rule_code.code())],
                vec![Span::raw(alert.title.clone())],
                vec![Span::raw(i18n::reasons_text(lang, &alert.detail, ", "))],
            ],
            style: theme::STYLE_TEXT,
        })
        .collect();
    render_card_table(
        frame,
        state,
        cards[0],
        CardTable {
            card: CardId::AlertLog,
            tone: theme::BORDER_DIM,
            title: i18n::text(lang, Msg::CardAlertLog).to_string(),
            hint: i18n::alert_log_hint(lang, snap.alerts.len()),
            columns,
            rows,
            empty: (Msg::EmptyAlerts, Msg::EmptyAlertsHint),
        },
    );

    // 2. 선택 경보 상세: 사유, 대상 DB, 경보 시점 최근 SQL 원문
    let selected = state.card_views[CardId::AlertLog as usize]
        .selected
        .min(view.len().saturating_sub(1));
    let label = |msg: Msg| Span::styled(format!("{}  ", i18n::text(lang, msg)), theme::STYLE_DIM);
    let lines = match view.get(selected) {
        Some(alert) => {
            let mut lines = vec![
                Line::from(vec![
                    Span::styled(
                        format!(" {} ", i18n::risk_label(lang, alert.level)),
                        theme::risk_badge_style(alert.level),
                    ),
                    Span::raw(" "),
                    Span::styled(
                        format!("{} [{}] {}", alert.timestamp, alert.rule_code.code(), alert.title),
                        Style::default()
                            .fg(theme::FG_STRONG)
                            .add_modifier(Modifier::BOLD),
                    ),
                ]),
                Line::from(vec![
                    label(Msg::LabelReason),
                    Span::styled(
                        i18n::reasons_text(lang, &alert.detail, ", "),
                        theme::risk_text_style(alert.level),
                    ),
                ]),
                Line::from(vec![
                    label(Msg::LabelTargetDb),
                    Span::styled(alert.remote_endpoint.clone(), theme::STYLE_STRONG),
                ]),
            ];
            if alert.recent_sql.is_empty() {
                lines.push(Line::from(vec![
                    label(Msg::LabelRecentSql),
                    Span::styled(i18n::text(lang, Msg::NoteNoRecentSql), theme::STYLE_DIM),
                ]));
            }
            for sql in &alert.recent_sql {
                lines.push(Line::from(vec![
                    label(Msg::LabelRecentSql),
                    Span::styled(
                        i18n::recent_sql_text(lang, sql),
                        Style::default().fg(theme::FG_ACCENT),
                    ),
                ]));
            }
            lines
        }
        None => vec![
            Line::from(Span::styled(
                i18n::text(lang, Msg::EmptyAlertDetail),
                theme::STYLE_TEXT,
            )),
            Line::from(Span::styled(
                i18n::text(lang, Msg::EmptyAlertDetailHint),
                theme::STYLE_DIM,
            )),
        ],
    };
    render_text_card(
        frame,
        state,
        CardId::AlertDetail,
        cards[1],
        i18n::text(lang, Msg::CardAlertDetail),
        lines,
    );
}
