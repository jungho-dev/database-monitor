//! decoder.rs
//! 패시브 TCP 페이로드 기반 MySQL·PostgreSQL·Redis 프로토콜 디코더 및 SQL 정규화.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::time::Instant;
use crate::analysis::reassembly::{FlowReassembler, MAX_FLOW_BYTES, SegmentOutcome, TcpFlowKey};
use crate::app::config::MonitorConfig;
use crate::app::model::{DbEngine, RiskLevel, RiskReason, SqlOpKind};

const MAX_SQL_BYTES: usize = 16 * 1024;

// 1. 디코딩된 와이어 프레임 ---------------------------------------------------
#[derive(Debug, Clone)]
pub struct WireFrame {
    pub src_ip: IpAddr,
    pub src_port: u16,
    pub dst_ip: IpAddr,
    pub dst_port: u16,
    pub payload_len: usize,
    pub decoded_sql: Option<DecodedSql>,
    pub captured_at: Instant,
}

#[derive(Debug, Clone)]
pub struct DecodedSql {
    pub engine: DbEngine,
    pub op_kind: SqlOpKind,
    pub target_table: String,
    pub fingerprint: String,
    pub preview: String,
    pub full_sql: String,
    pub complete: bool,
    pub pattern_risk: RiskLevel,
    pub pattern_note: Option<RiskReason>,
}

// 2. L2/L3/L4 패킷 헤더 파싱 --------------------------------------------------
pub fn decode_packet(
    raw: &[u8],
    datalink: i32,
    watch: &MonitorConfig,
    flows: &mut FlowReassembler,
) -> Option<WireFrame> {
    // 1. 데이터링크 오프셋 산출
    let (ip_offset, ethertype_hint) = match datalink {
        1 => {
            if raw.len() < 14 {
                return None;
            }
            let et = u16::from_be_bytes([raw[12], raw[13]]);
            if et == 0x8100 && raw.len() >= 18 {
                (18, Some(u16::from_be_bytes([raw[16], raw[17]])))
            } else {
                (14, Some(et))
            }
        }
        0 | 108 => {
            if raw.len() < 4 {
                return None;
            }
            (4, None)
        }
        12 | 101 => (0, None),
        _ => {
            if raw.len() >= 14 {
                (14, Some(u16::from_be_bytes([raw[12], raw[13]])))
            } else {
                return None;
            }
        }
    };

    if raw.len() <= ip_offset {
        return None;
    }
    let l3 = &raw[ip_offset..];
    let version = l3[0] >> 4;

    // 2. IPv4 및 IPv6 분기
    let (src_ip, dst_ip, l4_proto, l4) = match (version, ethertype_hint) {
        (4, _) | (_, Some(0x0800)) => {
            if l3.len() < 20 {
                return None;
            }
            let ihl = ((l3[0] & 0x0f) as usize) * 4;
            if ihl < 20 || l3.len() < ihl {
                return None;
            }
            let proto = l3[9];
            let src = IpAddr::V4(Ipv4Addr::new(l3[12], l3[13], l3[14], l3[15]));
            let dst = IpAddr::V4(Ipv4Addr::new(l3[16], l3[17], l3[18], l3[19]));
            (src, dst, proto, &l3[ihl..])
        }
        (6, _) | (_, Some(0x86dd)) => {
            if l3.len() < 40 {
                return None;
            }
            let proto = l3[6];
            let src_bytes: [u8; 16] = l3[8..24].try_into().ok()?;
            let dst_bytes: [u8; 16] = l3[24..40].try_into().ok()?;
            let src = IpAddr::V6(Ipv6Addr::from(src_bytes));
            let dst = IpAddr::V6(Ipv6Addr::from(dst_bytes));
            (src, dst, proto, &l3[40..])
        }
        _ => return None,
    };

    // 3. TCP 헤더 및 페이로드 추출
    if l4_proto != 6 || l4.len() < 20 {
        return None;
    }
    let src_port = u16::from_be_bytes([l4[0], l4[1]]);
    let dst_port = u16::from_be_bytes([l4[2], l4[3]]);
    let seq = u32::from_be_bytes([l4[4], l4[5], l4[6], l4[7]]);
    let data_offset = ((l4[12] >> 4) as usize) * 4;
    if data_offset < 20 || l4.len() < data_offset {
        return None;
    }
    let payload = &l4[data_offset..];

    // 4. 감시 대상 방향 판별: DB로 가는 요청만 SQL 디코딩, DB 응답(RX)은 길이만 집계
    // (응답 세그먼트마다 하던 문자열 변환·키워드 검색 제거, 비감시 트래픽은 채널에 넣지 않음)
    let is_outbound = watch.is_watched_endpoint(&dst_ip, dst_port);
    if !is_outbound && !watch.is_watched_endpoint(&src_ip, src_port) {
        return None;
    }
    let captured_at = Instant::now();
    let decoded_sql = if is_outbound && !payload.is_empty() {
        // 선언 길이보다 짧은 요청은 재조립 버퍼에 모아 한 번에 해독
        let key = TcpFlowKey { src_ip, src_port, dst_ip, dst_port };
        let declared_len = declared_message_len(DbEngine::from_port(dst_port), payload);
        match flows.accept_segment(key, seq, payload, declared_len, captured_at) {
            SegmentOutcome::Whole => decode_wire_sql(dst_port, payload),
            SegmentOutcome::Held => None,
            SegmentOutcome::Assembled(message) => decode_wire_sql(dst_port, &message),
        }
    } else {
        None
    };

    Some(WireFrame {
        src_ip,
        src_port,
        dst_ip,
        dst_port,
        payload_len: payload.len(),
        decoded_sql,
        captured_at,
    })
}

