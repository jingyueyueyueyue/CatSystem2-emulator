# 02/05 翻页取证与 04/06 后继页验收

2026-11-20。此表只供验收，不授权正文输入。`g/r` 为 CST 组和组内索引，`abs` 为绝对记录号；下表的正文源是非空 `0x20`，等待是 `0x02`。帧号只定位日志，静态摘要只标识预期资源，不替代运行时获选实例。02、05 的所列正文均为日文 CP932，不算汉化通过。

## 07、04、06 单槽重跑

08:17 批次 `20261120_081753_417370_obfhg8ow_playthrough` 的 `07.events.log:253` 为 `title.mpg` handle 7 的加载，`:1141` 为 f299 同 handle 释放，`:3802` 为 f1141 `[2,176,256,0]`。`07.host.log:3–5` 记录启用的遮罩区域 20、一次鼠标按下和物理回执。事件到 f1183 截断，07 批跑回执缺失。`03.json` 只记录 03。07 本次仍在标题入口后的过渡，`71/0x9b3` 在标题也出现。此前完整的 `20261120_071449_937759_dvb0l0be_playthrough` 在 f1271 加载 `com01`、f1369 加载 `OPmovie.mpg` handle 255、f3024 释放同 handle、f3354 到首个 `[input]`，f4200 仍有可见正文与等待提示。

主线程把 `$candidateReceipt` 设为本次批准候选的 `build_identity.json`，在 workspace 根执行。08:17 的 `1338148797bb86a09731475d4e1aea4c0efc7ae463728accf5a75372af657798` 和 07:14 的 `836cf850cbb2abf8954524a860f0b315c088982ecf0041493d33771d02ec457d` 只用于对照历史。

```powershell
$candidate = Get-Content -Raw -LiteralPath $candidateReceipt | ConvertFrom-Json
$player = $candidate.player
if ((Get-FileHash -LiteralPath $player -Algorithm SHA256).Hash -ne $candidate.sha256) {
    throw 'Replay candidate SHA-256 mismatch'
}
python scripts/analysis/run_playthrough_batch.py --player $player --indices 7 4 6 --scenarios scripts/analysis/pc_story_04_06_07_wait_observe.json --jobs 1 --timeout 1200
```

Windows 默认输出在 `F:/CS2-build-cache/playthrough/2026-11/<本次批次>_playthrough`，以命令打印的实际目录为准。收 `build_identity.json`，以及每个编号的 `.json`、`.events.log` 和 `.host.log`。逐个检查回执和日志，`visual_review_required` 只确认固定帧探针正常退出。点击遮罩不符时该游戏结束并记 `input_missed`。遇到引擎错误、入口偏离、收不到匹配回执或缺少正文来源及代次时，记录实际停点并停止该路线的后续输入。

恢复后核对到 18:00 和 18:05 批次共用同一候选，播放器 SHA-256 `cf2ac89ef4e9d6e16ac188e76e8ad9d1c33cfd1bc1747f8cb616166f1d62b24a`，源码树 SHA-256 `17001befc123c27d61f7759aacb6138dd511179c90dda73abc9887c2e178b38e`。可用 `20261120_180508_351007_of5zbm_b_playthrough/build_identity.json` 指定这个已落盘的候选。两批目前只有 03、17 的回执，07、04、06 仍待主线程合批。

