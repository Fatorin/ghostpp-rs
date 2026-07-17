# ghostpp-rs 指令參考

[English](COMMANDS.md) | 繁體中文

本文件由原始碼推導(`src/bot/mod.rs`、`src/game/actor.rs`、`src/bot/bnet.rs`、`src/bot/console.rs`),
列出 **實際實作** 的指令。與原版 GHost++ 相比刻意未實作的指令列於文末「未實作」小節。

## 觸發字元與權限

- **觸發字元**:預設 `!`。
  - battle.net 端由各連線的 `bnet_commandtrigger` 決定(預設 `!`)。
  - 遊戲內由 `bot_commandtrigger` 決定(預設 `!`)。
- **權限層級**:
  - **root admin**:設定檔 `bnet_rootadmin`(以空白分隔多個帳號,比對不分大小寫)。
  - **admin**:root admin,**或** 資料庫管理員(以 `!addadmin` 加入,依「該伺服器(realm)」判定)。
  - **一般玩家**:兩者皆非。
- **語法標記**:`<>` 必填、`[]` 選填。

### 遊戲內指令必須先 spoofcheck

遊戲內(大廳/遊戲中)除少數自查指令外,任何指令都要求發話者:

1. **通過 spoofcheck**:向 bot 帳號 **密語** `sc`(見「特殊機制」)。密語經 battle.net/PVPGN
   伺服器認證,名字無法偽造;GProxy++ client 加入遊戲時會自動發送。
2. spoofcheck 通過後,再以「通過驗證的 realm」比對是否為 root admin 或該 realm 的 db admin。

未 spoofcheck 就下指令 → bot 公開提示需要 spoofcheck;已 spoofcheck 但非 admin → 靜默忽略。
唯一例外是自查指令 `!checkme` / `!version` / `!stats` / `!statsdota`(免 spoofcheck、免 admin)。

---

## 一、battle.net 密語 / 頻道指令

來源:`handle_bnet_command`(`src/bot/mod.rs`)。

> 這些指令 **全部要求 admin**(程式在分派前 `if !is_admin { return; }`)。
> 標「root」者另外要求 root admin。回覆依原訊息是密語或頻道,以密語或頻道回覆。
> 其中大廳控制類(open/close/swap/kick/start/latency/synclimit/unhost)作用於 **目前大廳** 的那場遊戲;
> 進行中的遊戲只能用 `!saygame` / `!saygames` 或遊戲內聊天控制。

