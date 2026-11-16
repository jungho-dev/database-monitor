//! views/overview.rs
//! 지표 카드(위험도·연결·트래픽·쿼리율)와 통합관제 탭: 프로세스 표, 소켓 표, 쿼리 스트림, 인스펙터.

use std::sync::Arc;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Sparkline};
use crate::app::model::{MonitorSnapshot, RiskLevel};
use crate::analysis::risk::format_bytes;
use crate::tui::cards::CardId;
use crate::tui::i18n::{self, Msg};
use crate::tui::layout::{SplitAxis, SplitGroup};
use crate::tui::sort;
use crate::tui::state::AppState;
use crate::tui::theme;
use crate::tui::views::common::{
    ColumnFit, CardTable, TableRow, interactive_card, render_card_table, render_text_card,
};
use crate::tui::views::risk_cell_text;

// 1. 지표 카드 줄 -------------------------------------------------------------
// 기본 3:3:4:4 배치, 카드 사이 경계 드래그, 카드 순서 변경 가능
pub fn render_metric_cards(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    // 1. 분할
    let lang = state.lang;
    let default_cols = Layout::horizontal([
        Constraint::Fill(3),
        Constraint::Fill(3),
        Constraint::Fill(4),
        Constraint::Fill(4),
    ])
    .spacing(1)
    .split(area);
    let default_cuts: Vec<u16> = default_cols[..3]
        .iter()
        .map(|col| col.right() - area.x)
        .collect();
    let cols = state.card_layout.split_cards(
        &mut state.hit_areas,
        SplitGroup::MetricCols,
        SplitAxis::Columns,
        area,
        &default_cuts,
        &[14, 14, 14, 14],
    );

    // 2. 위험도 카드 (위험 점멸 위상이면 외곽선이 위험색, 아니면 기본)
    let is_danger = snap.overall_risk >= RiskLevel::Critical;
    let alarm_border = is_danger && (state.blink_on || !state.blink_enabled);
    let badge_style = if is_danger && state.blink_on {
        Style::default()
            .fg(theme::FG_ON_ACCENT)
            .bg(theme::ALARM_FLASH)
            .add_modifier(Modifier::BOLD)
    } else {
        theme::risk_badge_style(snap.overall_risk)
    };
    let risk_lines = vec![
        Line::from(vec![
            Span::styled(
                format!(
                    " {} {} ",
                    i18n::risk_label(lang, snap.overall_risk),
                    snap.overall_risk_score
                ),
                badge_style,
            ),
            Span::raw(" "),
            Span::styled(
                i18n::risk_guidance(lang, snap.overall_risk),
                theme::risk_text_style(snap.overall_risk),
            ),
        ]),
        Line::from(Span::styled(
            i18n::summary_text(lang, &snap.risk_summary),
            theme::STYLE_DIM,
        )),
    ];
    let risk_tone = if alarm_border {
        theme::COLOR_CRIT
    } else {
        theme::BORDER_DIM
    };
    let risk_card = interactive_card(
        state,
        CardId::Risk,
        cols[0],
        i18n::text(lang, Msg::CardRisk),
        "",
        risk_tone,
    );
    frame.render_widget(Paragraph::new(risk_lines).block(risk_card), cols[0]);

    // 3. 연결 카드
    let conn_lines = vec![
        Line::from(vec![
            Span::styled(format!("{} ", i18n::text(lang, Msg::MetricEstab)), theme::STYLE_DIM),
            Span::styled(snap.active_established.to_string(), theme::STYLE_STRONG),
            Span::styled(format!("  {} ", i18n::text(lang, Msg::MetricWait)), theme::STYLE_DIM),
            Span::styled(snap.active_time_wait.to_string(), theme::STYLE_TEXT),
            Span::styled(format!("  {} ", i18n::text(lang, Msg::MetricSyn)), theme::STYLE_DIM),
            Span::styled(snap.active_syn_sent.to_string(), theme::STYLE_TEXT),
        ]),
        Line::from(Span::styled(
            i18n::local_excluded(lang, snap.local_established),
            theme::STYLE_DIM,
        )),
    ];
    let conn_card = interactive_card(
        state,
        CardId::Connections,
        cols[1],
        i18n::text(lang, Msg::CardConnections),
        "",
        theme::BORDER_DIM,
    );
    frame.render_widget(Paragraph::new(conn_lines).block(conn_card), cols[1]);

    // 4. 트래픽 카드: 값 1행 + 60초 스파크라인
    let rx_style = if snap.total_rx_bps >= 1024 * 1024 {
        Style::default()
            .fg(theme::COLOR_WARN)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme::COLOR_OK)
            .add_modifier(Modifier::BOLD)
    };
    let rx_line = Line::from(vec![
        Span::styled(
            format!(
                "{} {}/s",
                i18n::text(lang, Msg::MetricRx),
                format_bytes(snap.total_rx_bps)
            ),
            rx_style,
        ),
        Span::styled(
            format!(
                "  {} {}/s",
                i18n::text(lang, Msg::MetricTx),
                format_bytes(snap.total_tx_bps)
            ),
            theme::STYLE_TEXT,
        ),
    ]);
    let rx_card = interactive_card(
        state,
        CardId::Traffic,
        cols[2],
        i18n::text(lang, Msg::CardTraffic),
        &i18n::total_bytes(lang, &format_bytes(snap.total_rx_bytes)),
        theme::BORDER_DIM,
    );
    render_metric_with_trend(
        frame,
        cols[2],
        rx_card,
        rx_line,
        &snap.rx_history_kb,
        theme::risk_text_style(snap.overall_risk),
    );

    // 5. 쿼리율 카드
    let qps_line = Line::from(vec![
        Span::styled(
            format!("{} {}", snap.total_qps, i18n::text(lang, Msg::MetricQps)),
            theme::STYLE_ACCENT,
        ),
        Span::styled(
            format!("  {}", i18n::alert_count(lang, snap.alerts.len())),
            theme::STYLE_TEXT,
        ),
    ]);
    let qps_card = interactive_card(
        state,
        CardId::QueryRate,
        cols[3],
        i18n::text(lang, Msg::CardQueryRate),
        &i18n::total_queries(lang, snap.total_queries),
        theme::BORDER_DIM,
    );
    render_metric_with_trend(
        frame,
        cols[3],
        qps_card,
        qps_line,
        &snap.qps_history,
        Style::default().fg(theme::FG_ACCENT),
    );
}