| 游戏 | 本轮可执行入口 | 本轮首个正文停点 | 在另一次验证中可推进到 |
| --- | --- | --- | --- |
| 07 | f1140 `(176,256)`，遮罩 20；f1141 对照根点击回执；此后自然等待 | `com01` g0 abs **56/57**。同一个 OP handle 自然释放，当前 `meswnd` 直接读 local 100，实际投影有 `keywait` 和同代正文。`WAIT_KEY=false` 是这个窗口的既有行为 | 先自然完成两个 40 单位淡入淡出和中间等待，再到 g1 abs **61/62**。当前作为后继候选 |
| 04 | f215 `(638,446)` 遮罩 19、f390 `(165,386)` 遮罩 15 | `__tar01` g0 abs **53/54**。自然 `rdraw 60`、两次同 task 的 530/531 擦除和禁用，根 `71/0xea8`，当前 `img_wait` 和同代正文 | g1 abs **66/67**，含自然 `rdraw 15` |
| 06 | f530 `(98,208)` 遮罩 18、f660 `(98,104)` 遮罩 10 | `C01_01` g0 abs **61/65**。abs 64 为空，根 `71/0xc72`，当前 `keywait` 和同代正文 | g1 abs **98/107**，含自然 fade 30 和 wait 210。历史单键已有此后继 |

`71e8a76` 的 `host_replay_wait_diagnostics` 已调用 `observe_07`，可查看 `legacy_07` 的窗口 local、当前 CST 和 prompt 投影。仍需核对实际加载来源与正文提交代次。这个提交里的 `observe_04_06` 没有接入诊断入口。04/06 即使出现预期正文与提示，遇到 `profile=None` 或缺实际资源绑定、`exact_body` 和 `same_epoch` 仍只登记为首等候选。源码触点是 `story_trace.rs::enable_host_story_trace` / `host_replay_wait_diagnostics` 与 `legacy_story_wait.rs::observe_04_06`。按加载者记录 `sscript`、`meswnd` 和 CST 的实际获选摘要，传入活 task、CST context/load ID，再比较当前的正文来源、自然完成与合成提交。

### 18:12 实跑结果与下一步

`20261120_181234_719918_7k21ziou_playthrough` 已落盘三个 `visual_review_required` 回执。07、04、06 分别到 f4200、f1320、f1440，均正常退出。只有场景清单规定的入口点击，无正文输入。

| 游戏 | 自然转场与首文证据 | 下一道验收 |
| --- | --- | --- |
| 07 | `07.events.log:6096` f1369 加载 `OPmovie.mpg` handle 255，`07.host.log:92`、`:139` 的合成 PTS 从 7.1s 到 64.766667s。`07.events.log:28446` f2766 同 handle 释放，f2827 继续场景脚本，`:33999`、`:34001` f3096 首文和等待，`:34029` 位于 `com01` g0 abs 57。`07.host.log:204`、`:594` 确认 owner 12、context 156、local 100=1、投影 `keywait` 可见、无影片和排队消息。正文 epoch 4 来自 f1274，f3123 自然完成并提交同代。 | `body_page=None` 且缺实际获选资源和该 epoch 对 CST abs 56 的绑定。绑定已有 epoch，允许其来源早于 OP 释放，要求自然完成和提交在释放后。`SingleAdvance::arm` 当前拒绝 07，正文 **61/62** 仍为后继候选。 |
| 04 | `04.events.log:4715` f607 的 `rdraw 60`。`:6558` 到 `:7034`、`:7901` 到 `:8377` 是同一 task `0x27` 的两轮 530 注册、531 启用、520 轮询、531 停用。`:8422`、`:8424` f852 记录首文和等待。`04.host.log:329` f1320 为 `MES_SHOW`、`WAIT_KEY=true`、根 `0xea8`。 | `profile=None`。现有 4808 个非零 alpha 像素在前后取样相同，按实际资源和活 CST 加载实例把 `__tar01` **53/54** 与新正文代次对齐，再考虑一次推进到 **66/67**。 |
| 06 | `06.events.log:2752` f821 加载 `C01_01`，`:13481` f1286 自然执行 `frameon fade 15`，`:14138` f1305 为非空正文，`:14179`、`:14181` f1306 为空正文和等待。`06.host.log:110`、`:171` 正文层从 927 到 4873，`:186`、`:188` 显示可见 `keywait`、`MES_SHOW`、`WAIT_KEY=true` 和根 `0xc72`，活动电影数为 0。 | `profile=None`。接入加载来源及代次诊断，确认 **61/65**；空白 abs 64 留作负例。再以历史单键为对照验到自然 fade 和 wait 之后的 **98/107**。 |