| 語法 | 權限 | 說明 |
|------|------|------|
| `!addadmin <name>` | root | 將 `<name>` 加為此伺服器的 db admin。已存在 / 失敗會回報。 |
| `!deladmin <name>` | root | 從此伺服器移除 db admin。 |
| `!checkadmin <name>` | admin | 查詢 `<name>` 是否為此伺服器的 admin。 |
| `!addban <name> [reason]`、`!ban <name> [reason]` | admin | 以名稱新增封鎖(記錄下令者、日期、原因;IP 留空)。 |
| `!delban <name>`、`!unban <name>` | admin | 移除該名稱的封鎖。 |
| `!checkban <name>` | admin | 查詢封鎖資訊(下令者、日期、原因)。 |
| `!autohost [on\|off]` | admin | `on` 開啟自動開房(需已設 `auto_host_game_name`)並立即嘗試開房;`off` 關閉;無參數顯示狀態(開關、房名、最大場數、自動開始人數)。 |
| `!say <text>` | admin | 對 **所有 bnet 頻道** 廣播文字(不是送進遊戲)。 |
| `!pub <name>` | admin | 建立 **公開** 遊戲(房名長度須 1–31)。 |
| `!priv <name>` | admin | 建立 **私人** 遊戲(房名長度須 1–31)。 |
| `!unhost` | admin | 解除目前大廳的遊戲。 |
| `!open <slot>` | admin | 開放大廳指定 slot(**1-based**,內部轉 0-based)。 |
| `!close <slot>` | admin | 關閉大廳指定 slot(1-based)。 |
| `!swap <s1> <s2>` | admin | 交換兩個 slot(1-based;需正好兩個數字)。 |
| `!kick <name\|slot>` | admin | 踢人:純數字視為 slot 編號(1-based),否則以名稱部分比對(不分大小寫)。 |
| `!start` | admin | 開始大廳倒數開局。 |
| `!latency [n]` | admin | 無參數查詢;設定 action 間隔 ms,**clamp 5~500**(見特殊機制)。 |
| `!synclimit [n]` | admin | 無參數查詢;`n` 為 **落後批次數**(換算成時間窗,見特殊機制)。 |
| `!exit`、`!quit` | **root** | 回覆關機訊息後觸發整支程式關閉。非 root 會被拒絕。 |
| `!disable` | admin | 停用建房(含 autohost)。 |
| `!enable` | admin | 恢復建房並嘗試 autohost。 |
| `!downloads <0\|1\|2>` | admin | 設定地圖下載模式:`0` 禁用(無圖玩家直接踢)/ `1` 啟用 / `2` 條件。其他值顯示用法。 |
| `!getgames` | admin | 摘要:大廳(0/1)、進行中場數、最大場數,並列出各房名。 |
| `!getgame` | admin | 顯示目前大廳遊戲的房名與 host_counter;無則回報無遊戲。 |
| `!saygames <text>` | admin | 對大廳與 **所有進行中** 遊戲廣播文字。 |
| `!saygame <host_counter> <text>` | admin | 對指定 host_counter 的那場遊戲廣播文字。 |
| `!countadmins` | admin | 統計此伺服器的 admin 數量。 |
| `!countbans` | admin | 統計此伺服器的封鎖數量。 |
| `!dbstatus` | admin | 顯示資料庫後端描述。 |
| `!channel <name>` | admin | 讓 bot 加入指定頻道。 |
| `!map [關鍵字]`、`!load [關鍵字]` | admin | 不帶參數:回報目前地圖。帶關鍵字:在 maps/ 目錄部分比對(不分大小寫)搜尋 .w3x/.w3m —— 唯一符合即載入並切換(之後建的房生效,現有房不受影響);多筆符合列出前 5 筆。 |

---

## 二、遊戲大廳 / 遊戲中指令

來源:`dispatch_lobby_command`(BotCore 端,`src/bot/mod.rs`)+ `handle_admin_command`
(GameActor 端,`src/game/actor.rs`)。權限見上方「遊戲內指令必須先 spoofcheck」。

### 一般玩家可用(免 spoofcheck、免 admin)

| 語法 | 權限 | 說明 |
|------|------|------|
| `!checkme` | 一般玩家 | 私訊回覆自己的資訊:ping、是否 spoofed、realm。 |
| `!version` | 一般玩家 | 私訊回覆版本字串。 |
| `!stats` | 一般玩家 | **已列入白名單但無實作**(W3MMD 統計未完成),目前無回應。 |
| `!statsdota` | 一般玩家 | 同上,無回應。 |

### 需 admin + spoofcheck

由 BotCore 直接處理的大廳控制:

| 語法 | 權限 | 說明 |
|------|------|------|
| `!say <text>` | admin | host 身分對全場廣播(大廳 flag 16 / 遊戲中 flag 32)。 |
| `!open <slot>` | admin | 開放 slot(1-based)。開局後無效。 |
| `!close <slot>` | admin | 關閉 slot(1-based)。若 slot 有真人會先踢出。 |
| `!swap <s1> <s2>` | admin | 交換兩個 slot(1-based)。依地圖選項(固定設定/自訂隊伍)決定換法。 |
| `!kick <name\|slot>` | admin | 踢人:純數字=slot(1-based),否則名稱部分比對。 |
| `!start` | admin | 開始倒數;若有人仍在下載地圖則拒絕。 |
| `!latency [n]` | admin | 查詢 / 設定 action 間隔 ms(clamp 5~500)。 |
| `!synclimit [n]` | admin | 查詢 / 設定 lag 容忍(以批次數輸入,內部存成時間窗)。 |
| `!unhost` | admin | 解除大廳遊戲。**注意**:實作作用於「目前大廳」而非發話者所在的那場(見不一致清單)。 |

