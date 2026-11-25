# database-monitor

## Overview

1. **Languages**
    - Korean version: [README-ko.md](README-ko.md)
    - English is the default language of these documents.
2. **What it is**
    - A Windows terminal (TUI) monitor that watches the database traffic leaving your PC without sending a single query or opening a single connection to any database (zero-query, passive).
    - It reads the local TCP table, the process table, and an Npcap passive capture to show outbound DB connections, traffic, SQL payloads, and over-execution by AI agents.
    - It rates local actors such as AI agents, MCP servers, Spring apps, and DB clients in real time for over-querying, repeat loops, connection floods, and dangerous SQL, and can stop them after one confirmation.
3. **Highlights**
    - No database load: no `SHOW PROCESSLIST` or any other query; the per-tick collection starts no external process (`netstat`, `tasklist`, PowerShell).
    - Local actors are tagged `[AGENT]`, `[SPRING]`, `[CLIENT]`, or `[SCRIPT]`; the tag never changes the risk level.
    - Four risk levels judged on 10-second averages, so a short burst or a scheduler poll does not raise an alarm.
    - On `CRITICAL`, the window pops to the top and blinks, and one key opens a confirmation to stop the critical local processes.
    - One-shot `--once` and `--json` snapshots for scripts.
4. **Display language**
    - The TUI starts in English and switches between English and Korean with `L` (footer chip `[L] EN` or `[L] KO`), without a restart.
    - The chosen language is saved in `~/.config/database-monitor/tui.toml` and restored on the next start.
    - Data values are never translated: process names, endpoints, SQL text, paths, and the system text of a failed process stop.
    - Risk reasons, alert details, the diagnosis summary, and stop results are typed values rendered in the active language, so they switch with `L`.
    - The `--once` text snapshot, log lines, and stderr notices are always English; the CLI has no language switch.
    - The `--json` field names are unchanged; the reason fields carry the English catalog text, not Korean text.
5. **Documents**
    - Source structure and internal flow: [ARCHITECTURE.md](ARCHITECTURE.md) (Korean: [ARCHITECTURE-ko.md](ARCHITECTURE-ko.md))
    - License: [LICENSE.md](LICENSE.md) (Apache-2.0)
