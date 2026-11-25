# database-monitor

## 개요

1. **언어**
    - 영어 원문: [README.md](README.md)
    - 문서의 기본 언어는 영어다.
2. **소개**
    - 외부 DB에 쿼리나 연결을 하나도 만들지 않고(zero-query, 패시브) 내 PC에서 나가는 DB 통신만 관찰하는 Windows 터미널(TUI) 모니터
    - 로컬 TCP 테이블, 프로세스 테이블, Npcap 패시브 캡처를 읽어 외부 DB 연결, 트래픽, SQL 페이로드, AI 에이전트의 과실행을 보여 준다.
    - AI 에이전트, MCP 서버, Spring 앱, DB 클라이언트 같은 로컬 행위자의 과조회, 반복 루프, 커넥션 폭주, 위험 SQL을 실시간 판정하고 한 번의 확인 뒤 중단한다.
3. **특징**
    - DB 부하 0: `SHOW PROCESSLIST` 같은 쿼리를 하지 않고, 매 tick 수집에 외부 프로세스(`netstat`, `tasklist`, PowerShell)를 띄우지 않는다.
    - 로컬 행위자를 `[AGENT]`, `[SPRING]`, `[CLIENT]`, `[SCRIPT]`로 표시하며, 이 표시는 위험 등급에 영향을 주지 않는다.
    - 10초 평균으로 판정하는 위험 4단계라 짧은 버스트나 스케줄러 폴링이 경보로 이어지지 않는다.
    - `CRITICAL`이면 창이 최상단으로 뜨고 점멸하며, 키 하나로 위험 로컬 프로세스를 중단하는 확인 창을 연다.
    - 스크립트용 일회성 `--once`, `--json` 스냅샷
4. **화면 언어**
    - TUI는 영어로 시작하며 `L` 키(하단 칩 `[L] EN` 또는 `[L] KO`)로 재시작 없이 영어와 한국어를 전환한다.
    - 고른 언어는 `~/.config/database-monitor/tui.toml`에 저장되고 다음 실행에서 복원된다.
    - 데이터 값은 번역하지 않는다. 프로세스 이름, 엔드포인트, SQL 원문, 경로, 프로세스 종료 실패의 시스템 문구가 그렇다.
    - 위험 사유, 경보 상세, 진단 요약, 중단 결과는 값으로 전달되어 화면 언어로 만들어지므로 `L` 키로 함께 바뀐다.
    - `--once` 텍스트 스냅샷, 로그 줄, stderr 안내는 항상 영어이며 CLI에는 언어 전환이 없다.
    - `--json` 필드 이름은 그대로이고 사유 필드는 한국어가 아니라 카탈로그의 영어 문구를 담는다.
5. **문서**
    - 소스 구조와 내부 흐름: [ARCHITECTURE-ko.md](ARCHITECTURE-ko.md) (영어: [ARCHITECTURE.md](ARCHITECTURE.md))
    - 라이선스: [LICENSE.md](LICENSE.md) (Apache-2.0)