源码解冻后，先用同一批入口的零正文输入探针验收 `observe_04_06`，同时补齐 07 的实际获选资源与正文 `load_id`、绝对记录绑定。负例覆盖旧加载实例、旧代次、空记录、活动媒体和仅匹配根等待地址。只在正向观察通过后的独立验证里，将当次活接收者的非重复物理按下、唯一根 `[2,0,0,0]` 回执、释放和下一条精确正文等待绑定为一次输入。任一证据缺失即停在当前记录。

## 02：Happiness! 2，FES 消息窗

| 项目 | 精确预期 SHA-256 |
| --- | --- |
| 扫描所得 `cs2.exe`（packed） | `21dfeb88edafc4bf19226d158e040687738f592282c578740e9d972127938df7` |
| 原版兼容表的来源 `cs2_crack.exe` | `5869b7d03a636c200f5042f0c3fc37878a362615bf724f58d47aec5eb8b2d44c` |
| `kcs.int::main.kcs` | `d0c55d768b149cb0a63d4f69ace7184f0a6e0d89dda01e63b536076e86bb207a` |
| `kcs.int::sscript.kcs` | `a853f3e0a1e1a1476cedd4bbc293a9ef93e996cec2db9986b1dd793ca71a3d15` |
| `fes.int::meswnd.fes` | `b86c9f0669ee9339444ae716ddf4c7d73ecba8faaa2460fad3801c1fe2899d37` |
| `scene.int::0_01_01.cst` | `7fd326a8a41ee87336fa9cb496abb2c16e60822b68ede1afb2fe1d2bb8efadf2` |

- 同版 `F:/CS2-build-cache/playthrough/2026-11/20261120_081046_517865_10_iqn5q_playthrough/`，播放器 SHA-256 `f21d1d5a938a668cb5719353343e5f38adf0b5acaaec8a24350b6f883491adb2`。`02.events.log:15` 验证兼容表来源及有效 producer，不证明 packed 本身的 ABI。`:3032` f917 装载 `0_01_01`；`:3293` f924 创建活窗口 kind 34（本次 owner 35，不是早期 owner 12）；`:14236`/`:14237` f1491 首文/等待。`02.host.log:76`/`:94` f2320 有 `str[0]` 6691 alpha 和 `keywait` 566 alpha，`:97` 是 `WAIT_GAME`、`MES_SHOW`、`WAIT_KEY=true`、根 `71/0xe0d`（十进制 3597），无排队消息或影片。但 `profile`、`body_page`、`body_generation` 均未绑定，本轮未发正文键，只能验到首文和等待外观。
- 首次翻页：`0_01_01` g0/r100–101（abs **100/101**）到 g1/r1、r2、r21（正文 **103、104**，等待 **123**）。abs 99 是人名；g1 的 `wait`、粒子淡出和 abs 116 `wait 90` 自然执行，abs 122 是空正文占位。应提交 103/104 形成的同一页，不能把 122 当成新代次。另一次独立验证才从 g1/123 到 g2/r0–1（abs **124/125**）；根 `0xe0d` 也用于计时，不能独立放行。
- 历史单键对照 `F:/CS2-build-cache/playthrough/2026-11/20261120_041601_360403_y4ztgw5r_playthrough/`：`02.host.log:97`/`:98` 为 f2320 Enter 到 f2321 的物理回执（例如 region 54 → 活窗口 35）；`02.events.log:34153`/`:34154` 是正文 103/104，`:34165` 是同一次唯一根 `[2,0,0,0]`。`:34240` f2322 开始 `wait 90`，`:36403`/`:36411` f2412 才停粒子并到继任 `[input]`。`02.host.log:150`/`:168` f2520 有继任正文层 13760 alpha 与提示。它证明历史的真实翻页，不补作当前同版代次或精确输入许可。