6. **Badges**
    - [![Crates.io](https://img.shields.io/crates/v/database-monitor.svg)](https://crates.io/crates/database-monitor) [![License: Apache-2.0](https://img.shields.io/badge/License-Apache--2.0-blue.svg)](LICENSE.md)

---

## Quick Start

### 1. Requirements

1. **To run**
    - Windows; the command-line query API (`ProcessCommandLineInformation`) needs Windows 8.1 or later, and runtime checks were done on Windows 11 only.
    - [Npcap](https://npcap.com/#download) for x64, for wire capture (SQL text and response sizes).
    - Without a working Npcap, the monitor falls back to `Passive Socket-Only` mode and shows connections and process data only.
2. **To build**
    - Rust 1.88 or later (`rust-version` in `Cargo.toml`, edition 2024).
    - The [Npcap SDK](https://npcap.com/#download): the `pcap` crate links `wpcap`, so the SDK `Lib/x64` folder must be on the `LIB` environment variable.
3. **Administrator rights**
    - The executable does not request elevation (`asInvoker` manifest) and starts as a normal user.
    - Stopping a process that runs elevated needs an elevated monitor; the monitor reports the missing permission instead of killing it.
    - If wire capture cannot start, the cause is not shown and the mode silently becomes `Passive Socket-Only`.

### 2. Install

1. **From crates.io**
    - Set `LIB` to the Npcap SDK `Lib/x64` folder first, then install.

```powershell
$env:LIB = "<Npcap SDK folder>\Lib\x64;$env:LIB"
cargo install database-monitor
```

2. **From source**
    - Clone the repository, set `LIB` the same way, and build with plain cargo.
    - The binary is `target\release\database-monitor.exe`.

```powershell
cargo build --release
```

### 3. Configure

1. **Create the config file**
    - Copy `docs/config.example.toml` from the repository to `~/.config/database-monitor/config.toml` and put your real DB addresses in it; on Windows the path is `%USERPROFILE%\.config\database-monitor\config.toml`.
    - The monitor also runs without a config file; it then watches only the default ports.
    - Keep the file private: it holds your real hosts. It lives in your home folder, outside the repository, so it is never committed.
2. **Where it is searched**
    - The `--config <path>` file when given.
    - Otherwise `~/.config/database-monitor/config.toml`, where `~` is `USERPROFILE`, or `HOME` when that is unset.
    - No other folder is searched.
    - A malformed file or an unreadable `--config` path stops startup with an error; a missing default file is not an error.
3. **Minimal example**
    - Addresses in `192.0.2.0/24` are documentation-only values (RFC 5737).

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
ports adds to the default watched ports 3306, 33060, 5002, 6033, 5432, 1433, 1521, 6379, 27017
hosts: every port except 80 and 443 of a listed host counts as a DB connection and enters the capture filter
thresholds: omitted keys keep their defaults
```

### 4. First run

1. **Start the TUI**
    - Run it from a terminal; a notice at the bottom shows the loaded config file path for 4 seconds.
    - In Windows Terminal, set the profile `font.face` to `D2Coding ligature` if you want the project font; the app cannot change that font itself.

```powershell
database-monitor
```

2. **Read the screen**
    - From top to bottom: app bar, tab bar, four metric cards (risk, DB connections, inbound traffic, query rate with a 60-second trend), an alert card while `CRITICAL`, the content cards, and the footer (action chips and a hint row).
    - `q`, `Esc`, or `Ctrl+C` quits; `?` opens the help overlay.
3. **Check capture**
    - The app bar shows `Capture drops N` (`캡처 누락 N` in Korean) when frames were dropped; RX and QPS may then read lower than the real values.
    - `Passive Socket-Only` mode has no SQL text and no response size.

### 5. Snapshot mode

1. **Single snapshot**
    - `--once` samples twice, one polling interval apart, to compute rates, then prints and exits.
    - `--json` prints the snapshot as JSON and implies `--once`.
    - Both outputs are English; in `--json`, `risk_reasons`, `risk_summary`, `alerts[].detail`, `alerts[].recent_sql`, and the `risk_reason` and `risk_note` fields hold English text, and an empty reason stays an empty string.
    - `--no-pcap` skips packet capture, which suits a machine without Npcap.

```powershell
database-monitor --once --no-pcap
database-monitor --json --no-pcap --interval-ms 500
```

---

## Features

1. **Zero load on the database**
    - The collector reads the Win32 TCP table (`GetExtendedTcpTable`, IPv4 and IPv6), the process table (Toolhelp32), and process command lines (`NtQueryInformationProcess`).
    - The Npcap capture is passive (`promisc=false`) and reads MySQL and PostgreSQL wire traffic for SQL text and response sizes.
    - The only external process the monitor starts is `taskkill`, and only after you confirm a stop.
2. **Local actor tags**
    - `[AGENT]`: AI agents and MCP servers (`mysql-mcp`, `claude`, `codex`, `antigravity`, `.gemini`), and any child process whose ancestor chain contains `claude`, `codex`, or `antigravity` (for example `mysql.exe`, `python.exe`, `java.exe` started by an agent).
    - `[SPRING]`: JVM and Spring apps (`java.exe`, `gradle`, `bootRun`).
    - `[CLIENT]`: DB clients and dump tools (`mysql.exe`, `mysqldump.exe`, `dbeaver.exe`, `datagrip`).
    - `[SCRIPT]`: script workers (`python.exe`, `node.exe`, `pwsh.exe`).
3. **Local traffic is excluded**
    - A DB connection to loopback (`127.0.0.1`, `::1`) or to the same socket's local IP is shown as `[LOCAL]` / `Excluded` (`제외` in Korean).
    - Local traffic is left out of risk scores, alerts, popups, bulk stop, traffic and query totals, and the SQL stream.
4. **Pop-up and blink on danger**
    - On `CRITICAL`, the terminal is restored and pinned on top (`SetWindowPos(HWND_TOPMOST)`, `SetForegroundWindow`, `FlashWindowEx`).
    - While the danger lasts, the window pops again every 6 seconds and the alert card and border blink red and amber.
    - `b` or the `[b] Blink` chip turns blinking off: the screen and taskbar blink stop and the alert colors stay fixed; the top-most popup remains.
5. **Confirmed bulk stop (circuit breaker)**
    - `x` (also `F9`, `F12`), a click on the alert banner, or the `[x] Stop risky` chip opens a confirmation overlay; nothing is killed until you confirm.
    - The overlay shows the number and names of the `CRITICAL` local processes that would be stopped; `Y` or a click on the filled `Y` button ends only those with `taskkill /PID <pid> /T /F`.
    - Any other key, `Esc`, or the `N / Esc` button cancels.
    - When no process is `CRITICAL`, no overlay opens and a notice says so.
    - System and terminal processes, the monitor itself, and the monitor's ancestors (shell, terminal, agent host) are protected.
    - A repeat request within 2 seconds of the last one is ignored.
    - `K` (Shift+k) stops the selected process after the same kind of confirmation overlay.
    - Below the minimum terminal size (60x16) no stop or kill key acts, and shrinking the terminal below it cancels an open confirmation.
6. **Dropped-frame notice**
    - The capture channel holds 16384 frames; when it is full, frames are dropped and counted, and the app bar shows `Capture drops N`.

---

## Risk Levels

1. **Levels**
    - `SAFE` (`Safe` / `정상`), `CAUTION` (`Watch` / `관찰`, table color only), `WARNING` (`Warning` / `주의`, alert history only), `CRITICAL` (`Critical` / `위험`).
    - Only `CRITICAL` triggers the alert card, blink, top-most popup, and bulk stop.
2. **CRITICAL**
    - 10-second average inbound traffic of 4 MB/s or more.
    - A repeat or high-frequency loop that pulls an average of 1 MB/s or more (average QPS of 100 or more, or the same SQL 100 times in 10 seconds).
    - 10-second average of 300 QPS or more.
    - `UPDATE` or `DELETE` without `WHERE`, `DROP`, `TRUNCATE`, `SLEEP()`, `BENCHMARK()`.
3. **WARNING**
    - 10-second average inbound traffic of 1 MB/s or more, or 100 QPS or more.
    - The same SQL 100 times in 10 seconds without heavy traffic.
    - 70 `ESTABLISHED` connections, or 120 short-lived connections (`TIME_WAIT` plus `SYN`).
    - `ALTER`.
4. **CAUTION**
    - 256 KB/s average inbound traffic, 30 QPS, the same SQL 30 times in 10 seconds.
    - 35 `ESTABLISHED` connections while active, 30 short-lived connections.
    - A full-table read with no condition or `LIMIT`.
5. **Not rated as risk**
    - `INSERT`, `UPDATE`, `DELETE` with a condition, and `SELECT *` with `WHERE`, are normal app behavior and stay `SAFE`.
    - A missing `WHERE` or `LIMIT` is judged only on a complete statement; a statement cut short by capture is shown with a trailing ` ...` and raises no such alert, and long statements split across packets are reassembled first.
    - TLS and binary packets and Redis commands are not counted as queries; they count as traffic only.
    - Driver and pool housekeeping (`COM_PING`, `SET autocommit`, `COMMIT`, `SELECT 1`, `SELECT @@...`) is excluded from query counts and repeats.
6. **Alert log**
    - Each alert line carries the three most repeated SQL fingerprints of the last 10 seconds.
    - The same alert is logged once per 60 seconds unless its level rises.
    - Details and all thresholds: [ARCHITECTURE.md](ARCHITECTURE.md), section 5.

---

## Configuration

1. **Precedence**
    - Defaults, then `config.toml`, then command-line options.
2. **Keys in `config.toml`**
    - `poll_interval_ms`: polling interval, clamped to 250 to 10000.
    - `enable_pcap`: `false` keeps only the socket table.
    - `interface`: capture interface name or description substring; without it, every interface that has an address is used.
    - `ports`: extra watched ports.
    - `hosts`: table of `"IP" = "alias"`; a key that is not an IP address stops startup.
    - `console_font`: classic-console (conhost) font, default `D2Coding ligature`; an empty string keeps the current font; Windows Terminal ignores it.
    - `[thresholds]`: risk thresholds; an unknown key stops startup. `docs/config.example.toml` lists every key with its default.
3. **Log files**
    - Logs always go to `~/.config/database-monitor/logs/database-monitor-YYYYMMDD.log`, also when `--config` points to another folder; `~` is `USERPROFILE`, or `HOME` when that is unset.
    - With no home folder, logging is off and one line on stderr says so; there is no fallback to the current folder.
    - Alert lines contain host aliases and real IP addresses, so do not share log files.

---

## Command-Line Options

1. **Options**
    - `--config <path>`: config file path.
    - `--interval-ms <ms>`: polling interval, 250 to 10000, default 1000.
    - `--no-pcap`: disable packet capture and use the socket table only.
    - `--interface <text>`: capture interface name or description substring.
    - `--ports 3307,15432`: extra watched ports.
    - `--host IP=ALIAS`: extra host alias, repeatable; a value that is not an IP address stops startup, and a missing alias becomes `CUSTOM-DB`.
    - `--once`: capture one non-interactive snapshot and exit.
    - `--json`: print the snapshot as JSON; implies `--once`.
    - `--help` and `--version` are provided by clap.

```powershell
database-monitor --host 192.0.2.10=DEV-DB --ports 3307
```

---

## TUI Controls

1. **Keys**
    - `Tab` / `Shift+Tab` / `1` to `4`: switch tabs (`1 Overview`, `2 Agents & Processes`, `3 Top SQL`, `4 Alerts`); `1` to `4` follow the order shown in the tab bar.
    - `<` / `>`: move the current tab one slot left or right; the order is saved.
    - `↑` / `↓` / `k` / `j` / `PgUp` / `PgDn` / `Home` / `End`: move the selected row, or scroll the focused card; `k` moves up and `j` moves down like the arrows, and `PgUp` and `PgDn` move one page.
    - `s` / `S`: cycle the sort of the focused table, the same cycle as a header click.
    - `r`: collect now; `Space` / `p`: pause or resume updates.
    - `+` / `-` (also `=` and `_`): polling interval in 250 ms steps, 250 to 5000 ms.
    - `x` / `F9` / `F12`: open the confirmation to stop every `CRITICAL` local process; `Y` confirms.
    - `K` (Shift+k): confirmation overlay to kill the selected process on tabs 1 and 2; `Y` confirms.
    - `t`: always-on-top toggle, enabled automatically on danger; `b`: alert blink toggle, default on.
    - `Shift+←` / `Shift+→` / `Shift+↑` / `Shift+↓`: move the focused card one slot (`←` `→` in a row of cards, `↑` `↓` in a column).
    - `l`: reset card order, card sizes, and column widths of the current tab; the language stays.
    - `L`: switch the display language between English and Korean.
    - `?`: help overlay; any key or a click closes it.
    - `!`: force restart, only after a new build replaced the running exe on disk: restores the terminal and reruns the TUI from the latest exe with the same arguments.
    - `q` / `Esc` / `Ctrl+C`: quit.
2. **Mouse**
    - Click a tab, a footer chip, a card (focus, thick border), or a row (select).
    - Click a column header to sort; drag a header edge to set a column width; double-click the edge for automatic width.
    - A column edge shows a faint `│` at rest and can be grabbed from one header cell on each side; on hover or drag a `┃` spans the full table height; double-clicking the edge cell restores the automatic width, and clicking the header cell beside it sorts.
    - Drag a tab to change the tab order; `Esc` cancels.
    - Drag a card border to resize cards; the sizes are kept as ratios.
    - Hold a card title for 400 ms, then drag to move the card with a live preview (dashed border); release to place it, `Esc` to cancel.
    - A button released outside the window is noticed at the next mouse move: it ends a card move, card border drag, column width drag, or scrollbar drag like a release, and a moved drag is saved.
    - The wheel scrolls the card under the pointer; drag the scrollbar thumb, or click the track to move one page.
3. **Sorting**
    - The first click on a numeric or time column sorts descending (largest or latest first); a text column sorts ascending first.
    - The second click flips the direction; the third click returns to the default order.
    - The active sort shows in the card title, for example `· Sort Executions↓`.
    - Sorting uses typed column values, so switching the language never reorders rows or moves the selection.
4. **Tables**
    - Columns keep their natural width; text columns shrink first (with `…`) and the right-most columns hide last.
    - The risk level column is pinned: it hides only after every other column, so it stays visible at 100x30 and 60x16.
    - A scrollbar (track `░`, thumb `█`) sits in its own gray gutter inside the card border, never in the focus color.
    - A scrollable card shows ` a-b / total ` on its bottom border.
5. **Footer**
    - Every chip is clickable and mirrors its key; when the width runs out the left chips drop from the right, the `[L]` language chip drops last, and the help and quit chips stay pinned right.
    - When a new build replaces the running exe on disk (checked every 2 seconds), a warn-colored `[!] Force restart` chip leads the footer and stays visible at the minimum size; clicking it does what `!` does.
    - The hint row shows a toast first (4 seconds; only an error toast is a red fill), then the keys that apply to the pointer, the focused card, and the tab.
6. **Minimum size**
    - The minimum is 60x16; below it, one line reads `Terminal too small (min 60x16, now WxH)` and nothing else is drawn.

---

## State File

1. **Location**
    - `~/.config/database-monitor/tui.toml`, next to `config.toml`; `~` is `USERPROFILE`, or `HOME` when that is unset.
    - When the file is a symbolic link, the link stays and the file it points to is updated; a chain of up to 16 links is followed, and a longer chain or a loop is reported as a save error.
2. **Content**
    - `version`, `language`, card orders, split ratios, column widths, and the tab order; only values that differ from the defaults are written.
    - A hand-edited invalid entry is skipped and the rest still apply; column widths are capped at 512 cells.
    - The tab order is written only when it differs from the default, as `tab_order = ["processes", "overview", "top_sql", "alerts"]`; a value that is not a permutation of the four tabs is skipped, and the `l` reset keeps the tab order.
    - Sort order and the selected row are not saved.
3. **When it is written**
    - Only on a committed change: a finished card move, card border drag, column width drag, or tab move (drag or `<` / `>`), the language switch, and the `l` reset.
    - Rendering, hovering, scrolling, sorting, and selecting never write.
    - A writer thread does the write, so the input loop never waits for the disk: requests coalesce (only the latest content per file is written), a failed write shows an error toast, and on quit the last request is written within 5 seconds, with a warning on stderr if the disk stalls longer.
    - The write goes to a temporary file next to the target, is synced, and is renamed over the file.
    - Only stale temporary files (untouched for 60 seconds) are removed at startup, so the file of another running instance stays.
    - A file that cannot be read, or has another version, is copied to `tui.toml.bak` before the first overwrite, and a notice shows once.
4. **Example**

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
orders: card order of a group, listed slot by slot (the card index placed in each slot)
ratios: border positions in ten-thousandths of the group length
column_widths: width of each column in cells, 0 means automatic width
```

---

## Troubleshooting

1. **The exe fails to start with `0xc000007b`**
    - A missing or 32-bit `wpcap.dll` is the usual cause; this is a bit-width mismatch error.
    - Install the x64 Npcap runtime from [npcap.com](https://npcap.com/#download), then start again.
2. **The build fails to link `wpcap`**
    - The Npcap SDK is not on the linker path.
    - Set the `LIB` environment variable to the SDK `Lib/x64` folder (not the x86 `Lib` folder) and build again.
3. **The mode shows `Passive Socket-Only`**
    - Capture could not start; the cause is not reported. Possible causes are a missing Npcap, no usable interface, or a device that could not be opened.
    - Install or repair Npcap for x64 and try an elevated terminal.
    - In this mode there is no SQL text and no response size.
4. **No config file loaded**
    - The startup notice says that no file was found and only the default ports are watched.
    - Place `config.toml` in `~/.config/database-monitor/` or pass `--config <path>`.
5. **Permission message on stop**
    - The target process runs elevated.
    - Run the monitor in an elevated terminal, or stop the app directly.
6. **Garbled box lines or letters**
    - Use a font with box-drawing and Hangul glyphs, such as `D2Coding ligature`.

---

## Development

1. **Checks**
    - `cargo test`: unit tests under `src/`, the engine and risk tests in `tests/monitor.rs`, and the screen tests in `tests/tui.rs`.
    - `tests/tui.rs` renders every tab in English and Korean at 60x16, 100x30, and 130x40, and checks cells, hit areas, and gestures with an injected clock.
2. **Code rules**
    - Numbered divider comments in source (`// N. Title ---`) are 80 columns wide; `tests/monitor.rs` checks this.
    - Source and tests use only `192.0.2.0/24` example addresses.
    - The `src/` root holds only `main.rs` and `lib.rs`; modules live in `app/`, `analysis/`, `collector/`, `platform/`, and `tui/` ([ARCHITECTURE.md](ARCHITECTURE.md), section 2).
    - Every Cargo dependency names a full `x.y.z` version in `Cargo.toml` (ratatui 0.30.2, crossterm 0.29.0), and `Cargo.lock` records the resolved set.

---

## License

1. **License**
    - Apache-2.0, see [LICENSE.md](LICENSE.md).