// 3. DB 와이어 페이로드 분석 --------------------------------------------------
pub fn decode_wire_sql(dst_port: u16, payload: &[u8]) -> Option<DecodedSql> {
    let engine = DbEngine::from_port(dst_port);

    // 1. MySQL 패킷 분석 (3바이트 길이 + 1바이트 시퀀스 + 1바이트 커맨드)
    if matches!(engine, DbEngine::MySql) && payload.len() >= 5 {
        let pkt_len = (payload[0] as usize)
            | ((payload[1] as usize) << 8)
            | ((payload[2] as usize) << 16);
        let cmd = payload[4];
        if pkt_len >= 1 && pkt_len <= payload.len() + MAX_FLOW_BYTES {
            // 선언 길이만큼 캡처되어야 완전한 문장 (0xFFFFFF 분할 패킷은 위 상한에 걸려 폴백)
            let wire_len = 4 + pkt_len;
            let text_complete = payload.len() >= wire_len;
            let body = &payload[5..payload.len().min(wire_len)];
            match cmd {
                0x03 => {
                    if let Some(sql) = extract_clean_sql(body) {
                        return Some(analyze_sql_text(engine, &sql, text_complete));
                    }
                }
                0x16 => {
                    if let Some(sql) = extract_clean_sql(body) {
                        let mut analyzed = analyze_sql_text(engine, &sql, text_complete);
                        analyzed.op_kind = SqlOpKind::PrepareExec;
                        return Some(analyzed);
                    }
                }
                0x17 => {
                    return Some(DecodedSql {
                        engine,
                        op_kind: SqlOpKind::PrepareExec,
                        target_table: "PREPARED".to_string(),
                        fingerprint: "EXECUTE_STMT".to_string(),
                        preview: "[COM_STMT_EXECUTE] prepared statement execute".to_string(),
                        full_sql: "[COM_STMT_EXECUTE] binary execute, SQL text visible only at PREPARE"
                            .to_string(),
                        complete: true,
                        pattern_risk: RiskLevel::Safe,
                        pattern_note: None,
                    });
                }
                0x02 => {
                    if let Some(schema) = extract_clean_sql(body) {
                        let schema = collapse_whitespace(&schema);
                        return Some(DecodedSql {
                            engine,
                            op_kind: SqlOpKind::ShowMeta,
                            target_table: schema.clone(),
                            fingerprint: format!("USE {schema}"),
                            preview: format!("USE {schema}"),
                            full_sql: format!("USE {schema}"),
                            complete: true,
                            pattern_risk: RiskLevel::Safe,
                            pattern_note: None,
                        });
                    }
                }
                0x01 | 0x0e => {
                    return Some(DecodedSql {
                        engine,
                        op_kind: SqlOpKind::PingQuit,
                        target_table: "-".to_string(),
                        fingerprint: if cmd == 0x0e { "COM_PING" } else { "COM_QUIT" }.to_string(),
                        preview: if cmd == 0x0e { "[COM_PING]" } else { "[COM_QUIT]" }.to_string(),
                        full_sql: String::new(),
                        complete: true,
                        pattern_risk: RiskLevel::Safe,
                        pattern_note: None,
                    });
                }
                _ => {}
            }
        }
    }

    // 2. PostgreSQL Simple/Extended Query 분석 ('Q' / 'P', 1바이트 태그 + int32 길이)
    if matches!(engine, DbEngine::PostgreSql)
        && let Some(wire_len) = declared_message_len(engine, payload)
    {
        let body = &payload[5..payload.len().min(wire_len)];
        // 'P' 는 문장 이름 뒤가 질의문
        let query = if payload[0] == b'P' {
            body.iter()
                .position(|&b| b == 0)
                .map_or(&[][..], |end| &body[end + 1..])
        } else {
            body
        };
        if let Some(sql) = extract_clean_sql(query) {
            return Some(analyze_sql_text(engine, &sql, payload.len() >= wire_len));
        }
    }

    // 3. Redis RESP 명령은 SQL 아님 (값 안의 SELECT·UPDATE 단어 오인 방지, 트래픽만 집계)
    if matches!(engine, DbEngine::Redis) {
        let text = String::from_utf8_lossy(&payload[..payload.len().min(64)]);
        let command_token = if text.starts_with('*') {
            text.split("\r\n").nth(2).unwrap_or("")
        } else {
            text.split_whitespace().next().unwrap_or("")
        };
        let command = if !command_token.is_empty()
            && command_token.len() <= 24
            && command_token.chars().all(|c| c.is_ascii_alphabetic() || c == '.')
        {
            command_token.to_ascii_uppercase()
        } else {
            "DATA".to_string()
        };
        return Some(DecodedSql {
            engine,
            op_kind: SqlOpKind::EncryptedFlow,
            target_table: "-".to_string(),
            fingerprint: format!("REDIS::{command}"),
            preview: format!("[REDIS {command} {}B]", payload.len()),
            full_sql: String::new(),
            complete: true,
            pattern_risk: RiskLevel::Safe,
            pattern_note: None,
        });
    }

    // 4. 일반 텍스트 SQL 폴백 스캔 (끝을 알 수 없어 불완전 문장)
    if let Some(sql) = scan_ascii_sql(payload) {
        return Some(analyze_sql_text(engine, &sql, false));
    }

    // 5. TLS 암호화 또는 바이너리 스트림 분류
    if payload.len() >= 16 {
        return Some(DecodedSql {
            engine,
            op_kind: SqlOpKind::EncryptedFlow,
            target_table: "-".to_string(),
            fingerprint: format!("{}::BINARY_OR_TLS_FLOW", engine.label()),
            preview: format!("[{} binary/TLS packet {}B]", engine.label(), payload.len()),
            full_sql: String::new(),
            complete: true,
            pattern_risk: RiskLevel::Safe,
            pattern_note: None,
        });
    }

    None
}