由 GameActor 處理(`handle_admin_command`):

| 語法 | 權限 | 說明 |
|------|------|------|
| `!abort`、`!a` | admin | 取消開局倒數;無倒數則私訊提示。 |
| `!openall` | admin | 開放所有目前為關閉的 slot。 |
| `!closeall` | admin | 關閉所有目前為開放的 slot。 |
| `!sp` | admin | 洗牌(將佔用中的真人玩家在各佔用 slot 間隨機重排)。 |
| `!hold <name> [name...]` | admin | 保留名額:將名字(小寫)加入保留名單,加入時消耗一次。 |
| `!mute <name>` | admin | 靜音玩家(其訊息不轉發)。名稱部分比對。 |
| `!unmute <name>` | admin | 解除靜音。 |
| `!muteall` | admin | 全場靜音:遊戲中僅擋「全體」公開訊息(flag 32 且 mode 0);隊伍 / 私訊仍放行。 |
| `!unmuteall` | admin | 解除全場靜音。 |
| `!check [name]` | admin | 私訊回覆玩家資訊(ping、spoofed、realm);省略名稱=查自己。 |
| `!trigger` | admin | 私訊回覆目前觸發字元(固定回 `!`)。 |
| `!from` | admin | 列出各玩家的 IP(國別需 GeoIP,未實作,先給 IP)。 |
| `!ping [n]` | admin | 無參數:私訊列出全體 ping;帶數字 `n`:踢除平均 ping > `n` ms 的玩家。 |
| `!drop` | admin | 遊戲中且有人 lag 時,踢掉所有 lag 中的玩家;否則私訊提示無 lag。 |
| `!end` | admin | 強制結束目前這場遊戲。 |
| `!autostart [n\|off]` | admin | 設定滿 `n` 人自動開始(全員地圖確認才倒數);`off` 或無參數關閉。 |
| `!announce [秒 訊息 \| off]` | admin | 大廳每 `秒` 廣播一次 `訊息`;`off` 或無參數關閉。 |
| `!hcl [str]` | admin | 無參數顯示目前 HCL 字串;設定會檢查已開局 / 長度 / 合法字元(見特殊機制)。 |
| `!clearhcl` | admin | 清空 HCL 字串(已開局則拒絕)。 |

> 其他未列出的字串會被 GameActor 靜默忽略(僅 debug log)。

---

## 三、console(stdin)指令

來源:`console.rs` 讀入每行 → `BotEvent::ConsoleInput` → `handle_event`(`src/bot/mod.rs`)。
本機操作台,**無權限檢查**。

| 語法 | 說明 |
|------|------|
| `exit`、`quit` | 關閉整支程式。 |
| `unhost` | 解除目前大廳的遊戲。 |
| `start` | 讓目前大廳遊戲開始倒數。 |
| `say <text>` | 對所有 bnet 頻道廣播文字。 |
| `pub <name>` | 建立公開遊戲。 |
| `priv <name>` | 建立私人遊戲。 |

> 註:console 指令 **不含** 觸發字元 `!`,直接輸入關鍵字。未知輸入僅記 warn。

---

## 特殊機制

### spoofcheck 流程與 `sc` 密語

- 玩家在 battle.net **密語** bot 帳號,訊息(去空白、轉小寫後)為 `s`、`sc` 或 `spoofcheck` 之一
  即觸發 spoofcheck(`bnet.rs` 的 `handle_chat_event`)。
- 密語經伺服器認證,`user` 即真實帳號,無法偽造。BnetActor 送出 `BnetEvent::SpoofCheck`。
- BotCore 收到後,對 **目前大廳** 同名玩家送 `GameCommand::SpoofCheck { name, realm }`;
  GameActor 標記該玩家 `spoofed=true` 並記下 `spoofed_realm`,公開廣播「已通過 spoofcheck」。
