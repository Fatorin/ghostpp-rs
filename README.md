# ghostpp-rs

[English](README.en.md) | 繁體中文

**ghostpp-rs** 是經典魔獸爭霸 III 主機機器人 [GHost++](https://github.com/uakfdotb/ghostpp) 的 Rust 重寫版,目標平台為 **Warcraft III 1.26~1.28 + PVPGN 伺服器**(如私服/對戰平台)。以 tokio 非同步 actor 架構全面取代原版 C++ 的單執行緒 50ms select 輪詢,協定層與 C++ 原版逐 byte 對照移植。

## 功能

- **PVPGN 登入**:CD key 解碼、XSHA1 密碼雜湊、checkRevision(exe hash)全部以純 Rust 實作(內嵌 bncsutil 移植),不依賴外部 C 函式庫
- **建房與大廳**:STARTADVEX3 廣播與 3 秒刷新、玩家加入/slot 管理、隊伍/顏色/種族/讓分自由切換、HCL 模式字串編碼
- **地圖下載**:MAPCHECK / STARTDOWNLOAD / MAPPART 滑動視窗傳輸,大廳內即時顯示下載進度
- **完整遊戲進行**:載入同步、可調延遲(5~500ms)的 action 批次迴圈、keepalive desync 偵測、lag screen(依 latency 自動換算容忍度)
- **Autohost**:自動連續開房、滿員自動開局
- **GProxy++ 斷線重連**:送出緩衝 + ACK 修剪、斷線保留玩家、重連緩衝重送、逾時安全移除
- **Spoofcheck**:密語 `sc` 身分驗證(GProxy 自動發送),遊戲內管理指令一律要求驗證後才依 realm 比對權限
- **資料庫**:SQLite(預設,零設定)與 PostgreSQL 皆內建,單一 `db_url` 切換;admin / ban / 遊戲與玩家紀錄
- **Replay 儲存**:每場自動存 `.w3g`(zlib 分段 packed 容器,W3 客戶端可直接播放)
- **i18n**:使用者可見訊息全部走語言檔(內建繁體中文與英文,`bot_language` 一行切換)
- **指令**:31 個 battle.net 密語指令 + 30+ 遊戲內指令,完整清單見 [COMMANDS.md](COMMANDS.md)

## 架構

```
main ─┬─ BotCore(事件迴圈:指令分派、權限、autohost、db)
      ├─ BnetActor(每個 PVPGN 連線:登入狀態機、防洪水佇列、廣播刷新)
      ├─ GameActor(每場遊戲:大廳/下載/遊戲進行/GProxy 緩衝/replay 錄製)
      │    └─ PlayerConn(每位玩家:read/write 兩個 task,framed codec)
      ├─ listener(host_port)與 reconnect listener(GProxy)
      └─ console(stdin 指令)
```

Actor 間全部以 `mpsc` 訊息傳遞(事件往上、命令往下),沒有共享可變狀態;協定編解碼(`src/core/`)與 C++ 原始碼逐函式對照,關鍵陷阱(字串 null 終止、整數寬度、封包順序)皆有註解標明對應的 C++ 位置。

## 建置

需求:Rust(stable)、CMake ≥ 4.1(stormlib-sys 用 CMake 編譯內含的 StormLib C++ 原始碼)。

- **Windows**:額外需要 VS2026 建置工具鏈時,需執行 `cargo update -p cmake`(cmake crate 舊版與 VS2026 不相容)。
- **Linux**:除了 CMake,還需要 C/C++ 編譯工具鏈與 zlib / bzip2 開發套件(StormLib 連結用)。Debian/Ubuntu:
  ```
  sudo apt install build-essential cmake zlib1g-dev libbz2-dev
  ```
  Fedora/RHEL:
  ```
  sudo dnf install gcc-c++ cmake zlib-devel bzip2-devel
  ```
- **macOS**:尚未實際驗證,先略過(理論上需要 Xcode command line tools + CMake)。

```
cargo build --release
```

## 執行前準備

由於版權因素,倉庫**不含**任何暴雪檔案,執行時需自備:

1. **`lib/`**:War3 安裝檔(`war3.exe`/`warcraft.exe`、`Storm.dll`、`game.dll`),checkRevision 計算 exe hash 用
2. **`maps/`**:要開的地圖檔(`.w3x`/`.w3m`)
3. **`config/`**:
   - `ghost.toml` — 主設定(埠、延遲、replay、資料庫、語言檔…)
   - `bnet.toml` — PVPGN 伺服器、帳號密碼、CD key、root admin(參考 `bnet.toml.example`;此檔含機密,已列入 .gitignore)
   - `map.toml` — 目前地圖設定

```
cargo run --release
```

啟動後 bot 會登入 PVPGN 並(若啟用 autohost)自動開房;或用密語 `!pub <房名>` 手動建房。指令用法見 [COMMANDS.md](COMMANDS.md)。

## 常用設定摘要(config/ghost.toml)

| 鍵 | 說明 |
|---|---|
| `db_url` | `sqlite://ghost.db`(預設)或 `postgres://user:pass@host/db` |
| `bot_language` | 語言檔;`config/language.toml`(繁中)/ `config/language_en.toml` |
| `bot_latency` | action 間隔 ms(5~500;`!latency` 可即時調整)|
| `bot_reconnect` / `bot_reconnectport` | GProxy++ 重連開關與埠 |
| `bot_savereplays` / `bot_replaypath` | Replay 自動儲存 |
| `autohost_gamename` / `autohost_maxgames` / `autohost_startplayers` | 自動開房 |

## 致謝與授權

本專案為 [GHost++](https://github.com/uakfdotb/ghostpp)(Trevor Hogan 原作)的重寫,協定知識與行為語意皆源自原專案,依 Apache License 2.0 授權。