// 값 한 줄 아래 스파크라인 (높이가 모자라면 값만)
fn render_metric_with_trend(
    frame: &mut Frame,
    area: Rect,
    block: Block<'static>,
    value: Line,
    history: &[u64],
    trend_style: Style,
) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);
    frame.render_widget(Paragraph::new(value), rows[0]);
    let recent = &history[history.len().saturating_sub(usize::from(rows[1].width))..];
    frame.render_widget(Sparkline::default().data(recent).style(trend_style), rows[1]);
}

// 2. 통합관제 탭 --------------------------------------------------------------
// 작은 화면: 프로세스 표·인스펙터 우선, 쿼리 스트림은 3행 미만이면 생략
pub fn render_overview_tab(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    // 1. 상하 분할
    let inspector_h = (area.height / 3).clamp(3, 6);
    let top_h = ((u32::from(area.height) * 52 / 100) as u16)
        .max(5)
        .min(area.height.saturating_sub(inspector_h));
    let stream_fits = area.height.saturating_sub(inspector_h + top_h) >= 3;
    let (top_area, stream_area, inspector_area) = if stream_fits {
        let rows = state.card_layout.split_cards(
            &mut state.hit_areas,
            SplitGroup::OverviewRows,
            SplitAxis::Rows,
            area,
            &[top_h, area.height.saturating_sub(inspector_h)],
            &[5, 3, 3],
        );
        (rows[0], Some(rows[1]), rows[2])
    } else {
        let rows = state.card_layout.split_cards(
            &mut state.hit_areas,
            SplitGroup::OverviewCompactRows,
            SplitAxis::Rows,
            area,
            &[area.height.saturating_sub(inspector_h)],
            &[5, 3],
        );
        (rows[0], None, rows[1])
    };

    // 2. 위 줄 좌우 분할 (프로세스 표, 소켓 표 순서 변경 가능)
    let top_cols = state.card_layout.split_cards(
        &mut state.hit_areas,
        SplitGroup::OverviewCols,
        SplitAxis::Columns,
        top_area,
        &[(u32::from(top_area.width) * 55 / 100) as u16],
        &[24, 20],
    );

    // 3. 카드 그리기
    render_process_table(frame, top_cols[0], state, snap);
    render_socket_table(frame, top_cols[1], state, snap);
    if let Some(stream_area) = stream_area {
        render_stream_table(frame, stream_area, state, snap);
    }
    render_inspector(frame, inspector_area, state);
}

