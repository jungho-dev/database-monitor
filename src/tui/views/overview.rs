//! views/overview.rs
//! 지표 카드(위험도·연결·트래픽·쿼리율)와 통합관제 탭: 프로세스 표, 소켓 표, 쿼리 스트림, 인스펙터.

use std::sync::Arc;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph, Sparkline};
use crate::app::model::{MonitorSnapshot, RiskLevel};
use crate::analysis::risk::format_bytes;
use crate::tui::cards::CardId;
use crate::tui::i18n::{self, Msg};
use crate::tui::layout::grid::{CardSlot, SlotHeight};
use crate::tui::sort;
use crate::tui::state::{ActiveTab, AppState};
use crate::tui::theme;
use crate::tui::views::charts;
use crate::tui::views::common::{
    ColumnFit, CardTable, TableRow, interactive_card, render_card_table, render_text_card,
};
use crate::tui::views::risk_cell_text;

// 1. 지표 카드 ----------------------------------------------------------------
// 모든 탭 격자 0행의 네 카드 조건 (기본 폭 3:3:4:4, 항상 표시)
pub fn metric_slots() -> [CardSlot; 4] {
    [
        CardSlot {
            card: CardId::Risk,
            min_w: 14,
            min_h: 3,
            height: SlotHeight::Fixed(4),
            weight: 3,
            rank: 0,
            reserve: 0,
        },
        CardSlot {
            card: CardId::Connections,
            min_w: 14,
            min_h: 3,
            height: SlotHeight::Fixed(4),
            weight: 3,
            rank: 0,
            reserve: 0,
        },
        CardSlot {
            card: CardId::Traffic,
            min_w: 14,
            min_h: 3,
            height: SlotHeight::Fixed(4),
            weight: 4,
            rank: 0,
            reserve: 0,
        },
        CardSlot {
            card: CardId::QueryRate,
            min_w: 14,
            min_h: 3,
            height: SlotHeight::Fixed(4),
            weight: 4,
            rank: 0,
            reserve: 0,
        },
    ]
}