// 재조립 대상 메시지의 선언 전체 길이 (MySQL 텍스트 커맨드, PostgreSQL Q/P)
fn declared_message_len(engine: DbEngine, payload: &[u8]) -> Option<usize> {
    match engine {
        DbEngine::MySql if payload.len() >= 5 && matches!(payload[4], 0x03 | 0x16) => {
            let pkt_len = (payload[0] as usize)
                | ((payload[1] as usize) << 8)
                | ((payload[2] as usize) << 16);
            (pkt_len >= 1).then_some(4 + pkt_len)
        }
        DbEngine::PostgreSql if payload.len() >= 5 && matches!(payload[0], b'Q' | b'P') => {
            let msg_len = i32::from_be_bytes([payload[1], payload[2], payload[3], payload[4]]);
            usize::try_from(msg_len)
                .ok()
                .filter(|&len| len >= 5)
                .map(|len| 1 + len)
        }
        _ => None,
    }
}

// 4. SQL 원문 추출 및 정규화 --------------------------------------------------
fn extract_clean_sql(bytes: &[u8]) -> Option<String> {
    let end = bytes.iter().position(|&b| b == 0).unwrap_or(bytes.len());
    let slice = &bytes[..end];
    if slice.is_empty() {
        return None;
    }
    let printable = slice
        .iter()
        .filter(|&&b| b.is_ascii_graphic() || b.is_ascii_whitespace() || b >= 0x80)
        .count();
    if printable * 10 < slice.len() * 8 {
        return None;
    }
    let text = String::from_utf8_lossy(slice).trim().to_string();
    if text.len() < 3 {
        return None;
    }
    Some(text)
}

fn scan_ascii_sql(bytes: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(bytes);
    let collapsed = collapse_whitespace(&text);
    let upper = collapsed.to_ascii_uppercase();
    // 가장 앞선 키워드 위치부터 (목록 순서가 아니라 문장 시작 기준)
    let start = [
        "SELECT ",
        "INSERT ",
        "UPDATE ",
        "DELETE ",
        "SHOW ",
        "DESCRIBE ",
        "EXPLAIN ",
        "ALTER ",
        "CREATE ",
        "DROP ",
        "TRUNCATE ",
    ]
    .iter()
    .filter_map(|kw| upper.find(kw))
    .min()?;
    let clean: String = collapsed[start..]
        .chars()
        .take_while(|&c| !c.is_control())
        .collect();
    (clean.len() >= 6).then_some(clean)
}

// text_complete 가 false 이거나 상한에 걸린 문장은 불완전 (조건 부재 규칙 판정 보류)
pub fn analyze_sql_text(engine: DbEngine, raw_sql: &str, text_complete: bool) -> DecodedSql {
    // 1. 공백 접기와 바이트 상한
    let mut sql = collapse_whitespace(raw_sql);
    let mut complete = text_complete;
    if sql.len() > MAX_SQL_BYTES {
        let mut cut = MAX_SQL_BYTES;
        while !sql.is_char_boundary(cut) {
            cut -= 1;
        }
        sql.truncate(cut);
        complete = false;
    }
    let upper = sql.to_ascii_uppercase();

    // 2. 주석을 뺀 분류용 코드
    let code_upper = collapse_whitespace(&strip_sql_comments(raw_sql)).to_ascii_uppercase();
    let first_word = code_upper.split_whitespace().next().unwrap_or("");

    // 3. SQL 명령 분류
    let op_kind = match first_word {
        "SELECT" | "WITH" => SqlOpKind::Select,
        "INSERT" | "REPLACE" => SqlOpKind::Insert,
        "UPDATE" => SqlOpKind::Update,
        "DELETE" => SqlOpKind::Delete,
        "CREATE" | "ALTER" | "DROP" | "TRUNCATE" | "RENAME" => SqlOpKind::Ddl,
        "SHOW" | "DESCRIBE" | "DESC" | "EXPLAIN" | "SET" | "USE" => SqlOpKind::ShowMeta,
        "BEGIN" | "START" | "COMMIT" | "ROLLBACK" | "SAVEPOINT" => SqlOpKind::Transaction,
        "PREPARE" | "EXECUTE" => SqlOpKind::PrepareExec,
        _ => SqlOpKind::ShowMeta,
    };

    // 4. 대상 테이블 추출 및 핑거프린트 생성
    let target_table = extract_target_table(&code_upper, op_kind);
    let fingerprint = build_sql_fingerprint(&upper);

    // 5. 위험 쿼리 패턴 진단
    let (pattern_risk, pattern_note) = evaluate_sql_pattern(&code_upper, op_kind, complete);

    // 6. 표 셀용 미리보기와 하단 상세용 원문 분리 (잘린 문장은 말줄임 표시)
    let full_sql = if complete { sql } else { format!("{sql} ...") };
    let preview = if full_sql.chars().count() > 140 {
        let head: String = full_sql.chars().take(140).collect();
        format!("{head}...")
    } else {
        full_sql.clone()
    };

    DecodedSql {
        engine,
        op_kind,
        target_table,
        fingerprint,
        preview,
        full_sql,
        complete,
        pattern_risk,
        pattern_note,
    }
}