## 05：Utate Meguri，普通消息窗

| 项目 | 精确预期 SHA-256 |
| --- | --- |
| 真正运行程序 `UTATEMEGURI.exe` | `a9f8b08713aac278671853b2293a13b2623a122ede9341cb921901b09601cb52` |
| 扫描可能显示的 `cs2conf.exe`，仅取证 | `ad70ab1bb00cc8ff9e62a333d12be2e700c1d57880fec0a6b0f8e41cad93f446` |
| `kcs.int::main.kcs` | `f3ca86f40e108dd96da7dd031bf8dda23dd4ff1a1b0e1037ee9441a6c7aec3ce` |
| `kcs.int::sscript.kcs` | `ac9c100957b552dca13f10946f6281e70d53c10be71a646b698b4a8088ccd94e` |
| `config.int::meswnd.xml`（静态普通窗资源，待记录获选来源） | `a5a2363856520ee6b0a9859da978c494fa121d87b83375ef544c9e9eedc74030` |
| `scene.int::00_01.cst`（实际首段正文） | `306ac319a2eb7589dba849bd5391ae0d189f3d851bdeb775288156a3f6f163c5` |

- 用真 PE 和有效启动 KCS 识别 Utate（与 `verified_utate_sound_loop_source` / `bind_utate_plane_input_profile` 一致）；`cs2conf.exe` 不是 ABI 等价指纹。观察实际 `sscript`、普通窗配置及 CST 的获选来源，不以 FES `MES_SHOW`、`WAIT_KEY`、`str[0]` 或文件名代替。正文输入应指向活的普通 `meswnd`（kind 12）；字形来自当前的 kind 47 `string` 对象，不是同组 `name` 或旧/隐藏的历史窗。观察配置有效、当前 `616(string)` 自然完成且新正文已合成；构造未配置对象也可能让 `616` 返回完成，不能单靠该返回放行。
- 08:10 同版 `05.json` 截止 f1720、无正文键；`05.events.log:3263` f1050 kind 12 owner 10，`:3371`/`:3791` 依次装入 `0000`、`00_00`。`00_00` 不是首文；其 g0/r40 `next 00_01`。历史 `20261120_054339_320859_tkws_1cq_playthrough/05.events.log:7910` f1770 装 `00_01`，`:8211` f1803 建立正文对象 17，`:8730`/`:8731` f1917 首文/等待；`20261120_052749_485356__z1nerb3_playthrough/05.events.log:7906` 起最后的 `616` 轮询在 f1985，f1986 后仍根 `71/0x929`，未收到新键。08:10 短跑不足以验收或判卡。
- `00_01` g0/r48–49（abs **48/49**，PCM abs 46，人名 47）一次 Enter 后到 g1/r3–4（abs **53/54**，PCM 51、人名 52）；再下一页静态为 g2/r2–3（abs **57/58**）。历史 `20261120_054339_320859_tkws_1cq_playthrough/05.host.log:76` f2050 是对象 17 generation 3 字形最终 RGB 3189/3189，`:145` f2051 有 Keyboard 13 到活 owner 10（例如 region 19），`05.events.log:9415`/`:9424`/`:9425` 为清页、新文/等待，`:9431` 为唯一根回执。`05.host.log:166` f2051 新 generation 4 只提交部分字形；`:256` f2280 才有稳定 3581/3581。旧 debug 文本 `�` 不是正文乱码证据；用 CP932 源记录与实际字形提交核对，不能比较已经损坏的日志字符串。

### 02/05 首段来源取证