// 3. 프로세스 표 --------------------------------------------------------------
// 두 탭이 같이 쓰는 선택 표, 위험 점멸 위상에서 위험 행 강조
pub fn render_process_table(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    let lang = state.lang;
    let alarm_blink = state.blink_on;
    let alarm_border =
        snap.overall_risk >= RiskLevel::Critical && (state.blink_on || !state.blink_enabled);
    let tone = if alarm_border {
        theme::COLOR_CRIT
    } else {
        theme::BORDER_DIM
    };
    let columns = vec![
        (Msg::ColKind, 8, ColumnFit::Fixed, Alignment::Left),
        (Msg::ColPid, 5, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColProcess, 0, ColumnFit::Shrink, Alignment::Left),
        (Msg::ColEstab, 5, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColWait, 4, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColAvgRx, 8, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColAvgQps, 7, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColRepeat10s, 6, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColRisk, 6, ColumnFit::Pinned, Alignment::Left),
    ];
    let rows: Vec<TableRow> = sort::processes(snap, &state.sorts)
        .into_iter()
        .map(|item| {
            let style = if item.risk_level >= RiskLevel::Critical && alarm_blink {
                Style::default()
                    .bg(theme::BG_DANGER_TINT)
                    .fg(theme::ALARM_FLASH)
                    .add_modifier(Modifier::BOLD)
            } else {
                theme::risk_text_style(item.risk_level)
            };
            TableRow {
                cells: vec![
                    vec![Span::raw(format!("[{}]", item.actor_kind.badge()))],
                    vec![Span::raw(item.pid.to_string())],
                    vec![Span::raw(item.process_name.clone())],
                    vec![Span::raw(item.established_conns.to_string())],
                    vec![Span::raw(item.time_wait_conns.to_string())],
                    vec![Span::raw(format_bytes(item.avg_rx_bps))],
                    vec![Span::raw(item.avg_qps.to_string())],
                    vec![Span::raw(i18n::times(lang, item.max_repeat_10s))],
                    vec![Span::raw(risk_cell_text(lang, item.risk_level, item.risk_score))],
                ],
                style,
            }
        })
        .collect();
    render_card_table(
        frame,
        state,
        area,
        CardTable {
            card: CardId::Processes,
            tone,
            title: i18n::text(lang, Msg::CardProcesses).to_string(),
            hint: i18n::text(lang, Msg::HintProcesses).to_string(),
            columns,
            rows,
            empty: (Msg::EmptyProcesses, Msg::EmptyProcessesHint),
        },
    );
}

// 4. 소켓 표 ------------------------------------------------------------------
fn render_socket_table(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    let lang = state.lang;
    let columns = vec![
        (Msg::ColPid, 5, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColProcess, 0, ColumnFit::Shrink, Alignment::Left),
        (Msg::ColEndpoint, 0, ColumnFit::Grow, Alignment::Left),
        (Msg::ColState, 5, ColumnFit::Fixed, Alignment::Left),
        (Msg::ColRxPerSec, 6, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColRiskShort, 4, ColumnFit::Pinned, Alignment::Left),
    ];
    let rows: Vec<TableRow> = sort::sockets(snap, &state.sorts)
        .into_iter()
        .map(|conn| {
            let risk = if conn.is_local {
                i18n::text(lang, Msg::SocketExcluded)
            } else {
                i18n::risk_label(lang, conn.risk_level)
            };
            TableRow {
                cells: vec![
                    vec![Span::raw(conn.pid.to_string())],
                    vec![Span::raw(conn.process_name.clone())],
                    vec![Span::raw(conn.endpoint_label.clone())],
                    vec![Span::raw(conn.state.label())],
                    vec![Span::raw(format_bytes(conn.rx_bps))],
                    vec![Span::raw(risk)],
                ],
                style: if conn.is_local {
                    theme::STYLE_DIM
                } else {
                    theme::risk_text_style(conn.risk_level)
                },
            }
        })
        .collect();
    render_card_table(
        frame,
        state,
        area,
        CardTable {
            card: CardId::Sockets,
            tone: theme::BORDER_DIM,
            title: i18n::text(lang, Msg::CardSockets).to_string(),
            hint: i18n::text(lang, Msg::HintSockets).to_string(),
            columns,
            rows,
            empty: (Msg::EmptySockets, Msg::EmptySocketsHint),
        },
    );
}