6. **배지**
    - [![Crates.io](https://img.shields.io/crates/v/database-monitor.svg)](https://crates.io/crates/database-monitor) [![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE.md)

---

## 빠른 시작

### 1. 요구 사항

1. **실행**
    - Windows. 커맨드라인 조회 API(`ProcessCommandLineInformation`)는 Windows 8.1 이상이 필요하며, 실행 검증은 Windows 11에서만 했다.
    - 와이어 캡처(SQL 원문, 응답 크기)용 x64 [Npcap](https://npcap.com/#download)
    - 동작하는 Npcap이 없으면 `Passive Socket-Only` 모드로 폴백해 연결과 프로세스 정보만 보여 준다.
2. **빌드**
    - Rust 1.88 이상 (`Cargo.toml`의 `rust-version`, edition 2024)
    - [Npcap SDK](https://npcap.com/#download): `pcap` crate가 `wpcap`을 링크하므로 SDK의 `Lib/x64` 폴더를 `LIB` 환경 변수에 넣어야 한다.
3. **관리자 권한**
    - 실행 파일은 권한 상승을 요청하지 않으며(`asInvoker` manifest) 일반 사용자로 시작한다.
    - 관리자 권한으로 뜬 프로세스를 종료하려면 모니터도 관리자 권한이어야 한다. 권한이 부족하면 종료하지 않고 권한 부족 메시지를 보여 준다.
    - 와이어 캡처를 시작하지 못하면 원인은 표시되지 않고 모드가 조용히 `Passive Socket-Only`가 된다.

### 2. 설치

1. **crates.io**
    - 먼저 `LIB`를 Npcap SDK의 `Lib/x64` 폴더로 지정한 뒤 설치한다.

```powershell
$env:LIB = "<Npcap SDK folder>\Lib\x64;$env:LIB"
cargo install database-monitor
```

2. **소스에서 빌드**
    - 저장소를 받고 같은 방식으로 `LIB`를 지정한 뒤 기본 cargo 명령으로 빌드한다.
    - 실행 파일은 `target\release\database-monitor.exe`다.

```powershell
cargo build --release
```

### 3. 설정

1. **설정 파일 만들기**
    - 저장소의 `docs/config.example.toml`을 `~/.config/database-monitor/config.toml`로 복사하고 실제 DB 주소를 넣는다. Windows에서는 `%USERPROFILE%\.config\database-monitor\config.toml`이다.
    - 설정 파일이 없어도 실행되며, 이때는 기본 감시 포트만 본다.
    - 이 파일에는 실제 호스트가 들어 있으므로 비공개로 둔다. 저장소 밖 홈 폴더에 있어 커밋되지 않는다.
2. **탐색 위치**
    - `--config <path>`를 주면 그 파일
    - 없으면 `~/.config/database-monitor/config.toml`. `~`는 `USERPROFILE`이며, 없으면 `HOME`이다.
    - 다른 폴더는 찾지 않는다.
    - 형식이 잘못된 파일이나 읽을 수 없는 `--config` 경로는 오류를 내고 실행을 멈추며, 기본 파일이 없는 것은 오류가 아니다.
3. **최소 예시**
    - `192.0.2.0/24` 주소는 문서용 가상 값이다(RFC 5737).

```toml
poll_interval_ms = 1000
enable_pcap = true
ports = [3307]

[hosts]
"192.0.2.10" = "dev DB"

[thresholds]
warn_repeat_10s = 50
```

```markdown
ports 는 기본 감시 포트 3306, 33060, 5002, 6033, 5432, 1433, 1521, 6379, 27017 에 추가됨
hosts: 등록한 호스트는 80/443 을 뺀 모든 포트를 DB 연결로 보고, 패킷 캡처 필터에도 포함됨
thresholds: 생략한 항목은 기본값 유지
```

### 4. 첫 실행

1. **TUI 시작**
    - 터미널에서 실행하며, 하단 알림이 로드된 설정 파일 경로를 4초 동안 보여 준다.
    - Windows Terminal에서 프로젝트 글꼴을 쓰려면 프로필의 `font.face`를 `D2Coding ligature`로 지정한다. 앱이 이 글꼴을 직접 바꿀 수 없다.

```powershell
database-monitor
```

2. **화면 읽기**
    - 위에서 아래로 앱 바, 탭 바, 지표 카드 4개(위험도, DB 연결, 수신 트래픽, 60초 추이를 포함한 쿼리율), `CRITICAL` 동안의 경보 카드, 콘텐츠 카드, 하단 영역(액션 칩과 안내 줄)이다.
    - `q`, `Esc`, `Ctrl+C`로 종료하고 `?`로 도움말 겹침을 연다.
3. **캡처 상태 확인**
    - 버려진 프레임이 있으면 앱 바에 `캡처 누락 N`(영어 `Capture drops N`)이 표시되며, 이때 RX와 QPS는 실제보다 낮게 보일 수 있다.
    - `Passive Socket-Only` 모드에는 SQL 원문과 응답 크기가 없다.

### 5. 스냅샷 모드

1. **일회성 스냅샷**
    - `--once`는 속도 지표를 위해 폴링 주기만큼 간격을 두고 두 번 샘플링한 뒤 출력하고 종료한다.
    - `--json`은 스냅샷을 JSON으로 출력하며 `--once`를 포함한다.
    - 두 출력 모두 영어이며, `--json`에서 `risk_reasons`, `risk_summary`, `alerts[].detail`, `alerts[].recent_sql`, `risk_reason`, `risk_note` 필드는 영어 문구를 담고 사유가 없으면 빈 문자열이다.
    - `--no-pcap`은 패킷 캡처를 건너뛰므로 Npcap이 없는 PC에 알맞다.

```powershell
database-monitor --once --no-pcap
database-monitor --json --no-pcap --interval-ms 500
```

---

## 기능

1. **DB 부하 0**
    - Win32 TCP 테이블(`GetExtendedTcpTable`, IPv4·IPv6), 프로세스 테이블(Toolhelp32), 커맨드라인(`NtQueryInformationProcess`)을 직접 읽는다.
    - Npcap 캡처는 패시브(`promisc=false`)이며 MySQL·PostgreSQL 와이어에서 SQL 원문과 응답 크기를 본다.
    - 모니터가 띄우는 외부 프로세스는 사용자가 중단을 확정한 뒤의 `taskkill`뿐이다.
2. **로컬 행위자 표시**
    - `[AGENT]`: AI 에이전트·MCP 서버(`mysql-mcp`, `claude`, `codex`, `antigravity`, `.gemini`)와, 상위 프로세스 체인에 `claude`·`codex`·`antigravity`가 있는 하위 프로세스(에이전트가 띄운 `mysql.exe`, `python.exe`, `java.exe` 포함)
    - `[SPRING]`: JVM·Spring 앱(`java.exe`, `gradle`, `bootRun`)
    - `[CLIENT]`: DB 클라이언트·덤프 도구(`mysql.exe`, `mysqldump.exe`, `dbeaver.exe`, `datagrip`)
    - `[SCRIPT]`: 스크립트 워커(`python.exe`, `node.exe`, `pwsh.exe`)
3. **로컬 통신 제외**
    - 루프백(`127.0.0.1`, `::1`)이나 같은 소켓의 로컬 IP로 가는 DB 연결은 `[LOCAL]` / `제외`(영어 `Excluded`)로만 표시한다.
    - 로컬 통신은 위험 점수, 경보, 팝업, 일괄 중단, 트래픽·쿼리 합계, SQL 스트림에서 모두 빠진다.
4. **위험 시 팝업·점멸**
    - `CRITICAL`이면 터미널을 복원하고 최상단에 고정한다(`SetWindowPos(HWND_TOPMOST)`, `SetForegroundWindow`, `FlashWindowEx`).
    - 위험이 이어지면 6초마다 다시 띄우고, 경보 카드와 테두리가 적색·황색으로 점멸한다.
    - `b` 키나 하단 `[b] 점멸` 칩으로 점멸을 끄면 화면 점멸과 작업표시줄 점멸이 멈추고 경보 색은 고정되며, 최상단 팝업은 유지된다.
5. **확인 뒤 일괄 중단 (서킷브레이커)**
    - `x`(`F9`, `F12` 포함), 경보 배너 클릭, 하단 `[x] 위험 중단` 칩은 확인 겹침을 연다. 확정하기 전에는 아무것도 종료하지 않는다.
    - 겹침에는 중단될 `CRITICAL` 로컬 프로세스의 개수와 이름이 보이며, `Y` 키나 채워진 `Y` 버튼 클릭은 그 프로세스만 `taskkill /PID <pid> /T /F`로 종료한다.
    - 그 외 키, `Esc`, `N / Esc` 버튼은 취소한다.
    - `CRITICAL` 프로세스가 없으면 겹침을 열지 않고 안내만 보여 준다.
    - 시스템·터미널 프로세스, 모니터 자신, 모니터의 상위 프로세스(셸, 터미널, 에이전트 호스트)는 보호된다.
    - 직전 요청 후 2초 안의 중복 요청은 무시한다.
    - `K`(Shift+k)는 같은 형태의 확인 겹침을 거쳐 선택한 프로세스를 종료한다.
    - 최소 터미널 크기(60x16) 미만에서는 중단·종료 키가 동작하지 않으며, 그 아래로 줄어들면 열려 있던 확인이 취소된다.
6. **캡처 누락 표시**
    - 캡처 채널은 16384 프레임을 담으며, 가득 차면 프레임을 버리고 개수를 세어 앱 바에 `캡처 누락 N`(영어 `Capture drops N`)을 표시한다.

---

## 위험 등급

1. **등급**
    - `SAFE`(`정상` / `Safe`), `CAUTION`(`관찰` / `Watch`, 표 색만), `WARNING`(`주의` / `Warning`, 경보 이력만), `CRITICAL`(`위험` / `Critical`)
    - 경보 카드, 점멸, 최상단 팝업, 일괄 중단은 `CRITICAL`에서만 동작한다.
2. **CRITICAL**
    - 10초 평균 수신 4 MB/s 이상
    - 평균 1 MB/s 이상을 끌어오는 반복·고빈도 루프(평균 QPS 100 이상, 또는 동일 SQL 10초 100회)
    - 10초 평균 300 QPS 이상
    - `WHERE` 없는 `UPDATE`/`DELETE`, `DROP`, `TRUNCATE`, `SLEEP()`, `BENCHMARK()`
3. **WARNING**
    - 10초 평균 수신 1 MB/s 이상 또는 100 QPS 이상
    - 큰 트래픽 없이 동일 SQL 10초 100회
    - `ESTABLISHED` 70개, 또는 단기 연결(`TIME_WAIT`+`SYN`) 120개
    - `ALTER`
4. **CAUTION**
    - 평균 수신 256 KB/s, 30 QPS, 동일 SQL 10초 30회
    - 활동 중 `ESTABLISHED` 35개, 단기 연결 30개
    - 조건·`LIMIT` 없는 전체 조회
5. **위험으로 보지 않는 것**
    - 조건 있는 `INSERT`/`UPDATE`/`DELETE`와 `WHERE` 있는 `SELECT *`는 앱 정상 동작으로 보고 `SAFE`다.
    - `WHERE`·`LIMIT` 없음은 완전한 문장만 판정한다. 캡처가 끊긴 문장은 끝에 ` ...`을 붙여 보여 주고 해당 경보를 내지 않으며, 여러 패킷에 나뉜 긴 문장은 먼저 재조립한다.
    - TLS·바이너리 패킷과 Redis 명령은 쿼리 수에 넣지 않고 트래픽으로만 계산한다.
    - 드라이버·커넥션 풀 관리 명령(`COM_PING`, `SET autocommit`, `COMMIT`, `SELECT 1`, `SELECT @@...`)은 쿼리 수·반복에서 뺀다.
6. **경보 로그**
    - 경보 로그 줄에 최근 10초 최다 반복 SQL 핑거프린트 3개를 남긴다.
    - 같은 경보는 등급이 오르지 않으면 60초에 한 번만 기록한다.
    - 상세 규칙과 전체 임계치: [ARCHITECTURE-ko.md](ARCHITECTURE-ko.md) 5절

---

## 설정

1. **우선순위**
    - 기본값, `config.toml`, 명령행 옵션 순으로 덮어쓴다.
2. **`config.toml` 키**
    - `poll_interval_ms`: 폴링 주기, 250 ~ 10000으로 고정
    - `enable_pcap`: `false`면 소켓 테이블만 사용
    - `interface`: 캡처 인터페이스 이름·설명 부분 문자열. 없으면 주소가 있는 모든 인터페이스를 쓴다.
    - `ports`: 추가 감시 포트
    - `hosts`: `"IP" = "별칭"` 표. IP가 아닌 키는 실행을 멈춘다.
    - `console_font`: 클래식 콘솔(conhost) 글꼴, 기본 `D2Coding ligature`. 빈 문자열이면 글꼴을 바꾸지 않고, Windows Terminal에서는 적용되지 않는다.
    - `[thresholds]`: 위험 임계치. 알 수 없는 키는 실행을 멈춘다. 모든 키와 기본값은 `docs/config.example.toml`에 있다.
3. **로그 파일**
    - 로그는 `--config`로 다른 폴더를 가리켜도 항상 `~/.config/database-monitor/logs/database-monitor-YYYYMMDD.log`에 쓴다. `~`는 `USERPROFILE`이며, 없으면 `HOME`이다.
    - 홈 폴더가 없으면 로그를 끄고 stderr에 한 줄로 알리며, 현재 폴더로 대신 쓰지 않는다.
    - 경보 줄에 호스트 별칭과 실제 IP가 들어가므로 로그 파일은 공유하지 않는다.

---

## 명령행 옵션

1. **옵션**
    - `--config <path>`: 설정 파일 경로
    - `--interval-ms <ms>`: 폴링 주기, 250 ~ 10000, 기본 1000
    - `--no-pcap`: 패킷 캡처를 끄고 소켓 테이블만 사용
    - `--interface <text>`: 캡처 인터페이스 이름·설명 부분 문자열
    - `--ports 3307,15432`: 추가 감시 포트
    - `--host IP=ALIAS`: 추가 호스트 별칭, 반복 가능. IP가 아니면 실행을 멈추고, 별칭을 생략하면 `CUSTOM-DB`가 된다.
    - `--once`: 비대화형 스냅샷을 한 번 만들고 종료
    - `--json`: 스냅샷을 JSON으로 출력, `--once` 포함
    - `--help`, `--version`은 clap이 제공한다.

```powershell
database-monitor --host 192.0.2.10=DEV-DB --ports 3307
```

---

## TUI 조작

1. **키**
    - `Tab` / `Shift+Tab` / `1` ~ `4`: 탭 전환(`1 통합관제`, `2 에이전트·프로세스`, `3 과조회SQL`, `4 위험경보`), `1` ~ `4`는 탭 바에 보이는 순서의 번호
    - `<` / `>`: 현재 탭을 왼쪽·오른쪽으로 한 칸 이동(순서 저장)
    - `↑` / `↓` / `k` / `j` / `PgUp` / `PgDn` / `Home` / `End`: 선택 행 이동 또는 포커스 카드 스크롤, `k`는 위로 `j`는 아래로 화살표와 같이 이동하고 `PgUp`과 `PgDn`은 한 화면씩 이동
    - `s` / `S`: 포커스 표의 정렬 순환, 머리글 클릭과 같은 순환
    - `r`: 지금 수집, `Space` / `p`: 화면 갱신 일시정지·재개
    - `+` / `-`(`=`, `_` 포함): 폴링 주기를 250ms 단위로 조절, 250 ~ 5000ms
    - `x` / `F9` / `F12`: `CRITICAL` 로컬 프로세스 모두 중단의 확인 겹침, `Y`로 확정
    - `K`(Shift+k): 1, 2번 탭에서 선택한 프로세스의 종료 확인 겹침, `Y`로 확정
    - `t`: 창 항상 위 고정 토글(위험 시 자동 활성화), `b`: 경보 점멸 토글(기본 ON)
    - `Shift+←` / `Shift+→` / `Shift+↑` / `Shift+↓`: 포커스 카드를 한 칸 이동(가로로 나란한 카드는 `←` `→`, 세로는 `↑` `↓`)
    - `l`: 현재 탭의 카드 순서, 카드 크기, 열 폭을 기본 배치로 복원하며 언어는 유지
    - `L`: 표시 언어를 영어와 한국어로 전환
    - `?`: 도움말 겹침, 아무 키나 클릭으로 닫는다.
    - `!`: 강제 재시작, 실행 중인 exe 가 디스크에서 새 빌드로 바뀐 뒤에만 동작하며, 터미널을 복원하고 같은 인자로 TUI를 최신 exe에서 다시 실행한다.
    - `q` / `Esc` / `Ctrl+C`: 종료
2. **마우스**
    - 탭, 하단 칩, 카드(포커스, 굵은 테두리), 행(선택)을 클릭한다.
    - 열 머리글을 클릭하면 정렬하고, 머리글 경계를 끌면 열 폭을 바꾸며, 경계를 더블클릭하면 자동 폭이 된다.
    - 열 경계는 평소 옅은 `│`로 보이고 양옆 머리글 한 칸까지 잡히며, 호버·드래그 중에는 표 전체 높이의 `┃`가 된다. 경계 칸 더블클릭은 자동 폭을 복원하고, 옆 머리글 칸 클릭은 정렬한다.
    - 탭을 끌어 탭 순서를 바꾸며, `Esc`는 취소한다.
    - 카드 경계를 끌어 카드 크기를 바꾼다. 크기는 비율로 기억한다.
    - 카드 제목을 400ms 누른 채 끌면 미리보기(점선 테두리)와 함께 카드가 이동하고, 놓으면 확정되며 `Esc`는 취소한다.
    - 창 밖에서 놓은 버튼은 다음 마우스 이동에서 알아차려, 카드 이동·카드 경계 드래그·열 폭 드래그·스크롤바 드래그를 놓은 것으로 끝내고, 이미 움직인 드래그는 저장한다.
    - 휠은 포인터 아래 카드를 스크롤하고, 스크롤바 썸을 끌거나 트랙을 클릭하면 한 화면씩 이동한다.
3. **정렬**
    - 숫자·시간 열의 첫 클릭은 내림차순(큰 값·최근 값 먼저)이고, 문자 열은 오름차순이 먼저다.
    - 두 번째 클릭은 방향을 바꾸고 세 번째 클릭은 기본 순위로 되돌린다.
    - 활성 정렬은 카드 제목에 `· 정렬 실행횟수↓`처럼 표시된다.
    - 정렬은 열 번호와 값 기준이라 언어를 바꿔도 행 순서와 선택이 바뀌지 않는다.
4. **표**
    - 열은 자연 폭을 유지하고, 글자 열이 먼저 줄어들며(`…`) 맨 오른쪽 열이 마지막에 숨는다.
    - 위험 등급 열은 고정되어 다른 열이 모두 숨은 뒤에야 숨으므로 100x30과 60x16에서도 보인다.
    - 스크롤바(트랙 `░`, 썸 `█`)는 카드 테두리 안쪽의 별도 회색 거터에 있으며 포커스 색을 쓰지 않는다.
    - 스크롤되는 카드는 하단 테두리에 ` a-b / total `을 표시한다.
5. **하단 영역**
    - 모든 칩은 클릭할 수 있고 키와 같은 동작을 한다. 폭이 모자라면 왼쪽 칩이 오른쪽부터 사라지고 `[L]` 언어 칩이 마지막까지 남으며, 도움말·종료 칩은 오른쪽 끝에 고정된다.
    - 실행 중인 exe 가 디스크에서 새 빌드로 바뀌면(2초마다 확인) 경고색 `[!] 강제 재시작` 칩이 하단 맨 앞에 나타나 최소 크기에서도 남고, 클릭하면 `!`와 같게 동작한다.
    - 안내 줄은 알림을 먼저 보여 주고(4초, 오류 알림만 적색 채움), 이어서 포인터, 포커스 카드, 탭에 맞는 키를 보여 준다.
6. **최소 크기**
    - 최소 크기는 60x16이며, 그보다 작으면 `터미널 크기 부족 (최소 60x16, 현재 WxH)` 한 줄만 그린다.

---

## 상태 파일

1. **위치**
    - `config.toml` 옆의 `~/.config/database-monitor/tui.toml`. `~`는 `USERPROFILE`이며, 없으면 `HOME`이다.
    - 파일이 심볼릭 링크면 링크는 그대로 두고 가리키는 파일을 갱신한다. 링크 사슬은 최대 16단계까지 따라가며, 더 긴 사슬이나 순환은 저장 오류로 알린다.
2. **내용**
    - `version`, `language`, 카드 순서, 경계 비율, 열 폭, 탭 순서를 담으며 기본값과 다른 값만 쓴다.
    - 손으로 고친 잘못된 항목은 건너뛰고 나머지는 그대로 적용하며, 열 폭은 512칸으로 제한한다.
    - 탭 순서는 기본 순서가 아닐 때만 `tab_order = ["processes", "overview", "top_sql", "alerts"]` 형식으로 쓰고, 네 탭의 순열이 아닌 값은 건너뛰며, `l` 복원은 탭 순서를 바꾸지 않는다.
    - 정렬과 선택 행은 저장하지 않는다.
3. **저장 시점**
    - 확정된 변경에만 쓴다. 카드 이동, 카드 경계 드래그, 열 폭 드래그, 탭 이동(드래그 또는 `<` / `>`)이 끝났을 때, 언어 전환, `l` 복원이 그렇다.
    - 그리기, 마우스 이동, 스크롤, 정렬, 선택은 저장하지 않는다.
    - 쓰기는 저장 스레드가 하므로 입력 루프는 디스크를 기다리지 않는다. 요청은 합쳐져 파일마다 최신 내용만 쓰이고, 쓰기가 실패하면 오류 알림을 보여 주며, 종료 때 마지막 요청은 5초 안에 쓰고 디스크가 더 오래 멈추면 stderr에 경고를 내본다.
    - 대상 파일 옆의 임시 파일에 쓰고 디스크에 동기화한 뒤 이름을 바꿔 교체한다.
    - 시작할 때는 오래된(60초 동안 건드리지 않은) 임시 파일만 지우므로 다른 실행 중인 인스턴스의 파일은 남는다.
    - 읽을 수 없거나 버전이 다른 파일은 첫 덮어쓰기 전에 `tui.toml.bak`으로 복사하고 알림을 한 번 보여 준다.
4. **예시**

```toml
version = 1
language = "ko"

[orders]
metric_cols = [3, 0, 1, 2]

[ratios]
top_sql_rows = [6000]

[column_widths]
processes = [0, 14, 0, 9, 0, 0, 0, 0, 0]
```

```markdown
orders: 그룹의 카드 순서, 칸 순서대로 각 칸에 놓인 카드 번호
ratios: 그룹 길이에 대한 경계 위치, 만분율
column_widths: 열별 폭(칸 수), 0은 자동 폭
```

---

## 문제 해결

1. **exe가 `0xc000007b`로 시작되지 않음**
    - `wpcap.dll`이 없거나 32비트인 경우가 흔한 원인이며, 비트 수 불일치 오류다.
    - [npcap.com](https://npcap.com/#download)에서 x64 Npcap 런타임을 설치한 뒤 다시 실행한다.
2. **빌드에서 `wpcap` 링크 실패**
    - Npcap SDK가 링커 경로에 없다.
    - `LIB` 환경 변수에 SDK의 `Lib/x64` 폴더(x86용 `Lib`가 아님)를 지정하고 다시 빌드한다.
3. **모드가 `Passive Socket-Only`로 표시됨**
    - 캡처를 시작하지 못했고 원인은 보고되지 않는다. Npcap이 없거나, 사용할 인터페이스가 없거나, 장치를 열지 못한 경우가 가능한 원인이다.
    - x64 Npcap을 설치 또는 복구하고 관리자 권한 터미널에서 시도한다.
    - 이 모드에는 SQL 원문과 응답 크기가 없다.
4. **설정 파일이 로드되지 않음**
    - 시작 알림이 파일을 찾지 못했다고 알려 주며 기본 감시 포트만 본다.
    - `config.toml`을 `~/.config/database-monitor/`에 두거나 `--config <path>`를 지정한다.
5. **중단 시 권한 부족 메시지**
    - 대상 프로세스가 관리자 권한으로 실행 중이다.
    - 모니터를 관리자 권한 터미널에서 실행하거나 해당 앱에서 직접 중지한다.
6. **테두리선이나 글자가 깨짐**
    - `D2Coding ligature`처럼 선 문자와 한글을 지원하는 글꼴을 쓴다.

---

## 개발

1. **검증**
    - `cargo test`: `src/` 단위 테스트, 엔진·위험도 테스트(`tests/monitor.rs`), 화면 테스트(`tests/tui.rs`)
    - `tests/tui.rs`는 모든 탭을 영어·한국어로 60x16, 100x30, 130x40에서 그려 셀, 클릭 영역, 시계를 주입한 제스처를 검사한다.
2. **코드 규칙**
    - 소스의 번호 구분선 주석(`// N. 제목 ---`)은 80열에 맞추며 `tests/monitor.rs`가 검사한다.
    - 소스·테스트는 `192.0.2.0/24` 가상 주소만 쓴다.
    - `src/` 루트에는 `main.rs`와 `lib.rs`만 두고, 모듈은 `app/`, `analysis/`, `collector/`, `platform/`, `tui/`에 둔다([ARCHITECTURE-ko.md](ARCHITECTURE-ko.md) 2절).
    - Cargo 의존성은 모두 `Cargo.toml`에 전체 `x.y.z` 버전을 적고(ratatui 0.30.2, crossterm 0.29.0), `Cargo.lock`이 해석된 집합을 기록한다.

---

## 라이선스

1. **라이선스**
    - Apache-2.0, [LICENSE.md](LICENSE.md) 참고