- 02 在主 KCS 的实际入口、`sscript` 子 task、kind 34 的 `meswnd.fes` 装载和 `load_cst_context` 取获选字节 SHA-256。把 `0_01_01` 的 abs 100 来源绑到当次 CST load、当前消息窗正文代次及 abs 101 的等待。已有兼容表摘要可从 08:10 的 events 日志复核。abs 103/104 是另一次单键的后继目标。
- 05 从 `verified_utate_sound_loop_source` 核对真正原版 PE 与主 KCS。从实际普通窗配置载入点、kind 12 owner、当前 kind 47 string 的 `616` 和提交取同页证据。用 `load_cst_context` 绑定 `00_01` abs 48/49。首轮至该点只取证，abs 53/54 留给独立的一次翻页验收。
- 现有固定帧场景只校验鼠标命中，02/05 的早期键盘输入需要逐个确认实际入口状态。入口已知后才继续，先看 02 的遮罩 26/35 和 05 的遮罩 13/13/15。05 先观察 `00_00 → 00_01` 自然执行，把取样延至 f2050、必要时 f2280。现有 05 首文对照含 f1060/f1160 的键，先用下列命令对照已有日志。
- 两份省略 f1060/f1160 的 05 批次（03:23、08:10）都在 f1720 达到采样上限，最后进过 `00_00`。带这两键的历史批次在 f1770 进入 `00_01`。`20261120_052749_485356__z1nerb3_playthrough/05.host.log:47`、`:52` 有两次物理回执，events 的 `:3790`、`:4565` 有相应根回执，证明这两键确实送达。无键对照的上限比历史加载早 50 帧，现有对照无法判断两键是否必要。

```powershell
$logs = 'F:/CS2-build-cache/playthrough/2026-11'
$batch0810 = Join-Path $logs '20261120_081046_517865_10_iqn5q_playthrough'
$history05 = Join-Path $logs '20261120_054339_320859_tkws_1cq_playthrough'
rg -an 'particle ABI|sscript loaded > (0_01_01|0000|00_00)|debug="\[input\]"' "$batch0810/02.events.log" "$batch0810/05.events.log"
rg -an 'sscript loaded > 00_01|frame=1917 .*\[input\]|frame 2050: wait|frame 2280: wait' "$history05/05.events.log" "$history05/05.host.log"
```

### 05 入口后延长观察

`pc_story_05_first_body_no_late_keys.json` 复用 08:10 的实际入口动作，三次点击分别校验启用区域 13、13、15，f985 后只取样。f1720 对照原批次上限，f1770、f1917、f1986 对照历史 CST 加载、首文和自然完成时间。f2050、f2280 检查当前普通正文对象是否提交可见完整字形。若停在 `00_00`，留下 f2280 的最后根等待、场景和转场状态。

主线程单槽可用同版候选。以下命令从 18:12 批次读取播放器身份，实际执行时由主线程安排。

```powershell
$receipt = 'F:/CS2-build-cache/playthrough/2026-11/20261120_181234_719918_7k21ziou_playthrough/build_identity.json'
$candidate = Get-Content -Raw -LiteralPath $receipt | ConvertFrom-Json
if ((Get-FileHash -LiteralPath $candidate.player -Algorithm SHA256).Hash -ne $candidate.sha256) {
    throw 'Replay candidate SHA-256 mismatch'
}
python scripts/analysis/run_playthrough_batch.py --player $candidate.player --indices 5 --scenarios scripts/analysis/pc_story_05_first_body_no_late_keys.json --jobs 1 --timeout 600
```

## 04/06：首文之后的精确索引

每行必须先通过前一行的等待和同代合成，再单独验一次翻页；不把本表当连续键序列。04/06 入口和资源基线见 `archive/2026-11/20261120_04_06_07_route_acceptance.md`；以下组内与绝对索引均从原始 CST 表重读。