// 5. 쿼리 스트림 표 -----------------------------------------------------------
fn render_stream_table(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    let lang = state.lang;
    let columns = vec![
        (Msg::ColTime, 8, ColumnFit::Fixed, Alignment::Left),
        (Msg::ColActor, 0, ColumnFit::Shrink, Alignment::Left),
        (Msg::ColOp, 6, ColumnFit::Fixed, Alignment::Left),
        (Msg::ColTable, 0, ColumnFit::Shrink, Alignment::Left),
        (Msg::ColRxSize, 7, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColRepeat10s, 6, ColumnFit::Fixed, Alignment::Right),
        (Msg::ColRiskShort, 4, ColumnFit::Pinned, Alignment::Left),
        (Msg::ColPayload, 0, ColumnFit::Grow, Alignment::Left),
    ];
    let rows: Vec<TableRow> = sort::events(snap, &state.sorts)
        .into_iter()
        .map(|event| TableRow {
            cells: vec![
                vec![Span::raw(event.timestamp.clone())],
                vec![Span::raw(format!("[{}] {}", event.actor_kind.badge(), event.process_name))],
                vec![Span::raw(event.op_kind.label())],
                vec![Span::raw(event.target_table.clone())],
                vec![Span::raw(format_bytes(event.rx_bytes))],
                vec![Span::raw(i18n::times(lang, event.repeat_10s))],
                vec![Span::raw(i18n::risk_label(lang, event.risk_level))],
                vec![Span::raw(event.sql_preview.clone())],
            ],
            style: theme::risk_text_style(event.risk_level),
        })
        .collect();
    render_card_table(
        frame,
        state,
        area,
        CardTable {
            card: CardId::SqlStream,
            tone: theme::BORDER_DIM,
            title: i18n::text(lang, Msg::CardSqlStream).to_string(),
            hint: i18n::text(lang, Msg::HintSqlStream).to_string(),
            columns,
            rows,
            empty: (Msg::EmptyStream, Msg::EmptyStreamHint),
        },
    );
}

// 6. 선택 프로세스 인스펙터 ---------------------------------------------------
// 프로세스 표의 선택 행 상세 (SQL 원문은 마지막 줄, 긴 줄은 줄바꿈, 넘치면 스크롤)
pub fn render_inspector(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let lang = state.lang;
    let label = |msg: Msg| Span::styled(format!("{}  ", i18n::text(lang, msg)), theme::STYLE_DIM);
    let lines = match state.selected_process() {
        Some(item) => {
            let sql = if item.last_sql.is_empty() {
                i18n::text(lang, Msg::PayloadWaiting).to_string()
            } else {
                item.last_sql.clone()
            };
            vec![
                Line::from(vec![
                    label(Msg::LabelProcess),
                    Span::styled(
                        format!(
                            "[{}] {} (PID {})",
                            item.actor_kind.badge(),
                            item.process_name,
                            item.pid
                        ),
                        theme::actor_style(item.actor_kind),
                    ),
                    Span::styled(format!("  {}  ", i18n::text(lang, Msg::LabelTargetDb)), theme::STYLE_DIM),
                    Span::styled(item.target_endpoints.join(", "), theme::STYLE_STRONG),
                ]),
                Line::from(vec![
                    label(Msg::LabelDiagnosis),
                    Span::styled(
                        format!(
                            "[{}] {}",
                            i18n::risk_label(lang, item.risk_level),
                            i18n::reasons_text(lang, &item.risk_reasons, " · ")
                        ),
                        theme::risk_text_style(item.risk_level),
                    ),
                ]),
                Line::from(vec![
                    label(Msg::LabelCommand),
                    Span::styled(item.command_line.clone(), theme::STYLE_TEXT),
                ]),
                Line::from(vec![
                    label(Msg::LabelLastSql),
                    Span::styled(sql, Style::default().fg(theme::FG_ACCENT)),
                ]),
            ]
        }
        None => vec![
            Line::from(Span::styled(
                i18n::text(lang, Msg::EmptyInspector),
                theme::STYLE_TEXT,
            )),
            Line::from(Span::styled(
                i18n::text(lang, Msg::EmptyInspectorHint),
                theme::STYLE_DIM,
            )),
        ],
    };
    render_text_card(
        frame,
        state,
        CardId::Inspector,
        area,
        i18n::text(lang, Msg::CardInspector),
        lines,
    );
}