// 지표 카드 한 장 (위험도·연결·트래픽·쿼리율, 다른 카드는 그리지 않음)
pub fn render_metric_card(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
    card: CardId,
) {
    // 1. 카드 강조 (위험 점멸 위상이면 위험도 외곽선이 위험색, 아니면 기본)
    let lang = state.lang;
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
    let risk_tone = if alarm_border {
        theme::COLOR_CRIT
    } else {
        theme::BORDER_DIM
    };
    let rx_style = if snap.total_rx_bps >= 1024 * 1024 {
        Style::default()
            .fg(theme::COLOR_WARN)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default()
            .fg(theme::COLOR_OK)
            .add_modifier(Modifier::BOLD)
    };

    // 2. 카드별 그리기
    match card {
        CardId::Risk => {
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
            let risk_card = interactive_card(
                state,
                CardId::Risk,
                area,
                i18n::text(lang, Msg::CardRisk),
                "",
                risk_tone,
            );
            frame.render_widget(Paragraph::new(risk_lines).block(risk_card), area);
        }
        CardId::Connections => {
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
                area,
                i18n::text(lang, Msg::CardConnections),
                "",
                theme::BORDER_DIM,
            );
            frame.render_widget(Paragraph::new(conn_lines).block(conn_card), area);
        }
        // 트래픽 카드: 값 1행 + 60초 스파크라인
        CardId::Traffic => {
            let rx_line = Line::from(vec![
                Span::styled("━ ", Style::new().fg(theme::CHART_RX)),
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
                area,
                i18n::text(lang, Msg::CardTraffic),
                &i18n::total_bytes(lang, &format_bytes(snap.total_rx_bytes)),
                theme::BORDER_DIM,
            );
            render_metric_with_trend(
                frame,
                area,
                rx_card,
                rx_line,
                &snap.rx_history_kb,
                theme::CHART_RX,
            );
        }
        CardId::QueryRate => {
            let qps_line = Line::from(vec![
                Span::styled("━ ", Style::new().fg(theme::CHART_QPS)),
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
                area,
                i18n::text(lang, Msg::CardQueryRate),
                &i18n::total_queries(lang, snap.total_queries),
                theme::BORDER_DIM,
            );
            render_metric_with_trend(
                frame,
                area,
                qps_card,
                qps_line,
                &snap.qps_history,
                theme::CHART_QPS,
            );
        }
        CardId::Processes
        | CardId::Sockets
        | CardId::SqlStream
        | CardId::Inspector
        | CardId::TopSqlRanking
        | CardId::SqlDetail
        | CardId::AlertLog
        | CardId::AlertDetail
        | CardId::Gate
        | CardId::ChartSqlMix
        | CardId::ChartTopTables => {}
    }
}

// 값 한 줄 아래 스파크라인 (높이가 모자라면 값만)
// value 첫 조각 = 범례 표식, 둘째 = 주 값: 둘이 한 줄에 다 들어가지 않으면 표식을 빼고 계열도 그리지 않음
fn render_metric_with_trend(
    frame: &mut Frame,
    area: Rect,
    block: Block<'static>,
    value: Line,
    history: &[u64],
    color: Color,
) {
    let inner = block.inner(area);
    frame.render_widget(block, area);
    if inner.height == 0 {
        return;
    }
    let rows = Layout::vertical([Constraint::Length(1), Constraint::Min(0)]).split(inner);
    let legend_width: usize = value.spans.iter().take(2).map(Span::width).sum();
    let legend_fits = legend_width <= usize::from(rows[0].width);
    let value = if legend_fits {
        value
    } else {
        Line::from(value.spans.into_iter().skip(1).collect::<Vec<_>>())
    };
    frame.render_widget(Paragraph::new(value), rows[0]);
    if !legend_fits {
        return;
    }
    let trend = charts::expand_trend(history, rows[1].width);
    let drawn_width = (trend.len() as u16).min(rows[1].width);
    let spark_area = Rect::new(
        rows[1].right() - drawn_width,
        rows[1].y,
        drawn_width,
        rows[1].height,
    );
    frame.render_widget(
        Sparkline::default()
            .data(trend.iter())
            .style(Style::new().fg(color)),
        spark_area,
    );
}

// 2. 통합관제 탭 --------------------------------------------------------------
// 탭 격자: 0행 지표, 표 두 장, 막대 차트 두 장, 쿼리 스트림, 인스펙터 (낮은 화면은 차트 줄, 스트림 순으로 숨김)
pub fn render_overview_tab(
    frame: &mut Frame,
    area: Rect,
    state: &mut AppState,
    snap: &Arc<MonitorSnapshot>,
) {
    // 1. 카드 조건 (지표 줄을 뺼 높이 기준 인스펙터·스트림 높이)
    let rest_h = area.height.saturating_sub(4);
    let inspector_h = (rest_h / 3).clamp(3, 6);
    let stream_h = (rest_h.saturating_sub(inspector_h).saturating_mul(3) / 10).max(3);
    let mut slots = metric_slots().to_vec();
    slots.extend([
        CardSlot {
            card: CardId::Processes,
            min_w: 24,
            min_h: 4,
            height: SlotHeight::Fill,
            weight: 55,
            rank: 0,
            reserve: 0,
        },
        CardSlot {
            card: CardId::Sockets,
            min_w: 20,
            min_h: 4,
            height: SlotHeight::Fill,
            weight: 45,
            rank: 0,
            reserve: 0,
        },
        CardSlot {
            card: CardId::ChartSqlMix,
            min_w: 24,
            min_h: 5,
            height: SlotHeight::Fixed(6),
            weight: 1,
            rank: 2,
            reserve: 10,
        },
        CardSlot {
            card: CardId::ChartTopTables,
            min_w: 24,
            min_h: 5,
            height: SlotHeight::Fixed(6),
            weight: 1,
            rank: 2,
            reserve: 10,
        },
        CardSlot {
            card: CardId::SqlStream,
            min_w: 30,
            min_h: 3,
            height: SlotHeight::Fixed(stream_h),
            weight: 1,
            rank: 1,
            reserve: 5,
        },
        CardSlot {
            card: CardId::Inspector,
            min_w: 20,
            min_h: 3,
            height: SlotHeight::Fixed(inspector_h),
            weight: 1,
            rank: 0,
            reserve: 0,
        },
    ]);

    // 2. 격자 배치와 카드 그리기
    let layout = state.card_layout;
    let placed = layout.arrange_tab(&mut state.hit_areas, ActiveTab::Overview, area, &slots);
    for (card, rect) in placed {
        match card {
            CardId::Risk | CardId::Connections | CardId::Traffic | CardId::QueryRate => {
                render_metric_card(frame, rect, state, snap, card)
            }
            CardId::Processes => render_process_table(frame, rect, state, snap),
            CardId::Sockets => render_socket_table(frame, rect, state, snap),
            CardId::SqlStream => render_stream_table(frame, rect, state, snap),
            CardId::Inspector => render_inspector(frame, rect, state),
            CardId::ChartSqlMix => charts::render_sql_mix(frame, rect, state, snap),
            CardId::ChartTopTables => charts::render_top_tables(frame, rect, state, snap),
            CardId::TopSqlRanking
            | CardId::SqlDetail
            | CardId::AlertLog
            | CardId::AlertDetail
            | CardId::Gate => {}
        }
    }
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