// 5. 대상 테이블 및 핑거프린트 빌더 -------------------------------------------
fn extract_target_table(upper: &str, op_kind: SqlOpKind) -> String {
    let tokens: Vec<&str> = upper.split_whitespace().collect();
    let marker = match op_kind {
        SqlOpKind::Select | SqlOpKind::Delete => "FROM",
        SqlOpKind::Insert => "INTO",
        SqlOpKind::Update => "UPDATE",
        SqlOpKind::Ddl => "TABLE",
        _ => "",
    };

    if !marker.is_empty() {
        for idx in 0..tokens.len() {
            if tokens[idx] == marker && idx + 1 < tokens.len() {
                let candidate = tokens[idx + 1]
                    .trim_matches(|c: char| c == '`' || c == '"' || c == '\'' || c == '(' || c == ';');
                if !candidate.is_empty() && candidate != "SELECT" {
                    return candidate.to_string();
                }
            }
        }
    }
    "-".to_string()
}

pub fn build_sql_fingerprint(upper: &str) -> String {
    let mut out = String::with_capacity(upper.len().min(120));
    let mut chars = upper.chars().peekable();
    let mut in_single = false;

    while let Some(ch) = chars.next() {
        if ch == '\'' {
            if !in_single {
                out.push('?');
            }
            in_single = !in_single;
            continue;
        }
        if in_single {
            continue;
        }
        if ch.is_ascii_digit() {
            out.push('?');
            while let Some(&next) = chars.peek() {
                if next.is_ascii_digit() || next == '.' {
                    chars.next();
                } else {
                    break;
                }
            }
            continue;
        }
        out.push(ch);
        if out.len() >= 110 {
            break;
        }
    }
    collapse_whitespace(&out)
}

// 문장 하나만으로 DB에 피해를 줄 수 있는 경우만 위험, 조건 있는 쓰기·SELECT * 는 앱 정상 동작
// 조회량·반복 위험은 문장이 아니라 실측 RX·빈도로 risk.rs 에서 판정
// 조건 부재 규칙은 완전한 문장에서만, 잘린 문장은 판정 보류
fn evaluate_sql_pattern(
    code_upper: &str,
    op_kind: SqlOpKind,
    complete: bool,
) -> (RiskLevel, Option<RiskReason>) {
    // 1. 데이터·스키마 파괴
    if matches!(op_kind, SqlOpKind::Ddl)
        && (code_upper.starts_with("DROP ") || code_upper.starts_with("TRUNCATE "))
    {
        return (RiskLevel::Critical, Some(RiskReason::DestructiveDdl));
    }
    if complete
        && matches!(op_kind, SqlOpKind::Delete | SqlOpKind::Update)
        && !has_top_level_keyword(code_upper, "WHERE")
    {
        return (RiskLevel::Critical, Some(RiskReason::UnconditionalWrite));
    }
    if code_upper.contains("SLEEP(") || code_upper.contains("BENCHMARK(") {
        return (RiskLevel::Critical, Some(RiskReason::BlockingFunction));
    }

    // 2. 잠금·재작성이 길어질 수 있는 스키마 변경
    if matches!(op_kind, SqlOpKind::Ddl) && code_upper.starts_with("ALTER ") {
        return (RiskLevel::Warning, Some(RiskReason::SchemaChange));
    }

    // 3. 조건·LIMIT 없는 전체 조회는 관찰만 (코드테이블·소형 테이블이 대부분)
    if complete
        && matches!(op_kind, SqlOpKind::Select)
        && has_top_level_keyword(code_upper, "FROM")
        && !has_top_level_keyword(code_upper, "WHERE")
        && !has_top_level_keyword(code_upper, "LIMIT")
    {
        return (RiskLevel::Caution, Some(RiskReason::FullScan));
    }

    (RiskLevel::Safe, None)
}

fn collapse_whitespace(input: &str) -> String {
    input.split_whitespace().collect::<Vec<_>>().join(" ")
}