- 之後該玩家的遊戲內指令即以 `spoofed_realm` 判定 admin 權限。
- **GProxy++** client 加入遊戲時會自動發送此密語,一般玩家無需手動。

### autohost 行為

- 啟用條件(初值):`auto_host_game_name` 非空、`auto_host_maximum_games > 0`、
  `auto_host_auto_start_players > 0`。可用 `!autohost on/off` 執行期切換。
- `try_autohost` 觸發時機:bnet 登入完成、開局(大廳空出)、遊戲刪除。
- 會被以下情況擋下:`!disable` 停用中、autohost 關閉、目前已有大廳遊戲、進行中場數達
  `auto_host_maximum_games`、地圖無效、無任何 bnet 連線。
- 開出的房名為 `「房名 #N」`(N 為遞增計數)。房內滿 `auto_host_auto_start_players` 人
  且全員完成地圖確認即自動倒數(`maybe_autostart`)。

### `!hcl` 字元 / 長度限制

- 合法字元集:`abcdefghijklmnopqrstuvwxyz0123456789 -=,.`(常數 `HCL_ALLOWED_CHARS`)。
- 長度不得超過 **目前佔用中的 slot 數**(`occupied_slot_count`),否則回「too long」。
- 含非法字元 → 回「invalid chars」。已開局(`started`)→ 拒絕修改。
- 開局時把 HCL 字串編碼進各佔用 slot 的 handicap 欄位,供地圖端解碼選模式;
  初值取自地圖預設 HCL(`map_defaulthcl`)。

### `!latency` / `!synclimit`(lag 容忍模型)

- `latency_ms` 初值來自設定 `bot_latency`(預設 100),**clamp 5~500**(`LATENCY_MIN`/`LATENCY_MAX`)。
- lag 容忍採「時間窗」`sync_window_ms`(初值 `SYNC_TOLERANCE_MS = 5000` ms)。
  實際觸發 lag 畫面的落後批次數 = `sync_window_ms / latency_ms`(至少 1),隨 latency 自動換算。
- `!synclimit <n>`:把使用者給的 **批次數** 換算回時間窗 `n × latency`,並 clamp
  `SYNC_WINDOW_MIN_MS(500)` ~ `SYNC_WINDOW_MAX_MS(30000)` ms。

### `!ping` 的 RTT 來源與 `lc_pings`

- RTT 由 `W3GS_PONG_TO_HOST` 計算:host 送 PING 時放入 `get_ticks`,pong 回來時
  `RTT = 現在 ticks − pong`。第一個 pong(常為 1)丟棄;RTT ≥ 60000 ms 視為異常濾除;
  每位玩家保留最近 10 筆,顯示取平均。
- `lc_pings`(設定 `bot_lcpings`)為真時,顯示值再 **除以二**(單程估計)。

---

## 未實作(對照原版 GHost++,刻意不做)

| 指令 / 功能 | 未實作理由 |
|------|------|
| `!savegame` 系列(load/host saved game) | 存讀檔續玩流程未移植。 |
| `!hostsg` / admin game(`!` admin-only 房) | 專用管理遊戲介面未移植。 |
| matchmaking(`!pub`/`!priv` 以外的配對) | ELO/配對系統未移植。 |
| warden(反作弊模組) | Warden 質詢/回應未移植。 |
| `!votekick` | 投票踢人未移植。 |
| comp 系列(`!comp`/`!compcolour`/`!comprace`/`!comphandicap`/`!compteam`) | 加入電腦玩家未移植。 |
| `!owner` / `!lock` / `!unlock` | 房主鎖定機制未移植(權限改以 spoofcheck + admin 判定)。 |
| `!stats` / `!statsdota` | W3MMD 統計未完成;指令已列入白名單但目前無回應。 |
| `!from` 的國別查詢 | GeoIP 未整合,`!from` 只顯示 IP。 |
| `!reload` | 重載設定檔未實作(換地圖請用 `!map <關鍵字>`)。 |
| `!sendlan` | 區網廣播(UDP)未移植。 |