| 游戏/场景 | 等待位置 | 非空正文位置 | 进入本页时必须区分的动作或证据 |
| --- | --- | --- | --- |
| 04 `__tar01` | g0/r54 abs 54 | g0/r53 abs 53 | 旧双擦除均自然停用；同版首等已见，代次仍待验 |
| 04 | g1/r12 abs 67 | g1/r11 abs 66 | abs 62/63 自然 `rdraw 15`/等待，abs 64 `SHI_tar01_001`；以下后继均为静态候选 |
| 04 | g2/r2 abs 70 | g2/r1 abs 69 | 新正文，不复用 abs 66 的代次 |
| 04 | g3/r2 abs 73 | g3/r1 abs 72 | 新正文，不复用 abs 69 的代次 |
| 04 | g4/r11 abs 85 | g4/r10 abs 84 | abs 75 立绘变化、80/81 自然 `rdraw 15`/等待、82 `SHI_tar01_002` |
| 04 | g5/r1 abs 87 | g5/r0 abs 86 | 新正文；下一页无需额外过场键 |
| 06 `C01_01` | g0/r65 abs 65 | g0/r61 abs 61 | abs 64 空；不能作为正文来源 |
| 06 | g1/r41 abs 107 | g1/r32 abs 98 | abs 67 fade 30、86 wait 210、88/93 计时、94 fade 30；abs 106 空。历史 f1421→1762/1765 有回执及正文/等待 |
| 06 | g2/r2 abs 110 | g2/r1 abs 109 | 历史 f1921 有后继正文、等待和根回执；不是同版证明 |
| 06 | g3/r1 abs 112 | g3/r0 abs 111 | 以下为静态候选，不承接历史键时刻 |
| 06 | g4/r5 abs 118 | g4/r0 abs 113 | abs 117 空 |
| 06 | g5/r8 abs 127 | g5/r3 abs 122 | abs 120 `C_IROHA_C01_002`，126 空 |
| 06 | g6/r4 abs 132 | g6/r3 abs 131 | abs 129 `C_IROHA_C01_003` |

## 主线程测试清单

1. 同一候选、单槽、`capture-every:0`、`no-final-capture`、`story-events`、`story-trace`。02 复用 08:10 入口并核对实际两次点击（masked 26、35）及相应回执；04/06 复用 `pc_story_04_06_07_wait_observe.json` 的命中守卫。05 首轮只延长 08:10 的原入口窗，观察 `00_00 → 00_01 → 48/49`，不追加老对照的 f1060/f1160 Enter；两键历史上都有根回执，未证明新版自然路径需要它们。不通过则停在具体场景/等待，判定是入口分叉、自然转场未完还是新错误，再考虑独立对照。
2. 零正文键验收：同时记录获选 PE/KCS/窗和 CST 指纹、加载 ID/context、组和绝对索引、活窗口/文本对象、已完成且最终可见的正文代次。没有实际绑定就记待验；`inspect-log` 的帧只是取样，`trace_enabled=false`、`profile=None` 或单纯像素值不能算已就绪。原版/汉化分开验。失焦、暂停、菜单、选项、自动播放、活动媒体/合成、排队消息或 handler 未结清都拒绝发键。
3. 授权后的一次性验证：一次正常 `Keyboard(13)` down/up，只接受同一物理按下的 `pressed=1`、`repeated=false`、`response=1` 且命中当下活正文窗口；用加载实例与输入序号对齐恰好一次根 `message-resume [2,0,0,0]`，然后核对未跳过转场、准确继任记录和**新**的完整可见正文代次。同一按键会扇出多个路由回执（含其他 listener），不能据此算多次翻页；只有 root 或无关 listener 响应不算接收。若实际是独立的第二次按下、两个根回执、只收到回执未进后继，或任何 provenance 缺失，立即留证停止，不重发。
4. 负例至少覆盖：替换 PE/producer/窗/CST 之一，旧加载 ID 或旧正文代次，空 02/06 记录，02 同地址计时等待，05 错误 kind 或未配置 `616` 报完成，只有人名/旧页像素，收到 Enter 但没有新且正确的合成。预期全部不授权；本文件不改 `legacy_story_wait.rs` 或共享 trace，不是已接线的测试实现。