// 6. SQL 주석 제거와 최상위 키워드 판정 ---------------------------------------
// `-- `, `/* */` 주석을 공백으로 바꾸고 문자열·백틱 안은 그대로 둠
fn strip_sql_comments(raw_sql: &str) -> String {
    let bytes = raw_sql.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut idx = 0;
    while idx < bytes.len() {
        let byte = bytes[idx];
        match byte {
            b'\'' | b'"' | b'`' => {
                let end = skip_quoted(bytes, idx);
                out.extend_from_slice(&bytes[idx..end]);
                idx = end;
            }
            b'-' if bytes.get(idx + 1) == Some(&b'-')
                && bytes.get(idx + 2).is_none_or(|next| next.is_ascii_whitespace()) =>
            {
                idx += bytes[idx..].iter().position(|&b| b == b'\n').unwrap_or(bytes.len() - idx);
                out.push(b' ');
            }
            b'/' if bytes.get(idx + 1) == Some(&b'*') => {
                idx = raw_sql[idx + 2..]
                    .find("*/")
                    .map_or(bytes.len(), |end| idx + 2 + end + 2);
                out.push(b' ');
            }
            _ => {
                out.push(byte);
                idx += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

// 여는 따옴표 위치에서 닫는 따옴표 다음 위치를 돌려줌 (두 겹 따옴표·역슬래시 이스케이프, 미종결이면 끝)
fn skip_quoted(bytes: &[u8], start: usize) -> usize {
    let quote = bytes[start];
    let mut idx = start + 1;
    while idx < bytes.len() {
        if bytes[idx] == b'\\' && quote != b'`' {
            idx += 2;
        } else if bytes[idx] == quote {
            if bytes.get(idx + 1) == Some(&quote) {
                idx += 2;
            } else {
                return idx + 1;
            }
        } else {
            idx += 1;
        }
    }
    bytes.len()
}

// 문자열·백틱 밖, 괄호 깊이 0 에서 키워드 토큰이 있는지 (대소문자 무시, 입력은 주석 제거본)
fn has_top_level_keyword(code: &str, keyword: &str) -> bool {
    let bytes = code.as_bytes();
    let word = keyword.as_bytes();
    let is_word_byte = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'$' || byte >= 0x80;
    let mut depth = 0usize;
    let mut idx = 0;
    while idx < bytes.len() {
        match bytes[idx] {
            b'\'' | b'"' | b'`' => idx = skip_quoted(bytes, idx),
            b'(' => {
                depth += 1;
                idx += 1;
            }
            b')' => {
                depth = depth.saturating_sub(1);
                idx += 1;
            }
            byte if is_word_byte(byte) => {
                let end = idx + bytes[idx..].iter().take_while(|&&b| is_word_byte(b)).count();
                if depth == 0 && bytes[idx..end].eq_ignore_ascii_case(word) {
                    return true;
                }
                idx = end;
            }
            _ => idx += 1,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // 원시 IPv4(DLT_RAW 12) + TCP 헤더 20바이트씩, 데이터 오프셋 5워드
    fn build_tcp_packet(
        src: [u8; 4],
        sport: u16,
        dst: [u8; 4],
        dport: u16,
        seq: u32,
        payload: &[u8],
    ) -> Vec<u8> {
        let mut raw = vec![0u8; 40];
        raw[0] = 0x45;
        raw[9] = 6;
        raw[12..16].copy_from_slice(&src);
        raw[16..20].copy_from_slice(&dst);
        raw[20..22].copy_from_slice(&sport.to_be_bytes());
        raw[22..24].copy_from_slice(&dport.to_be_bytes());
        raw[24..28].copy_from_slice(&seq.to_be_bytes());
        raw[32] = 0x50;
        raw.extend_from_slice(payload);
        raw
    }

    // 선언 길이를 declared_extra 만큼 늘려 뒷부분이 캡처되지 않은 패킷을 흉내 냄
    fn build_mysql_query(sql: &str, declared_extra: usize) -> Vec<u8> {
        let pkt_len = sql.len() + 1 + declared_extra;
        let mut packet = vec![pkt_len as u8, (pkt_len >> 8) as u8, (pkt_len >> 16) as u8, 0, 0x03];
        packet.extend_from_slice(sql.as_bytes());
        packet
    }

    fn build_postgres_query(tag: u8, name: Option<&str>, sql: &str, declared_extra: usize) -> Vec<u8> {
        let mut body = Vec::new();
        if let Some(name) = name {
            body.extend_from_slice(name.as_bytes());
            body.push(0);
        }
        body.extend_from_slice(sql.as_bytes());
        body.push(0);
        if tag == b'P' {
            body.extend_from_slice(&[0, 0]);
        }
        let msg_len = (4 + body.len() + declared_extra) as u32;
        let mut message = vec![tag];
        message.extend_from_slice(&msg_len.to_be_bytes());
        message.extend_from_slice(&body);
        message
    }

    // 줄마다 들여쓰기가 큰 MyBatis 형태 UPDATE (열마다 40칸 들여쓰기)
    fn build_wide_update_sql(column_count: usize, tail: &str) -> String {
        let mut sql = String::from("UPDATE tb_cont_cono\n    SET\n");
        for idx in 0..column_count {
            sql.push_str(&format!("{}CO_COL_{idx} = #{{col{idx}}},\n", " ".repeat(40)));
        }
        sql.push_str(tail);
        sql
    }

    #[test]
    fn conditional_select_star_is_safe_and_keeps_full_sql() {
        let res = analyze_sql_text(
            DbEngine::MySql,
            "SELECT * FROM tb_payment_history WHERE user_id = 4219",
            true,
        );
        assert_eq!(res.op_kind, SqlOpKind::Select);
        assert_eq!(res.target_table, "TB_PAYMENT_HISTORY");
        assert_eq!(res.pattern_risk, RiskLevel::Safe);
        assert!(res.fingerprint.contains("USER_ID = ?"));
        assert_eq!(res.full_sql, "SELECT * FROM tb_payment_history WHERE user_id = 4219");

        let scan = analyze_sql_text(DbEngine::MySql, "SELECT * FROM tb_code", true);
        assert_eq!(scan.pattern_risk, RiskLevel::Caution);

        let long_sql = format!("SELECT {} FROM tb_order WHERE id = 1", "col_a, ".repeat(60));
        let long = analyze_sql_text(DbEngine::MySql, &long_sql, true);
        assert!(long.preview.ends_with("..."));
        assert!(long.full_sql.ends_with("WHERE id = 1"));
    }

    #[test]
    fn detects_unbounded_delete_as_critical() {
        let res = analyze_sql_text(DbEngine::MySql, "DELETE FROM tb_outbox_event", true);
        assert_eq!(res.op_kind, SqlOpKind::Delete);
        assert_eq!(res.pattern_risk, RiskLevel::Critical);

        let wire = decode_wire_sql(3306, &build_mysql_query("DELETE FROM tb_outbox_event", 0))
            .expect("delete");
        assert!(wire.complete);
        assert_eq!(wire.pattern_risk, RiskLevel::Critical);
        assert_eq!(wire.pattern_note, Some(RiskReason::UnconditionalWrite));
    }

    #[test]
    fn conditional_write_is_safe_and_alter_is_warning() {
        let update = analyze_sql_text(
            DbEngine::MySql,
            "UPDATE tb_user SET last_login_dt = NOW() WHERE user_id = 'u1'",
            true,
        );
        let insert = analyze_sql_text(
            DbEngine::MySql,
            "INSERT INTO tb_access_log (user_id, menu_id) VALUES ('u1', 'm1')",
            true,
        );
        let alter = analyze_sql_text(
            DbEngine::MySql,
            "ALTER TABLE tb_user ADD COLUMN memo VARCHAR(10)",
            true,
        );
        let truncate = analyze_sql_text(DbEngine::MySql, "TRUNCATE TABLE tb_user", true);
        assert_eq!(update.pattern_risk, RiskLevel::Safe);
        assert_eq!(insert.pattern_risk, RiskLevel::Safe);
        assert_eq!(alter.pattern_risk, RiskLevel::Warning);
        assert_eq!(truncate.pattern_risk, RiskLevel::Critical);
    }

    #[test]
    fn long_indented_update_keeps_trailing_where_and_is_safe() {
        let sql = build_wide_update_sql(80, "    WHERE CO_NO = ?");
        assert!(sql.len() > 4096);

        let decoded = decode_wire_sql(5002, &build_mysql_query(&sql, 0)).expect("update");
        assert_eq!(decoded.op_kind, SqlOpKind::Update);
        assert_eq!(decoded.target_table, "TB_CONT_CONO");
        assert!(decoded.complete);
        assert_eq!(decoded.pattern_risk, RiskLevel::Safe);
        assert_eq!(decoded.pattern_note, None);
        assert!(decoded.full_sql.ends_with("WHERE CO_NO = ?"));
    }

    #[test]
    fn mysql_update_shorter_than_declared_length_is_incomplete_and_safe() {
        let head = build_wide_update_sql(30, "");
        let decoded = decode_wire_sql(5002, &build_mysql_query(&head, 5000)).expect("update");
        assert_eq!(decoded.op_kind, SqlOpKind::Update);
        assert!(!decoded.complete);
        assert_eq!(decoded.pattern_risk, RiskLevel::Safe);
        assert_eq!(decoded.pattern_note, None);
        assert!(decoded.full_sql.ends_with(" ..."));

        let split = decode_wire_sql(5002, &build_mysql_query("DELETE FROM tb_user", 0xFF_FFFF - 20))
            .expect("split packet");
        assert!(!split.complete);
        assert_eq!(split.pattern_risk, RiskLevel::Safe);
    }

    #[test]
    fn sql_over_byte_cap_is_incomplete_and_safe() {
        let sql = format!("UPDATE tb_user SET {}", "memo = 'a', ".repeat(2000));
        assert!(sql.len() > MAX_SQL_BYTES);
        let decoded = analyze_sql_text(DbEngine::MySql, &sql, true);
        assert!(!decoded.complete);
        assert_eq!(decoded.pattern_risk, RiskLevel::Safe);

        let folded = format!("UPDATE tb_user SET memo = 1{}WHERE id = 1", " \n\t".repeat(2000));
        let kept = analyze_sql_text(DbEngine::MySql, &folded, true);
        assert!(kept.complete);
        assert_eq!(kept.pattern_risk, RiskLevel::Safe);
    }

    #[test]
    fn incomplete_sql_skips_absence_rules_but_keeps_prefix_rules() {
        let write = analyze_sql_text(DbEngine::MySql, "DELETE FROM tb_user", false);
        let scan = analyze_sql_text(DbEngine::MySql, "SELECT * FROM tb_code", false);
        let drop = analyze_sql_text(DbEngine::MySql, "DROP TABLE tb_user", false);
        let sleep = analyze_sql_text(DbEngine::MySql, "SELECT SLEEP(30) FROM tb_user", false);
        assert_eq!(write.pattern_risk, RiskLevel::Safe);
        assert_eq!(scan.pattern_risk, RiskLevel::Safe);
        assert_eq!(drop.pattern_risk, RiskLevel::Critical);
        assert_eq!(sleep.pattern_risk, RiskLevel::Critical);
    }

    #[test]
    fn where_only_inside_subquery_is_unconditional_write() {
        let update = analyze_sql_text(
            DbEngine::MySql,
            "UPDATE tb_user SET memo = (SELECT note FROM tb_note WHERE note_id = 1)",
            true,
        );
        assert_eq!(update.pattern_risk, RiskLevel::Critical);
        assert_eq!(update.pattern_note, Some(RiskReason::UnconditionalWrite));

        let joined = analyze_sql_text(
            DbEngine::MySql,
            "DELETE a FROM tb_a a JOIN (SELECT id FROM tb_b WHERE x = 1) b ON a.id = b.id",
            true,
        );
        assert_eq!(joined.pattern_risk, RiskLevel::Critical);
    }

    #[test]
    fn top_level_where_is_found_across_case_lines_and_parens() {
        let cases = [
            ("update tb_user set memo = 'a'\nwhere\nuser_id = 1", RiskLevel::Safe),
            ("UPDATE tb_user SET memo = 'a' WHERE(user_id = 1)", RiskLevel::Safe),
            ("UPDATE tb_user SET memo = (SELECT 1) WHERE user_id = 1", RiskLevel::Safe),
            ("UPDATE tb_user SET memo = 'no WHERE here'", RiskLevel::Critical),
            ("UPDATE tb_user SET memo = 'it''s WHERE' ORDER BY id", RiskLevel::Critical),
            ("UPDATE tb_user SET `where` = 1", RiskLevel::Critical),
            ("UPDATE tb_user SET memo = 1 /* WHERE user_id = 1 */", RiskLevel::Critical),
            ("UPDATE tb_user SET memo = 1 -- WHERE user_id = 1", RiskLevel::Critical),
            ("UPDATE tb_user SET memo = 1 WHERE_FLAG = 1", RiskLevel::Critical),
        ];
        for (sql, expected) in cases {
            let decoded = analyze_sql_text(DbEngine::MySql, sql, true);
            assert_eq!(decoded.pattern_risk, expected, "{sql}");
        }
    }

    #[test]
    fn leading_comment_does_not_change_classification() {
        let hinted = analyze_sql_text(
            DbEngine::MySql,
            "/* mapper: ContConoMapper.updateContCono */ UPDATE tb_user SET memo = 1 WHERE user_id = 1",
            true,
        );
        assert_eq!(hinted.op_kind, SqlOpKind::Update);
        assert_eq!(hinted.target_table, "TB_USER");
        assert_eq!(hinted.pattern_risk, RiskLevel::Safe);

        let lined = analyze_sql_text(DbEngine::MySql, "-- cleanup\nDELETE FROM tb_user", true);
        assert_eq!(lined.op_kind, SqlOpKind::Delete);
        assert_eq!(lined.pattern_risk, RiskLevel::Critical);

        let drop = analyze_sql_text(DbEngine::MySql, "/* x */ DROP TABLE tb_user", true);
        assert_eq!(drop.pattern_risk, RiskLevel::Critical);
    }

    #[test]
    fn insert_on_duplicate_key_update_stays_insert() {
        let upsert = analyze_sql_text(
            DbEngine::MySql,
            "INSERT INTO tb_user (user_id, memo) VALUES (1, 'a') ON DUPLICATE KEY UPDATE memo = 'b'",
            true,
        );
        assert_eq!(upsert.op_kind, SqlOpKind::Insert);
        assert_eq!(upsert.target_table, "TB_USER");
        assert_eq!(upsert.pattern_risk, RiskLevel::Safe);
    }

    #[test]
    fn postgres_message_length_decides_completeness() {
        let delete = build_postgres_query(b'Q', None, "DELETE FROM tb_user", 0);
        let full = decode_wire_sql(5432, &delete).expect("simple query");
        assert_eq!(full.op_kind, SqlOpKind::Delete);
        assert!(full.complete);
        assert_eq!(full.pattern_risk, RiskLevel::Critical);

        let cut = decode_wire_sql(5432, &build_postgres_query(b'Q', None, "DELETE FROM tb_user", 5000))
            .expect("cut query");
        assert!(!cut.complete);
        assert_eq!(cut.pattern_risk, RiskLevel::Safe);

        let parse = build_postgres_query(b'P', Some("s1"), "UPDATE tb_user SET a = 1 WHERE id = $1", 0);
        let parsed = decode_wire_sql(5432, &parse).expect("parse message");
        assert_eq!(parsed.op_kind, SqlOpKind::Update);
        assert_eq!(parsed.target_table, "TB_USER");
        assert!(parsed.complete);
        assert_eq!(parsed.pattern_risk, RiskLevel::Safe);
    }

    #[test]
    fn plain_text_fallback_is_always_incomplete() {
        let decoded = decode_wire_sql(1433, b"....DELETE FROM tb_user").expect("fallback");
        assert_eq!(decoded.op_kind, SqlOpKind::Delete);
        assert!(!decoded.complete);
        assert_eq!(decoded.pattern_risk, RiskLevel::Safe);
    }

    #[test]
    fn packet_decodes_sql_only_toward_db_and_drops_unwatched_traffic() {
        let cfg = MonitorConfig::default();
        let mut flows = FlowReassembler::default();
        let client = [192, 0, 2, 25];
        let db = [192, 0, 2, 10];
        let sql = b"SELECT id FROM tb_order WHERE id = 1";
        let mut query = vec![(sql.len() + 1) as u8, 0, 0, 0, 0x03];
        query.extend_from_slice(sql);

        let outbound = decode_packet(
            &build_tcp_packet(client, 55101, db, 3306, 1, &query),
            12,
            &cfg,
            &mut flows,
        )
        .expect("outbound frame");
        assert_eq!(outbound.decoded_sql.expect("sql").op_kind, SqlOpKind::Select);

        let reply = b"....SELECT text inside result row....";
        let inbound = decode_packet(
            &build_tcp_packet(db, 3306, client, 55101, 1, reply),
            12,
            &cfg,
            &mut flows,
        )
        .expect("inbound frame");
        assert!(inbound.decoded_sql.is_none());
        assert_eq!(inbound.payload_len, reply.len());

        let unwatched = build_tcp_packet(client, 55101, db, 8080, 1, sql);
        assert!(decode_packet(&unwatched, 12, &cfg, &mut flows).is_none());
    }

    #[test]
    fn split_update_is_reassembled_before_judging() {
        let cfg = MonitorConfig::default();
        let mut flows = FlowReassembler::default();
        let client = [192, 0, 2, 25];
        let db = [192, 0, 2, 10];
        let sql = build_wide_update_sql(80, "    WHERE CO_NO = ?");
        let packet = build_mysql_query(&sql, 0);
        let (first, rest) = packet.split_at(1400);
        let (second, third) = rest.split_at(1400);

        let held_first = decode_packet(
            &build_tcp_packet(client, 55101, db, 3306, 1000, first),
            12,
            &cfg,
            &mut flows,
        )
        .expect("first segment");
        let held_second = decode_packet(
            &build_tcp_packet(client, 55101, db, 3306, 2400, second),
            12,
            &cfg,
            &mut flows,
        )
        .expect("second segment");
        assert!(held_first.decoded_sql.is_none() && held_second.decoded_sql.is_none());
        assert_eq!(held_first.payload_len + held_second.payload_len, 2800);

        let last = decode_packet(
            &build_tcp_packet(client, 55101, db, 3306, 3800, third),
            12,
            &cfg,
            &mut flows,
        )
        .expect("last segment");
        let decoded = last.decoded_sql.expect("assembled sql");
        assert_eq!(decoded.op_kind, SqlOpKind::Update);
        assert!(decoded.complete);
        assert_eq!(decoded.pattern_risk, RiskLevel::Safe);
        assert!(decoded.full_sql.ends_with("WHERE CO_NO = ?"));
        assert!(flows.drain_expired_flows(Instant::now() + Duration::from_secs(3)).is_empty());
    }

    #[test]
    fn out_of_order_segment_gives_up_reassembly_without_alert() {
        let cfg = MonitorConfig::default();
        let mut flows = FlowReassembler::default();
        let client = [192, 0, 2, 25];
        let db = [192, 0, 2, 10];
        let sql = build_wide_update_sql(80, "    WHERE CO_NO = ?");
        let packet = build_mysql_query(&sql, 0);
        let (first, rest) = packet.split_at(1400);

        decode_packet(&build_tcp_packet(client, 55101, db, 3306, 1000, first), 12, &cfg, &mut flows)
            .expect("first segment");
        decode_packet(&build_tcp_packet(client, 55101, db, 3306, 9999, rest), 12, &cfg, &mut flows)
            .expect("skipped segment");

        let abandoned = flows.drain_expired_flows(Instant::now());
        assert_eq!(abandoned.len(), 1);
        assert_eq!(abandoned[0].payload_len, 0);
        let decoded = abandoned[0].decoded_sql.as_ref().expect("incomplete sql");
        assert_eq!(decoded.op_kind, SqlOpKind::Update);
        assert!(!decoded.complete);
        assert_eq!(decoded.pattern_risk, RiskLevel::Safe);
        assert!(flows.drain_expired_flows(Instant::now() + Duration::from_secs(3)).is_empty());
    }

    #[test]
    fn redis_value_words_are_not_decoded_as_sql() {
        let set_cmd = b"*3\r\n$3\r\nSET\r\n$7\r\nmenu:01\r\n$27\r\n{\"title\":\"select update\"}\r\n";
        let select_db = b"*2\r\n$6\r\nSELECT\r\n$1\r\n1\r\n";
        let decoded_set = decode_wire_sql(6379, set_cmd).expect("redis set");
        let decoded_select = decode_wire_sql(6379, select_db).expect("redis select");

        assert_eq!(decoded_set.op_kind, SqlOpKind::EncryptedFlow);
        assert_eq!(decoded_set.fingerprint, "REDIS::SET");
        assert_eq!(decoded_select.op_kind, SqlOpKind::EncryptedFlow);
        assert_eq!(decoded_select.fingerprint, "REDIS::SELECT");
    }
}
