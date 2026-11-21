# 01 NEKOPARA Vol.3 普通存档接续路线

18:25 候选的 EXE SHA-256 为 `a44e9b11571515191c12b309aa8b9b9168a39e1c11726c8e6a64ce8ee62a8fb1`，同源回执见 `archive/2026-11/20261120_182559_920636_story_replay_integration/build.json`。回放控制器该版的连续正文白名单止于 `01 22/200`。`to-213`、`to-218`、`to-337` 可生成现有格式的路线，分别需要控制器识别到 g23、g24、g25 的精确等待，并在 `1038` 输出同范围的 CSTL 回执。下表的绝对记录、局部序号和资源身份来自 `pc_story_neko_scene01_to_kitchen_candidate.json`。

| 等键位置 | 正文记录 / 组内序号 | 后继与输入门槛 |
| --- | --- | --- |
| `01 22/200` | `198 / 2` | 双语基线收齐后，在新回放中守住同一 `meswnd` 只发一次正文点击，等待淡出、音频、计时自然完成。 |
| `01 23/213` | `210 / 9` | 与 r200 同一 CST context/load 及 `meswnd` 实例。观察轮停在这里。接续轮在本页另发一次相同守卫的正文点击。 |
| `01 24/218` | `216 / 2` | 与 r213 同一 CST context/load 及 `meswnd` 实例。先停在本页，观察正文 `(625,478)` 和 Save 候选 `(1147,612)`。 |
| `01 25/337` | `316 / 97` | 从 r218 最多一按，保留过场、动画、音频及 `rdraw`。只在最终等键收据确认后停止。 |

正文守卫使用 `meswnd/MES_SHOW`、当帧 `WAIT_KEY`、唯一 CST 上下文、自然完成的同代正文和已合成提示。`1038` 主输出 `0x00062c6b` 必须命中 `scene.int::01.cstl` 的日文列 2 或繁中列 0，逐页核对 UTF-8 长度和 SHA-256。`199/212/217/336` 是控制尾记录。r200、r213、r218 的正文点击在 `(625,478)` 核对启用命中 77、候选 `[3,77,52,57,7]`，实际由 region 52 及本页 `meswnd` 消费，`key=0 group=0 response=1`。控制器等物理释放后才核验后继。未知等待、接收者变化或漏回执即停。

## 主线程命令

已归档 `archive/2026-11/20261120_neko_r200_to_r218_route.patch`。补丁对当前主检出的 `story_route/mod.rs` 和 `cst_context_native.rs` 增补 `[23,213]`、`[24,218]` 和对应的场景 01 来源诊断，复用已有相邻页输入约束。它把现有路线单测的终点扩至 r213 和 r218，继续验证跳页和越过 r218 的路线被拒绝。主线程可在主检出选择应用后构建新的 PC 集成候选。

```powershell
$patch = 'archive/2026-11/20261120_neko_r200_to_r218_route.patch'
git apply --check --whitespace=error-all $patch
```

从工作区运行。`--print-plan` 只读并输出 JSON。用包含相应白名单和来源日志的已归档集成 EXE 调用 staging，随后按 `stage.json` 的 `runs.jp/cn.argv` 启动。每次 staging 都从冷启动新游戏走到指定终点，回放器退出后不会保留可继续推进的进程。

```powershell
$stageScript = 'scripts/analysis/stage_neko_cold_language.py'
$check = 'scripts/analysis/check_neko_save_roundtrip.py'
python -X utf8 -B $stageScript --print-plan --startup-probe to-213
python -X utf8 -B $stageScript --print-plan --startup-probe to-218
python -X utf8 -B $stageScript --print-plan --startup-probe to-337

# $exe and $sha come from the archived build.json for the corresponding cutoff.
$mode = 'to-213'
$receiptPath = python -X utf8 -B $stageScript --player $exe --sha256 $sha --startup-probe $mode
if ($LASTEXITCODE -ne 0) { throw 'NEKO staging failed' }
$stage = Get-Content -LiteralPath $receiptPath -Encoding UTF8 -Raw | ConvertFrom-Json
$language = 'jp'
$run = $stage.runs.$language
$argv = @($run.argv)
$encodedArgs = ($argv | Select-Object -Skip 1 | ForEach-Object { '"' + $_ + '"' }) -join ' '
$env:CS2_MESSAGE_REVEAL_TRACE = 'fes'
$proc = Start-Process -FilePath $argv[0] -ArgumentList $encodedArgs -WindowStyle Hidden -PassThru -Wait `
    -RedirectStandardOutput $run.stdout -RedirectStandardError $run.stderr
if ($proc.ExitCode -ne 0) { throw "NEKO player exit $($proc.ExitCode)" }
python -X utf8 -B $check probe --run $run.run --language $language --to $mode
```

完成双语时，对两个已退出的目录各跑一次 `probe`，再用 `check_neko_cold_language.py --jp $jpRun --cn $cnRun --startup-probe to-213 --summary` 核对二进制和源码身份一致。将 `to-213` 换为 `to-218` 后，单语言可加 `--require-save-hit`，双语原检查器也支持该参数。检查器退出码 `0` 为通过，`1` 有矛盾证据，`2` 缺回执。`--require-save-hit` 通过只说明该点命中启用候选及活接收者。

独立进程的标题观察使用相同私有输出前缀，日志另命名。先只生成标题 Load 候选观察，可在观测到语言弹窗时改用 `--startup language-select`，它只确认已匹配的语言，再停在标题。

```powershell
# $privateRun is the producer's archived private run directory.
$prefix = Join-Path $privateRun '01'
$loadProbe = Join-Path $privateRun 'load-title.route.json'
python -X utf8 -B $check title-plan --language jp --startup title | Set-Content -LiteralPath $loadProbe -Encoding ASCII
# Use the same launch pattern above with this argv and distinct output log names.
$argv = @($exe, $game, $prefix, '3600', '--route-json', $loadProbe)
```

## 普通存读档步骤

19:06 候选的 `archive/2026-11/20261120_neko_to_r218_receipt.json` 记录日文 f3328、繁中 f2873。`(1147,612)` 同为启用 region 45，候选 `[3,45,77,52,57,7]`，region 45 归活 `meswnd` owner 35。单次入口补丁使用这个实际命中，同时在当帧验证按钮绑定为 `btn` bank 4。一次物理左键后，在 `saveload/WAIT` 核对 Save 模式、保留的 r218 CST 来源及空槽 0，再观察 `(285,184)` 与活绑定 `btdata[0]`。终点零输入。入口检查器复核全过程的 25 页来源与物理点击，将面板回执与同帧 host 探针交叉比对，并检查私有 `save0000.dat` 缺席。

```powershell
$entryPatch = 'archive/2026-11/20261120_neko_r218_save_entry_once.patch'
git apply --check --whitespace=error-all $entryPatch
if ($LASTEXITCODE -ne 0) { throw 'NEKO Save entry patch check failed' }
# Stage a new private profile with the EXE built after applying the entry patch.
$receiptPath = python -X utf8 -B $stageScript --player $exe --sha256 $sha --startup-probe save-entry
if ($LASTEXITCODE -ne 0) { throw 'NEKO Save entry staging failed' }
$stage = Get-Content -LiteralPath $receiptPath -Encoding UTF8 -Raw | ConvertFrom-Json
$language = 'jp'
$run = $stage.runs.$language
# Launch using the argv and log redirection above, then check the exited run.
python -X utf8 -B $check save-entry --run $run.run --language $language
```

入口成功回执 `neko-save-panel` 包含面板 owner、当帧 `WAIT`、模式 globals/locals、r218 的 context/load/owner，以及空槽探针的启用 region、候选顺序和接收者。用这份实测记录接下一轮空槽单次写入。`saveload.fes` 的 `btdata.PUSH_L` 在空槽分支直接到 `SAVE_DATA2`，而已存在的 Load 槽经过 `CONFIRM_LOAD` 与 `confirm2`。写入后等面板重新进入 `WAIT` 再取退出后的文件摘要。FES 还在写入后执行日期字段 `saveapend_str`，以实际最终 CSS 字节为准。

### 写槽和重开读槽增量

`archive/2026-11/20261120_neko_r218_ordinary_slot_increment.patch` 以 19:34 候选附近已合入单次入口的源码为基线。`menu-route --operation save` 复用已验的 r218 入口，在同一 `saveload/WAIT` 探针确认 `btdata[0]` 后发一次左键，等待唯一成功的 `fes-datasave` 和槽存在时重新进入 `WAIT`。空槽路径由 `saveload.fes` 第 901–928、1100–1132 行确认。`menu-route --operation load` 用独立进程的实际启动状态选择路线。它在每次点击前核对槽摘要、语言、当帧 FES 等待、启用的活控件和接收者，并等到扫描确认释放才进入下一步。`saveload.fes` 第 1145–1172、1200–1206 行和 `ttlflow.fes` 第 386–400 行给出读槽分支。`confirm2.fes` 第 129–147 行把 `btn[0]` 对应到确认并返回 `1`。

19:39 候选的面板日志在 `F:/CS2-build-cache/playthrough/2026-11/20261120_194443_871525_z0f03eay_neko_language_stage/`。日文 f3329、繁中 f2874 的 Save 点击均由 region 45、owner 35 消费。日文 f3357 和繁中 f2902 的空槽 `(285,184)` 均命中 region 32、`saveload` owner 12，候选 `[3,32,77,52,57,123,7]`。原始探针在日文 `01.host.log` 第 14717 行和繁中第 11858 行。写槽守卫要求当帧命中和命名绑定匹配这份双语实测。读档面板使用当帧 FES 命名绑定和实收回执。

```powershell
$increment = 'archive/2026-11/20261120_neko_r218_ordinary_slot_increment.patch'
git apply --check --whitespace=error-all $increment
if ($LASTEXITCODE -ne 0) { throw 'NEKO ordinary slot increment check failed' }

# With the integrated EXE, stage one fresh private profile per language.
$stagePath = python -X utf8 -B $stageScript --player $exe --sha256 $sha --startup-probe save-entry
if ($LASTEXITCODE -ne 0) { throw 'NEKO ordinary slot staging failed' }
$stage = Get-Content -LiteralPath $stagePath -Encoding UTF8 -Raw | ConvertFrom-Json
$language = 'jp'
$run = $stage.runs.$language
$prefix = $run.argv_prefix[2]
$writerPlan = Join-Path $run.run 'write-slot0.route.json'
$json = python -X utf8 -B $check menu-route --language $language --operation save
if ($LASTEXITCODE -ne 0) { throw 'NEKO writer route failed' }
$json | Set-Content -LiteralPath $writerPlan -Encoding ASCII
$writerArgv = @($run.argv_prefix) + @('25000', '--route-json', $writerPlan)
# Launch $writerArgv, log the PID and times, and wait for normal exit.

$slotPath = "${prefix}.profile/save0000.dat"
$json = python -X utf8 -B $check slot $slotPath
if ($LASTEXITCODE -ne 0) { throw 'NEKO ordinary slot failed CSS verification' }
$slot = $json | ConvertFrom-Json
$readerPlan = Join-Path $run.run 'read-slot0.route.json'
# Set $startup from the observed cold title path: title or language-select.
$json = python -X utf8 -B $check menu-route --language $language --operation load --startup $startup --slot-sha256 $slot.sha256
if ($LASTEXITCODE -ne 0) { throw 'NEKO reader route failed' }
$json | Set-Content -LiteralPath $readerPlan -Encoding ASCII
$readerArgv = @($run.argv_prefix) + @('12000', '--route-json', $readerPlan)
# Launch $readerArgv as a new process with separate stdout/stderr logs.
python -X utf8 -B $check roundtrip --receipt $roundtripReceipt --to 218
```

每种语言用各自的 profile 和进程回执，`save_confirm=false`。仅接受进程退出后完整 CSS 的大小与摘要。Load 路线在标题、槽位、确认三个实等键分别验证命名按钮，实际 `247` 读取的 CSS 摘要须一致。确认前的槽读取单列计数，验收以确认后到 r216 来源之间的唯一成功打开为准。r216 自然完成并合成后，零输入停到 `01/24/218`。旧 `20261120_neko_r218_ordinary_save_load.patch` 留作草案存档。

`menu-plan` 生成菜单控制器接线规格及已可复用的类型化开场步骤。18:25 的 `host_replay_menu_ready` 只为 ISLAND 分类菜单，NEKO 标题输入仅允许原语言确认与 New Game，正文也只允许白名单的相邻页点击。菜单步骤需要对应的 NEKO 控制器和运行日志后才可传入回放器。主线程可按步骤逐次添加只观察终点，再把实际快照匹配的接收者接入下一次输入守卫。

```powershell
python -X utf8 -B $check menu-plan --language jp --startup title --to 218
python -X utf8 -B $check slot (Join-Path $privateRun '01.profile/save0000.dat')
python -X utf8 -B $check roundtrip --receipt $roundtripReceipt --to 218
python -X utf8 -B $check menu-plan --language jp --startup title --to 337
python -X utf8 -B $check roundtrip --receipt $kitchenReceipt --to 337 --require-kitchen-assets
```

1. **写入进程。** 新私有 `.profile`，从语言确认和 New Game 重走到 r218。在当前页匹配 Save `meswnd btn[4]`、命令 19。首次菜单入口只发 Save 一次，停在 `saveload/WAIT` 并探测 `btdata[0]`。确认模式为 globals `800=0,801=1,860=0`、locals `100=0,110=0`，槽 0 为空。槽候选点为 `(285,184)`。确认该槽和活接收者后再建立写档路线。若现场观察到 `saveload/CONFIRM_WAIT` 与 `confirm2/WAIT`，确认按钮候选 `(559,377)`，使用 `menu-plan --save-confirm`。逐个控制的 region、实际消费 region、候选顺序及 response 由本版运行观察确定。
2. **写入结束。** 使用 `Stop after ordinary Save` 停止。要求唯一 `fes-datasave slot=0 container=<非零> written=true`。写入进程正常退出后，核对 `save0000.dat` 的 `CSS\0` 魔数、头部声明长度、文件大小和 SHA-256。写入失败、自动存档、预览、文件存在和设定语言文件都各有不同意义。此路线只认指定槽的普通 Save 回执。
3. **读档进程。** 写入退出后，用相同 EXE、游戏资源及输出前缀新起进程。标题 `flow/WAIT_TITLE`、`ttlflow/FES_WAIT`、`title/WAIT` 时观察 `title btn1[1]` 候选点 `(1155,325)`。Load 面板必须为 `ttlflow/LOAD_WAIT` + `saveload/WAIT`，globals `800=1,801=0,860=1`、locals `100=0,110=1`，槽 0 存在。确认实际 Load 控件、槽 0 和观察到的 `confirm2 btn[0]` 各自的一次物理左键与接收者。面板退出后，要求实际消费普通槽的成功打开回执，以及 r218 的 r216 正文自然完成并合成，再无输入地停在 `Stop at restored 01/24/218`。若面板路径与以上 FES 契约不同，先保留状态日志，重新核对实际路径。
4. **准备 r337。** r218 重开通过后，可以从同一个私有槽另起读档进程，独立复验 r218 并用 `Turn at restored 01/24/218` 发一次正文点击。也可用新游戏的 `to-337` 路线验证不经过存读档的同一后继。专用 r218→r337 守卫应抓取点击前后 `host_replay_body_route_anchor`，确认旧 `meswnd` 收到按键与释放，保持本进程 CST context/load，且 r316 的来源帧晚于按键消费。窗实例更替需 `task-create` 回执，同一数字 ID 的重建也要重新探测。到 r337 只探测活命中。背景 `image_bg.int::bg03a_19.hg3` 和模型 `psz.int::ca01_l_body_1.psz` 的 `all_parts` 要有当帧绑定证据。若需要脸部姿态证据，用 `check_neko_kitchen_face.py` 核对独立无输入观察段的自然时钟 77/279。

### 20:10 普通写档后的 `getsavestr` 首错

日文槽 0 在 f3359 获得 `fes-datasave slot=0 container=8728 written=true`，f3361 在 `saveload/SETDATA_EXIST` 刷新已占用槽时遇到 `SAVEPANEL IMAGE property "getsavestr" has no confirmed consumer`，进程 exit 1。保留该诊断槽。CSS 长 36378 字节，完整解压 209940 字节，tag 3500，SHA-256 `f73f0918d003c84fe3841ea849874a5adebd922074e6ebe1e33484e5d406daba`。 [本次事件日志](F:/CS2-build-cache/playthrough/2026-11/20261120_201556_091776_6gbrgu1u_neko_language_stage/jp/01.events.log:42754)

同资源 `saveload.fes` 解压后 SHA-256 为 `d5cb0effc011ed64cd97d26e242b6272d70274cce9244b5dce3a3f3a336b5ffe`。第 544 行先 `setsave`，第 547、553、554、562 行依次读取下表。第 1264–1265 行还用 `tmpdata` 单银行读 `0x200` 供评论编辑。原版 PE `c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc` 的命令表条目 `0x8f65f0` 指向 `0x779750`。处理器先求值目标字符串编号，再求值记录 ID，只接受 SAVEPANEL 种类 `0x0c`，经 `0x763160` 和 `0x78c1a0` 按完整 DWORD 扫描该银行装入的 CSS。`0x779818` 遇缺记录传入空字符串，`0x740800` 写入首次 NUL 前的字节。`0x80000000/01` 与普通记录走同一查询。

| 字符串 | 记录 ID | 本次槽的结果 |
| --- | --- | --- |
| `$str480` 标题 | `0x00000201` | 46 字节记录，NUL 前 42 字节 UTF-8，SHA-256 `5ebc69334ea309bb444e8ea7125be36f231a2efde24779223b4530118ec42be3` |
| `$str481` 日期栏第一段 | `0x80000000` | 缺记录，输出空字符串 |
| `$str482` 日期栏第二段 | `0x80000001` | 缺记录，输出空字符串 |
| `$str483` 评论 | `0x00000200` | 94 字节记录，NUL 前 90 字节 UTF-8，SHA-256 `2ab0e5b239ec8aaf7f2f3936377a16fbdd8f39d92dd3d5f2c84dc28e34ab473c` |

归档增量 `20261120_neko_savepanel_getsavestr.patch` 在 SAVEPANEL 银行分派中读取既有 `SavePanelData::record`，把原始前缀字节写入游戏共享字符串表。UTF-8 优先识别，其余沿用游戏文本编码，复用 FES 对有编码标记的共享字符串展开。编码标记只携带类型，用于预览和评论，CSS 记录字节留给脚本和 KCS 原样使用。共用源码改动在 `fes_save_panel_bank.rs` 和 `cst_context_native.rs` 中，写档格式与文件日期逻辑沿用现有实现。

```powershell
$savePanelFix = 'archive/2026-11/20261120_neko_savepanel_getsavestr.patch'
git apply --reverse --check --whitespace=error-all $savePanelFix 2>$null
if ($LASTEXITCODE -ne 0) {
    git apply --check --whitespace=error-all $savePanelFix
    if ($LASTEXITCODE -ne 0) { throw 'NEKO getsavestr patch check failed' }
    # Integrate the patch before building this checkout.
}
# Build the PC candidate and use the fresh writer route above.
rg -n 'fes-datasave |fes-savepanel-getsavestr |Stop after ordinary Save|replay_status=' $writerEvents
```

新私档验收要求槽 0 只成功写一次，四个选择器都产生 `fes-savepanel-getsavestr` 回执，随后回到已占用槽的 `saveload/WAIT` 并正常结束。确认 `$str480` 与 `$str483` 的长度和摘要来自本轮 CSS，两个高位选择器以各自记录是否存在为准。写入进程退出后，按同槽摘要另起读档进程至 r218，再验正文续跑。原版处理器和本轮诊断 CSS 已离线核对，下一版需实测菜单刷新及冷读。

### 20:50 日文冷读的空 M2 加载串

同一候选的写进程 PID `61376` 在 f3359 成功写槽 0，f3362 停在 `Stop after ordinary Save` 并正常退出。独立读进程 PID `68908` 在 f584 以 `native=247` 打开同一槽，获得 CSS 句柄 `321`，进入 `LOAD_LoadObj` 后创建 `id=112 kind=54 name="m2plane 67"`，随后报 `empty M2 profile alias`。诊断槽为 `01.profile/save0000.dat`，36378 字节，前后 SHA-256 均为 `09d5fd97bd139766e2e0df07a7d679a4d2822dcca2d9216f5ecf4d062e5dae0e`。 [冷读事件](F:/CS2-build-cache/playthrough/2026-11/20261120_205040_595065_qkz4l05x_neko_language_stage/jp/load.events.log:1522) [读写回执](F:/CS2-build-cache/playthrough/2026-11/20261120_205040_595065_qkz4l05x_neko_language_stage/jp/roundtrip.json)

同版 `sscript.kcs` 在指令偏移 `704691` 用 native 244 将 `77 × 1180` 字节的对象表写入 CSS 记录 `0x1`。解压后记录头在 `0x84`，载荷在 `0x8c`。第 67 项起于 `0x13560`，其 `+0x8` 加载串起于 `0x13568`，首字节为 `00`。第 68–76 项的同字段也各以 `00` 开头。四组 `0x10010043`、`0x10020043`、`0x10030043`、`0x10040043` 系列记录各为四字节零，用于分别恢复对象表 `+808/+816/+884/+892` 的表句柄。加载串由 `0x1` 直接入盘。

冷读时指令 `716074` 用 native 254 读取 `0x1`，`716258` 逐项复制到对象表。`720926` 重建 kind 54，`721000` 调用恢复子程序，`790643` 将该对象的 `+0x8` 字符串传给 native 906。两处调用的下一指令偏移分别为 `721011` 和 `790654`。原版 PE SHA-256 `c71c5b85e6eca660474f156d96b0021a53a5f05690dd077d7861d72ff5b11adc` 的 M2Plane 处理器在加载失败后检查字符串首字节，空串于 RVA `0x2389ac` 跳至 `0x2393a0` 返回 0。native 906 包装器在 `0x24d2e8` 返回脚本状态 2，对应 `NativeReturn::Continue`。

`20261120_neko_cold_m2_empty_spec.patch` 只在 `emote_native.rs` 加一个分支。它用游戏 KCS 代码摘要和上述两处指令偏移限定普通读档调用，收到确切空串且目标没有成功导入资源时，保留已注册的 M2Plane 并继续脚本。其他加载串沿用现有资源验证。已通过 `git apply --check --whitespace=error-all` 和 `rustfmt` 的 stdin 语法解析。候选构建及读档结果由主线程验收。

```powershell
$m2RestoreFix = 'archive/2026-11/20261120_neko_cold_m2_empty_spec.patch'
git apply --check --whitespace=error-all $m2RestoreFix
if ($LASTEXITCODE -ne 0) { throw 'NEKO cold M2 restore patch check failed' }
git apply --whitespace=error-all $m2RestoreFix
# After the short standalone load, inspect the actual restored targets and checkpoint.
rg -n 'neko-slot-open|LOAD_LoadObj|neko-m2-restore-empty|task-error|Stop at restored 01/24/218' $readerEvents
python -X utf8 -B $check roundtrip --receipt $roundtripReceipt --to 218
```

分槽验收时先看同语言私档的槽 SHA 是否与 `neko-slot-open` 一致，且读前后字节摘要相同。首个 `neko-m2-restore-empty` 的 `target` 须与同帧新建的 `m2plane 67` 一致。短回放通过还要求读进程正常退出，r216 正文来源和自然合成匹配，在 `01 24/218` 的实际等键停住。现有诊断槽可供单独验证兼容性。同版普通档往返回执使用合批候选重新写入的私档。

### 21:23 冷读正文来源登记被 CSS 恢复清空

旧日文槽独立重开确认 SHA-256 `09d5fd97bd139766e2e0df07a7d679a4d2822dcca2d9216f5ecf4d062e5dae0e` 读前后相同。f584 的十个 `neko-m2-restore-empty` 分别命中重建的平面 67–76。同帧加载 `scene.int::01.cstl`，context 为 209。f604 的 native 1038 正常读取 `g24 ordinal=2` 即 r216，JP 列 2 为 84 字节，SHA-256 `2cdc860b83ba917e916fe5aa792403aab74ae562c12d2149c054ca7398b10b3d`，与写档前一致。ordinal 3 即 r217 返回零字节。f699，`meswnd` owner 122 的正文第 6 代自然完成且提交。f2262 仍停在 `01 24/218` 的实际等键，`body_page` 报 `(209, None, false, false)`，控制器超时为 `unrecognized_wait`。 [冷读 CSTL](F:/CS2-build-cache/playthrough/2026-11/20261120_neko_cold_retry_212356/load.events.log:1762) [终点诊断](F:/CS2-build-cache/playthrough/2026-11/20261120_neko_cold_retry_212356/load.host.log:6926) [旧槽重开回执](F:/CS2-build-cache/playthrough/2026-11/20261120_neko_cold_retry_212356/receipt.json)

同版 `sscript.kcs` 的普通 Load 先在 `713501` 调用 `_SAVE_GetScene`，经 `727629` 的 native 325 建立 scene/context 和 `body_loads` 条目。随后 `713555` 调用字符串恢复子程序，在 `727401` 用 native 253 读取 CSS 记录 `0x4`。实际槽里该记录载荷从解压偏移 `0x2aa40` 开始，含 16 个共享字符串，其中 `$str1000` 是 84 字节。当前 native 253 在解包后将整个 `CstTextOrigins` 重置，仍存活的 context 209 由此失去加载登记。f604 的 CSTL 重读因缺少此登记只记录编码，FES 正文收不到 r216 的 `CstBodySource`。 [恢复入口](D:/迅雷下载/CS2/simulator/crates/desktop/src/lib.rs:4899) [正文来源生成](D:/迅雷下载/CS2/simulator/crates/desktop/src/cst_context_native.rs:884)

归档补丁 `20261120_neko_cold_css_body_provenance.patch` 将 native 253 的重置改为清除字符串编码和正文来源缓存，以及前一阶段的读取观测。保留活 CST 加载 ID 和探针登记，让 f604 同一加载内的真实 CSTL 读取重新建立标签。回归用例覆盖同字节的旧字符串标签失效、活加载保留、新读取可重新发布。整套可移植快照替换 context 时仍走完整重置。

```powershell
$bodyRestoreFix = 'archive/2026-11/20261120_neko_cold_css_body_provenance.patch'
git apply --check --whitespace=error-all $bodyRestoreFix
if ($LASTEXITCODE -ne 0) { throw 'NEKO cold CST body provenance patch check failed' }
git apply --whitespace=error-all $bodyRestoreFix
# Inspect the short JP cold reader with the existing slot digest.
rg -n 'neko-slot-open|input=24 ordinal=2|Stop at restored 01/24/218|replay_status=' $readerEvents
rg -n 'absolute_record: 216|unrecognized_wait' $readerHost
# For a same-candidate writer and reader receipt, run the existing checker.
python -X utf8 -B $check roundtrip --receipt $roundtripReceipt --to 218
```

旧槽短验收要求日志出现同 context、同一加载代的 `group: 24, absolute_record: 216`，`exact_match=true`、`composited=true`，在 `Stop at restored 01/24/218` 零追加输入停住，槽文件读前后 SHA-256 一致。同版双语往返仍由合批候选分别写入自己的私档后验收。

### r337 新游戏观察补丁

`archive/2026-11/20261120_neko_r218_to_r337_probe.patch` 可供后续 PC 合批。它只扩展一个相邻等待和 g25 的 `1038` 诊断。按前额外校验当前合成的 r218 与游戏 CST 中相邻的 r337，之后沿用控制器的同窗守卫。遇到 `meswnd` 重建会停在边界，保留 `task-create` 供下一轮接续。检查器核对 r316 的来源帧晚于 r218 的实际物理消费、CST context/load 一致，且 r337 没有输入。背景和模型的实时绑定回执仍需单独验证。

```powershell
$r337Patch = 'archive/2026-11/20261120_neko_r218_to_r337_probe.patch'
git apply --check --whitespace=error-all $r337Patch
python -X utf8 -B $stageScript --print-plan --startup-probe to-337
# Stage with --startup-probe to-337 and the EXE built with the r337 patch.
# Launch its recorded argv and check the exited run.
python -X utf8 -B $check probe --run $run.run --language $language --to to-337
```

### r337 后首批连续正文等键

从 `scene.int::01.cst` 逐组核对 `0x02` 等键，并从同包 `01.cstl` 独立计算日文列 2、繁中列 0 的 UTF-8 长度和 SHA-256。`338..485` 间没有更换窗口或剧情选项命令。现在可以把 r337 后的路线拆成下列边界。每次输入都使用当前已完成合成的正文等键和实际接收者，帧数由游戏决定。

| 路段 | 等键和正文 | 运行判据 |
| --- | --- | --- |
| r337→r368 | `25/337` 正文 r316 → `26/368` 正文 r342 | 单按观测。r343–363 有多次 `wait mpl`，r364 是 `wait tag 01`。保留自然等待，在 r368 停止观察。 |
| r368→r410 | `27/381` r374 → `28/399` r384 → `29/405` r404 → `30/410` r409 | 先单验 r368→r381。r381 后到 r410 有连续短段候选，包含第二模型、位移动画和语音，逐页核对活输入及正文后再接下一键。 |
| r410→r481 | `31/461` r428 → `32/475` r467 → `33/481` r480 | r410→r461 含粒子和 `wait tag 01 50`，单次验证。再逐页走到 r481，探测正文接收者并停。 |
| **首个纯正文小批次** | **`33/481` → `34/483` r482 → `35/485` r484** | 两次物理点击各自等待前页释放与后页合成。两组之间只有正文和 `0x02`，r485 零输入。若活窗口重建，停下并保留 `task-create`。 |
| r485 后 | `36/497` r491 | 再次有模型命令、语音和 `wait tag 01`，另作单段。 |

逐页记录见 `scripts/analysis/pc_story_neko_r337_to_r485_evidence.json`，包括需要视为零字节的 CSTL 尾片段。r337 之后的组序、正文和命令分类证据来自资源，窗口寿命、活命中、物理消费及实际合成待运行验证。

### 从普通槽冷读后分段推进

`20261120_neko_r218_cold_to_r485.patch` 接在已合入 `&mut` 修复的普通档路线后。在加载确认、同语言、同槽摘要以及 r218 自然显示并合成通过后，`--to` 指定本次的终点。正文点击位于 `(625,478)`。每页确认屏幕命中启用、当前 `meswnd` 下唯一候选确为 `low button`，该位置没有命名 FES 操作。日志分别保留屏幕命中 region 和正文实际回执的 region。收齐按键回执及设备扫描释放后才接下一页，保持同一 CST context/load。点击与新正文可在同一游戏帧内生效。只有 r218→r337 允许一次有 `task-create` 证据的窗口重建，后续要求窗口持续存活。每个终点还探测活按钮，然后以零终点输入停止。

| 建议终点 | 从 r218 冷读后的正文点击总数 | 相对上一档新增 |
| --- | ---: | ---: |
| r218 | 0 | 0 |
| r337 | 1 | 1 |
| r368 | 2 | 1 |
| r381 | 3 | 1 |
| r410 | 6 | 3 |
| r461 | 7 | 1 |
| r481 | 9 | 2 |
| r485 | 11 | 2 |

两种语言各沿用自己的已退出生产进程和私有 `01.profile`。先验 `218,337,368,381`，可直接选 `410` 覆盖 r399、r405 两页，再用 `461,481,485` 分段验收。每个目标用独立冷读进程和独立 stdout/stderr 路径，输出前缀沿用生产进程。下列命令生成目标路线与调度参数。`$run`、`$language`、`$slot`、`$startup` 和 `$check` 沿用前面的写槽与重开步骤。

```powershell
$coldContinuation = 'archive/2026-11/20261120_neko_r218_cold_to_r485.patch'
git apply --check --whitespace=error-all $coldContinuation
if ($LASTEXITCODE -ne 0) { throw 'NEKO cold continuation patch check failed' }
# Integrate the patch and choose a target for this process.
$to = 337
$readerPlan = Join-Path $run.run "read-slot0-to-$to.route.json"
$json = python -X utf8 -B $check menu-route --language $language --operation load --startup $startup --slot-sha256 $slot.sha256 --to $to
if ($LASTEXITCODE -ne 0) { throw 'NEKO continuation route generation failed' }
$json | Set-Content -LiteralPath $readerPlan -Encoding ASCII
$readerArgv = @($run.argv_prefix) + @('25000', '--route-json', $readerPlan)
# Dispatch $readerArgv in a new PC process and record exit times and separate log paths in $roundtripReceipt.
python -X utf8 -B $check roundtrip --receipt $roundtripReceipt --to $to
```

验收要求 `status=pass`。`body_edges` 的长度等于表中的正文点击总数，每条边均核对自然完成时间、同次加载下的 CSTL 字节指纹及真实回执。`--to 337` 及以后要求批次的 `1038` 诊断覆盖相关组，并核对表中记录的零字节尾片段。厨房背景和模型的绑定证据另走已有的可视专项，正文分段命令使用默认检查选项。

## 21:52 同版双语普通档验收

21:41 诊断候选用旧日文槽独立冷读，f584 的 native 247 打开同一 SHA-256，f604 重新建立 context 209、load ID 1 下的 r216 来源，f699 的 owner 122 自然完成正文，f700 确认正文及提示已合成并停在 `01 24/218`。进程 PID 65760 正常退出，槽读前后均为 `09d5fd97bd139766e2e0df07a7d679a4d2822dcca2d9216f5ecf4d062e5dae0e`。这是旧槽兼容性实证。同版 JP/CN 新私档和 r485 等待本轮验收。 [事件](F:/CS2-build-cache/playthrough/2026-11/20261120_neko_cold_provenance_214524/load.events.log:3232) [正文来源](F:/CS2-build-cache/playthrough/2026-11/20261120_neko_cold_provenance_214524/load.host.log:24) [冷读回执](F:/CS2-build-cache/playthrough/2026-11/20261120_neko_cold_provenance_214524/receipt.json)

本轮固定使用 21:52 集成候选，源码 SHA-256 为 `987a6c0c1a0f8b24e7923e1b3a63b5a37f7dee6d0ea72b819334dfdc819b685d`，已包含 `20261120_neko_r218_cold_to_r485.patch` 的续段接线。当前 01 游戏目录整树 SHA-256 为 `3b32887e9e1e2007ae050b2a14c7a902b48d2156040b721b654b1054a7efa562`。准备阶段会重新计算一次资源摘要，整个批次沿用该资源目录。 [21:52 构建回执](D:/迅雷下载/CS2/archive/2026-11/20261120_215210_157211_story_replay_integration/build.json)

在工作区根目录执行。第一段仅准备两个独立 `01.profile`，可把输出的 `stage.json` 交给主线程的分槽终端。每一槽使用同一个 `stage.json`，分别选 `jp` 和 `cn`。

```powershell
$stageScript = 'scripts/analysis/stage_neko_cold_language.py'
$exe = (Resolve-Path -LiteralPath 'archive/2026-11/20261120_215210_157211_story_replay_integration/story_replay.exe').Path
$sha = 'd28fceeb4308a21611014f814ff94d7dfe96ba9a2d05d772d96599b8bf75525f'
$stagePath = python -X utf8 -B $stageScript --player $exe --sha256 $sha --startup-probe save-entry
if ($LASTEXITCODE -ne 0) { throw 'NEKO staging failed' }
$stage = Get-Content -LiteralPath $stagePath -Encoding UTF8 -Raw | ConvertFrom-Json
$game = $stage.runs.jp.argv_prefix[1]
$resourceSha = @'
import sys
from pathlib import Path
sys.path.insert(0, "scripts/analysis")
from run_playthrough_batch import resource_tree_sha256
print(resource_tree_sha256(Path(sys.argv[1])))
'@ | python -X utf8 -B - $game
if ($LASTEXITCODE -ne 0 -or $resourceSha -cnotmatch '^[0-9a-f]{64}$') { throw 'NEKO resource hash failed' }
$resourceSha | Set-Content -LiteralPath (Join-Path $stage.run_root 'resource.sha256') -Encoding ASCII
$stagePath
```

主线程在分配的 PC 槽执行下面这段，每种语言单独运行一次。若换终端，先将 `$stagePath` 指向准备阶段输出。默认按已保留语言设定的 `title` 启动，若已观察到语言选择等待，则用 `language-select`。同语言先写新槽，r218 短冷读检查通过后再用该槽独立冷读至 r485。失败时保留该轮日志和私档，按第一处失败状态核对。

```powershell
$language = 'jp' # Set to cn in the other replay slot.
$startup = 'title'
$check = 'scripts/analysis/check_neko_save_roundtrip.py'
$stage = Get-Content -LiteralPath $stagePath -Encoding UTF8 -Raw | ConvertFrom-Json
$resourceSha = (Get-Content -LiteralPath (Join-Path $stage.run_root 'resource.sha256') -Raw).Trim()
if ($resourceSha -cnotmatch '^[0-9a-f]{64}$') { throw 'Invalid staged resource hash' }
$run = $stage.runs.$language
$prefix = $run.argv_prefix[2]
$slotPath = Join-Path "${prefix}.profile" 'save0000.dat'
$env:CS2_MESSAGE_REVEAL_TRACE = $stage.environment.CS2_MESSAGE_REVEAL_TRACE

function Get-NekoSlot {
    if (Test-Path -LiteralPath $slotPath) {
        $json = python -X utf8 -B $check slot $slotPath
        if ($LASTEXITCODE -ne 0) { throw "Incomplete CSS at $slotPath" }
        $json | ConvertFrom-Json
    }
}

function Invoke-NekoPass([string]$tag, [string[]]$planArgs, $slotBefore) {
    $plan = Join-Path $run.run "$tag.route.json"
    $events = Join-Path $run.run "$tag.events.log"
    $hostLog = Join-Path $run.run "$tag.host.log"
    if ((Test-Path -LiteralPath $events) -or (Test-Path -LiteralPath $hostLog)) {
        throw "Existing replay logs for $tag"
    }
    $json = python -X utf8 -B $check menu-route @planArgs
    if ($LASTEXITCODE -ne 0) { throw "Route generation failed for $tag" }
    $json | Set-Content -LiteralPath $plan -Encoding ASCII
    $binary = $run.argv_prefix[0]
    $binarySha = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($binarySha -cne $stage.player.sha256) { throw 'Archived EXE identity changed' }
    $arguments = @($run.argv_prefix[1], $prefix, '25000', '--route-json', $plan)
    $encoded = ($arguments | ForEach-Object { '"' + $_ + '"' }) -join ' '
    $proc = Start-Process -FilePath $binary -ArgumentList $encoded -WindowStyle Hidden -PassThru -Wait `
        -RedirectStandardOutput $events -RedirectStandardError $hostLog -ErrorAction Stop
    [ordered]@{
        pid = $proc.Id
        start_ms = ([DateTimeOffset]$proc.StartTime).ToUnixTimeMilliseconds()
        exit_ms = ([DateTimeOffset]$proc.ExitTime).ToUnixTimeMilliseconds()
        exit_code = $proc.ExitCode
        output_prefix = $prefix
        sha256 = $binarySha
        source_tree_sha256 = $stage.player.source_tree_sha256
        resource_tree_sha256 = $resourceSha
        events = $events
        host = $hostLog
        slot_before = $slotBefore
        slot_after = Get-NekoSlot
    }
}

if (Test-Path -LiteralPath $slotPath) { throw "Writer requires an empty slot at $slotPath" }
$writer = Invoke-NekoPass 'write-slot0' @('--language', $language, '--operation', 'save') $null
if ($writer.exit_code -ne 0 -or $null -eq $writer.slot_after -or
    -not (Select-String -LiteralPath $writer.events -Pattern '^replay_status=completed$' -Quiet)) {
    throw "Writer failed. See $($writer.events) and $($writer.host)"
}

foreach ($to in @(218, 485)) {
    $before = Get-NekoSlot
    if ($null -eq $before -or $before.sha256 -cne $writer.slot_after.sha256 -or
        $before.size -ne $writer.slot_after.size) {
        throw 'Ordinary slot changed after writer exit'
    }
    $reader = Invoke-NekoPass "read-slot0-to-$to" @('--language', $language, '--operation', 'load',
        '--startup', $startup, '--slot-sha256', $before.sha256, '--to', "$to") $before
    $receipt = Join-Path $run.run "roundtrip-r$to.json"
    [ordered]@{
        schema = 'cs2-neko-save-roundtrip/v1'
        language = $language
        save_confirm = $false
        reopen_startup = $startup
        output_prefix = $prefix
        producer = $writer
        consumer = $reader
    } | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $receipt -Encoding UTF8
    $verification = python -X utf8 -B $check roundtrip --receipt $receipt --to $to
    $verdict = $LASTEXITCODE
    $verification | Set-Content -LiteralPath (Join-Path $run.run "roundtrip-r$to.check.json") -Encoding UTF8
    $verification
    if ($verdict -ne 0) { throw "Roundtrip r$to failed. See $receipt" }
}
```

每种语言应各有 `roundtrip-r218.check.json`、`roundtrip-r485.check.json`，四份均须 `status=pass`。检查器要求生产和消费的 PID 不同且时间有序，构建、资源和私档身份匹配，消费侧 native 247 打开与写后同摘要的槽 0，读前后 CSS 大小和摘要一致。r218 必须自然显示同语言 CSTL 的 r216 并零输入停在 `Stop at restored 01/24/218`。r485 在三次 Load 菜单按键之外须有 **11 次正文物理左键**，逐页验 CSTL 来源、活接收者消费和扫描释放，最终在 `Stop at 01/35/485 without input` 零输入停止。每进程有独立 events/host 日志，单看退出码无法辨别检查结果。

## r485 后的自然剧情路线

离线核对 `scene.int::01.cst` SHA-256 `47ea1f3d6ba0de16736f5f574bda9297544d220d4998dd3061bb96689ad1c37e` 和 `01.cstl` SHA-256 `8085d37fd751947577579ff0ac6eea7a58f5f6863e5df0f4d9561666a6065ae6`。CSTL 列序为 `cn,en,jp`，下表长度为日文／繁中的 UTF-8 字节数。每组末尾恰有一个 `0x02` 等键。r497 及以后是资源候选，21:52 回放控制器与来源诊断覆盖至 r485。首验从已确认的上游等键只发一键，保留场景命令和真实时间，若活 owner 或接收者变化则停下记录。每页重新确认当帧 `WAIT_KEY`、CSTL 来源与自然合成，输入后收齐原接收者消费和物理释放。

| 上游 → 后继等键 | 正文记录/组内序号，JP/CN 字节 | 零字节尾记录/组内序号 | 自然段与首验 |
| --- | --- | --- | --- |
| r485 → `36/497` | `491/5`，54/39 | `496/10` | 单按。8 条命令，含 `pcm A_01_009`、`wait tag 01`。 |
| r497 → `37/547` | `502/4`，120/87 | `546/48` | 单按。46 条命令，含模型移位、淡变和 `call sub_getattm2`。 |
| r547 → `38/569` | `558/10`，60/105 | `568/20` | 单按。18 条命令，含 `rdraw`、`pcm B_01_008` 和三次 `wait mpl`。 |
| r569 → `39/583` | `575/5`，66/57 | `582/12` | 单按。10 条命令，含模型、`pcm B_01_009` 和三次 `wait mpl`。 |
| r583 → `40/586` | `585/1`，42/48 | — | 只有说话人和正文。r583 首验通过后可接在上一段末尾，仍逐页守卫。 |
| r586 → `41/594` | `593/6`，75/84 | — | 5 条模型命令。r594 的实际等待确认后可接下一页。 |
| r594 → `42/597` | `596/1`，24/36 | — | 只有说话人和正文。r586→r594→r597 可组成两页小批次。 |
| r597 → `43/627` | `603/5`，51/36 ＋ `604/6`，41/47 | `626/28` | 双正文、双语音、模型动作和 `wait tag 01 50`。单独观察，核对两条来源的顺序和最终正文的自然提交。当前检查器按单个非空正文验证，接线前需扩展。 |
| r627 → `44/647` | `638/10`，87/72 | `646/18` | 单按。含 `rwipe wipe04a`、语音和模型切换。 |
| r647 → `45/650` | `649/1`，45/36 | — | 只有说话人和正文。 |
| r650 → `46/653` | `652/1`，60/63 | — | 只有说话人和正文。r647→r650→r653 是下一处两条边都无场景命令的小批次。 |

## 21:56:30 双语普通档与 r337 首错

日文 PID 62328 在 f3359 写槽，f3362 正常完成。繁中 PID 56636 在 f2904 写槽，f2907 正常完成。独立读进程 PID 73096 和 51540 都在 f584 经 native 247 打开各自 SHA-256 一致的槽，读前后分别为 36378、36376 字节，摘要不变。日文 f700、繁中 f697 的 r216 已自然完成并合成，context 209、load ID 1、owner 122。两轮发出的首个正文左键都由 region 139 → owner 122 收到，region 77 → task 8 随后的 key 2 消费使 VM 同帧恢复。r316 的实际 CSTL 读取、r336 的零字节尾均已发生。 [双语回执](D:/迅雷下载/CS2/archive/2026-11/20261120_neko_215630_cold_r337_receipt.json)

日文 f771、繁中 f768 都执行 g25 的 r251 `rdraw 120` 与 r252 `wait`，随后没有 r259 `initialize_header`。从输入帧后持续出现 `help disp` 批次。超时分别为 f2500、f2497，活窗仍是 `MES_SETSTATE`，r316 的正文代次一直没有自然完成或提交，投影关闭。两轮读进程返回 0，但状态为 `unrecognized_wait`。旧检查器要求完整终点才汇总菜单回执，故整轮报告为 `insufficient`。归档回执单独核对了三个 Load 菜单键、实际 native 247 打开、r218 与首个正文按键。 [日文转场](F:/CS2-build-cache/playthrough/2026-11/20261120_215630_034330_csyfr82s_neko_language_stage/jp/load.events.log:4125) [繁中超时](F:/CS2-build-cache/playthrough/2026-11/20261120_215630_034330_csyfr82s_neko_language_stage/cn/load.host.log:33)

`kcs.int::sscript.kcs` 的代码偏移 `0x2b09` 调用 native 630，入参是 `MES_SETSTATE`、空前缀、bank 0。在 `fes.int::meswnd.fes` 的解压文本中，1373 行调用 `SETBUTTON`，668–687 行依次调用 `_SETBUTTON`，712／727 行包含日志反复出现的 `help disp`。通用 System 通知可在内层块尾提前结束，遗留的调用回址随后使普通帧的执行流落回通知块。现有 `advance_system_notification` 支持在 `SETBUTTON` 和 `_SETBUTTON` 上先退回通知的调用深度。本轮增量复用这条路径，限制为已验证 PE、启动脚本、FES 内容摘要及空上下文的 `343 / bank 0 / MES_SETSTATE`。缓存不进存档，恢复的 driver 用实际胜出的 FES 和当前解释器程序重新验证来源。适用到 22:05 集成候选所用的这两个 FES 文件版本。 [归档增量](D:/迅雷下载/CS2/archive/2026-11/20261120_neko_cold_mes_setstate_return.patch)

```powershell
$patch = 'archive/2026-11/20261120_neko_cold_mes_setstate_return.patch'
git apply --check --whitespace=error-all $patch
if ($LASTEXITCODE -ne 0) { throw 'NEKO FES return patch check failed' }
git apply --stat $patch
```

合入后的归档候选使用上节双语命令，更换 `$exe`、`$sha`，首轮目标改为 `@(337)`。如需逐项追溯转场，在 `$arguments` 末尾加入 `'trace-native:630', 'trace-native:450'`。同版各写一次新私档，独立进程复验 `--to 337` 后再跑 `--to 485`。r337 通过须同时看到 r252 的等待自然结束、r259 开始后续初始化、r316 来源对应的正文自然完成并合成、`01 25/337` 活接收者探针，以及 `Stop at 01/25/337 without input`。检查器要求 `status=pass` 且 `body_edges` 恰有一条。若 `help disp` 循环消失而转场仍停在 r252，保留 rdraw 对象生命周期，并用 `trace-native:527`、`trace-native:528`、`trace-native:520` 区分配置、起动和轮询。原 `sscript.kcs` 的直接调用分别见 `0xb82b3`、`0xb8352`、`0xb83b8`。

## 22:22 双语冷读 r252 与 527 ABI

22:22 同版双语均写入新普通槽，并由不同 PID 独立打开同一槽。日文 f771、繁中 f768 在 `rdraw 120` 后创建 kind 38 的 task 54，马上进入裸 `wait`。终点活窗已回到 `MES_SHOW`，正文仍未自然提交。两边均未到 r259 `initialize_header`。旧批次未采集 527、528、520 参数或查询值。 [PE、KCS 和双语日志回执](D:/迅雷下载/CS2/archive/2026-11/20261120_neko_r252_rdraw_receipt.json)

原版 PE 2.6.1.76 的 SHA-256 前缀为 `c71c5b85`，实际注册表 RVA 为 `0x62dd38`。527 的 VA `0x642e40` 把入参 `+8/+12/+4` 依次写到记录 `+0/+4/+8`，发消息 `0x180`。本作 KCS 的 `0xb82b3` 推入 `[id, duration, mode, depth]`。528 的 VA `0x642eb0` 原样转发 `[id, enable]`，KCS `0xb8352` 的 `enable=1`。520 的 VA `0x642fc0` 用 `[id]` 查询 `0x179`，把接收者返回值写回 VM，KCS `0xb83b8` 随后依据该值决定等待分支。当前 528 与 520 的分派对应原版，527 的 `duration_first_draw_profile` 缺本作 PE。主补丁只增加该 PE 的 527 重排和窄测例，诊断补丁额外发出各索引的同对象实际状态。 [最小修复](D:/迅雷下载/CS2/archive/2026-11/20261120_neko_rdraw_527_abi.patch) · [可叠加诊断](D:/迅雷下载/CS2/archive/2026-11/20261120_neko_rdraw_527_trace.patch)

```powershell
$fix = 'archive/2026-11/20261120_neko_rdraw_527_abi.patch'
$trace = 'archive/2026-11/20261120_neko_rdraw_527_trace.patch'
git apply --check --whitespace=error-all $fix $trace
# 主线程在用于诊断构建时执行 git apply $fix $trace
# 在 Invoke-NekoPass 的 --route-json $plan 之后追加以下四项
$arguments += @('story-events', 'trace-native:527', 'trace-native:528', 'trace-native:520')
# 按本文件同版双语命令重新写私有槽，将循环目标改为 @(337, 485)
rg -n 'neko-rdraw-native|native-call index=(520|527|528) |\[command\] (rdraw 120|wait|initialize_header)|Stop at 01/|^replay_status=' $reader.events
```

- 紧接 r251 的创建按实际 ID 关联 527、528、520。527 的原始 16 字节读取为四个小端 DWORD，按 `[id,duration,mode_bits,depth_bits]` 验。22:48 批实际绘制时长为 118 tick，深度为 45000。
- 528 的第二个 DWORD 为 1。同对象起动后 `active=1` 且 `end-started` 等于 527 的实测时长。日文 task 54 在 f772 起动，end=890，f891 查询返回 0。繁中对应 f769、887、888。日文 f984、繁中 f981 执行 r259 `initialize_header`。 [日文](F:/CS2-build-cache/playthrough/2026-11/20261120_224851_911340_vpeukqzj_neko_language_stage/jp/load.events.log:8502) [繁中](F:/CS2-build-cache/playthrough/2026-11/20261120_224851_911340_vpeukqzj_neko_language_stage/cn/load.events.log:8451)
- 日繁各自同版写档并冷读。两个 `roundtrip --to 337` 均须通过，终点在 `01/25/337` 零输入，随后复验 `roundtrip --to 485`。已验证补丁应用和 Rust 语法，时钟与后继命令等实跑回执待主线程补齐。

## 22:48 冷读 r461 来源失效

日文 f1535 确认 r410，f1536 由正文按钮 region 29 → owner 12 收到，根按钮 region 77 → task 8 消费，VM 同帧恢复。随后在 handle 209 装入 CSTL。r428（g31/ordinal 17）的日文 117 字节进入正文队列，r460（ordinal 49）返回 0 字节。同帧随后另两次装入同摘要的 `scene.int::01.cstl`，日志之间有 r413 `call sub_getattm2`。繁中在 f1399 读取了 r428 的 93 字节及 r460 的零字节尾。 [日文实际读取](F:/CS2-build-cache/playthrough/2026-11/20261120_224851_911340_vpeukqzj_neko_language_stage/jp/load.events.log:19678) [繁中实际读取](F:/CS2-build-cache/playthrough/2026-11/20261120_224851_911340_vpeukqzj_neko_language_stage/cn/load.events.log:17358)

日文 f1649 在 r456–r460 执行音效、立绘与空字串后进入 `[input]`。f3335 诊断确认 owner 12 为 `MES_SHOW/WAIT_KEY=true`，VM 等在 native 71 的 `0xd6d`，epoch 26 在 f1670 自然完成并提交，剩余计数为零，投影可见，`body_page.sources=None`。`playback=134/first_pending=None` 是已完成的揭示范围。r461 页仍应以 r428 为正文来源，r460 只提供零字节尾据。 [活等待和来源诊断](F:/CS2-build-cache/playthrough/2026-11/20261120_224851_911340_vpeukqzj_neko_language_stage/jp/load.host.log:11188)

native 325 每次成功装载都分配新的正文来源 ID。已排入的 r428 携带早先的 ID，`body_is_current()` 因此拒绝该来源。归档增量以本作 PE、原 `sscript.kcs` 代码摘要及 `01.cst`／CSTL 的实际摘要限定相同场景的脚本重读。原脚本在 `0x14459` 重装并恢复原位置，在 `0x14e86` 和 `0x14f8c` 装载子脚本，在 `0x1516d` 恢复保存的场景名及位置。补丁给已核对的 `sub_getattm2.cst` 临时分配独立 ID，在同帧返回调用恢复外层 ID。补丁的 `pc` 字段记录 native 指令之后的 VM 地址，供主线程复跑核对。 [归档增量](D:/迅雷下载/CS2/archive/2026-11/20261120_neko_r461_cst_reload_provenance.patch)

```powershell
$patch = 'archive/2026-11/20261120_neko_r461_cst_reload_provenance.patch'
git apply --check --whitespace=error-all $patch
# 主线程应用并构建后，沿用同版双语写档和冷读 --to 485
rg -n 'neko-cst-(origin|helper|reload)|neko-cstl-read.*input=31.*destination=0x00062c6b|01/31/461|01/32/475|Stop at 01/35/485|^replay_status=' $reader.events
```

r461 的 host 探针应见 `absolute_record: 428`、同代自然完成、`exact_match=true/composited=true`、正文与根按钮活绑定。此时才发一次物理键，收齐根消费和同帧 VM 恢复后进入 r475。最终在 r485 零输入。离线检查器已对 r337、r461 限定读取窗口，要求上一等键之后的正确 1038 摘要及 FES 队列证据，并允许同帧重复的完全相同 CSTL 装载回执。旧批次复查已消除 r337 的读取帧误报。JP f3335、CN f3198 均仍为 `unrecognized_wait`，缺少 r485 终点。 [繁中活等待和来源诊断](F:/CS2-build-cache/playthrough/2026-11/20261120_224851_911340_vpeukqzj_neko_language_stage/cn/load.host.log:10259)

## r485 后连续守卫

普通槽路线可从 `01/35/485` 自动继续。首个长目标是 `01/95/1398`，有 60 次受守卫翻页。片头前上界是 `01/286/3631`，有 251 次，下一段的 `movie nekopara3op` 留给影片转场验收。两个目标均已从实际日繁 CST/CSTL 生成。g43/r627 首次检查同代正文 r603、r604 与零字节尾 r626。 [独立路线增量](D:/迅雷下载/CS2/archive/2026-11/20261120_neko_cstl_continuation.patch) · [生成和检查脚本](D:/迅雷下载/CS2/scripts/analysis/neko_cstl_continuation.py)

```powershell
$patch = 'archive/2026-11/20261120_neko_cstl_continuation.patch'
git apply --check --whitespace=error-all $patch
# 这些变量取普通档同版批次的实际路径
$helper = 'scripts/analysis/neko_cstl_continuation.py'
$to = 1398
$startup = 'title'
$route = Join-Path $run.run "load-r$to.route.json"
$slot = Join-Path $run.run '01.profile/save0000.dat'
$routeJson = python -X utf8 -B $helper route --language $language --startup $startup --slot $slot --to $to
if ($LASTEXITCODE -ne 0) { throw 'NEKO CSTL route generation failed' }
$routeJson | Set-Content -LiteralPath $route -Encoding ASCII
$events = Join-Path $run.run "load-r$to.events.log"
$hostLog = Join-Path $run.run "load-r$to.host.log"
$arguments = @($run.argv_prefix[1], $run.argv_prefix[2], '40000', '--route-json', $route, 'story-events')
# 主线程用同一私档前缀调度独立读进程，沿用上节回执和日志重定向
python -X utf8 -B $helper check --route $route --events $events --host $hostLog
```

守卫从上一键真实回执帧计算本页读取窗口，核对 CST 与 CSTL 装载、主地址 `0x00062c6b` 的 1038 读取和有序正文记录。键前要求 `MES_SHOW/WAIT_KEY`、同代自然完成提交、正文及提示实际合成和活按钮。键后要求正文按钮 key 0 回执、活根接收者独占 key 2、VM 同帧一次恢复及扫描释放，再等待同一 context/load/window 的新正文代次。终点也验全部页条件并保持零输入。失败保留 host 的合成诊断、候选控件、消费回执、截图以及 events 的最近 KCS 命令，检查器回传最后通过的页。待主线程编译和验证运行时。

### 00:42 日文 r497 到 r547 的加载帧冲突

00:42 批日文读进程的 EXE SHA-256 为 `0bee1a3abe863d338d73ad7346c2f9c61664d87d6f48449bb5d6060ffdbdb0a1`。f2021 计划翻 r497，f2022 的实际按键由正文 region 29 → owner 12 接收，根 region 77 → task 8 消费，同帧 VM 恢复。context 213 从 f623 首次加载起使用 `load_id=1`。f2022 读取 r502 的日文列 2，共 120 字节，r546 是零字节控制尾。f2022 一次、f2091 两次嵌套 helper 返回均记录 `preserved=true`。 [首次加载](F:/CS2-build-cache/playthrough/2026-11/20261121_004238_340093_2y9wpu6j_neko_language_stage/jp/load.events.log:1730) · [物理消费](F:/CS2-build-cache/playthrough/2026-11/20261121_004238_340093_2y9wpu6j_neko_language_stage/jp/load.host.log:6571) · [f2091 返回](F:/CS2-build-cache/playthrough/2026-11/20261121_004238_340093_2y9wpu6j_neko_language_stage/jp/load.events.log:28807)

r547 等键标记为 f2131，f2132 报 `active`。f3821 以 `unrecognized_wait` 结束。停点同时确认 `MES_SHOW/WAIT_KEY`、精确来源 `[502]`、正文 epoch 50 于 f2022 取得来源且在 f2159 自然完成、`committed_epoch=50`。场景追踪把 f2091 的同代返回写进 `loaded`，使 `committed_body_at` 的 `loaded <= source_frame` 失败，实际像素合成检查尚未执行。 [终点诊断](F:/CS2-build-cache/playthrough/2026-11/20261121_004238_340093_2y9wpu6j_neko_language_stage/jp/load.host.log:13981)

归档补丁让已验证的 NEKO 01 reload 同时保留 load ID 与首次加载帧。每次重读仍重新登记等待标记。运行时 CSTL 守卫按 context 和加载事件帧取两份资源摘要，早期回执可与 f2022 的正文来源关联。自然显完、像素合成、活按钮和同帧消费均沿用现有条件。 [r547 增量](D:/迅雷下载/CS2/archive/2026-11/20261121_neko_r547_retained_load_frame.patch)

```powershell
$patch = 'D:/迅雷下载/CS2/archive/2026-11/20261121_neko_r547_retained_load_frame.patch'
git apply --check --whitespace=error-all $patch
# 主线程合入并构建后，$exe 取新候选的绝对路径。
$producer = 'F:/CS2-build-cache/playthrough/2026-11/20261121_000313_563719__363i_cn_neko_language_stage'
python -X utf8 -B 'D:/迅雷下载/CS2/scripts/analysis/run_neko_save_roundtrip.py' --player $exe --resume-from $producer --to 547
```

两种语言各应有 r485、r497、r547 三张 `neko-cstl-page`，前两页各一次物理消费及释放，r547 以 `Stop at 01/37/547 without input` 完成。r547 核对 `loaded <= earliest <= source_frame`，CSTL r502 为日文 120／繁中 87 字节，r546 为零字节。主线程取得两份终点回执后，可把目标改为 `--to 569`，由现有守卫在 r547 确认合成和活按钮后发唯一一键，待三次原有 `wait mpl` 自然结束后验后继。r547 的修复运行及 r569 尚待主线程复验。

### 01:10 r1398 普通槽检查点

01:05 固定源码候选的日文运行在 f2160 通过 `01/37/547`，CST 首次加载 f623，正文来源 f2022，自然显完 f2159。f2229 通过 `01/38/569`，f3327 通过 `01/52/723`。到 f4634 的 `01/65/979` 仍使用同一 context/load 和活 `meswnd`。 [01:10 日文日志](F:/CS2-build-cache/playthrough/2026-11/20261121_011006_370171_wfn8zkyg_neko_language_stage/jp/load.events.log:29982)

r1398 的锚点为 `01/95/1398`，r1385 为正文，r1397 为零字节尾，日文列 2 为 99／0 字节，繁中列 0 为 75／0 字节。下一普通等键为 `01/96/1401`。原 `saveload.fes` 901–928、1100–1124 行确认槽号、已有槽覆盖确认、`datasave` 和时间字段。增量选择将已验 r218 槽 0 复制到独立私档，再通过原版覆盖流程写 r1398。 [普通槽增量](D:/迅雷下载/CS2/archive/2026-11/20261121_neko_scene01_ordinary_checkpoints.patch)

主线程应用补丁后，用现有 staging 建新私档。下列 `$run` 取新候选 staging 的同语种记录，`$slot` 指向该私档中的 r218 副本，`$startup` 取实测的 `title` 或 `language-select`。

```powershell
$patch = 'D:/迅雷下载/CS2/archive/2026-11/20261121_neko_scene01_ordinary_checkpoints.patch'
git apply --check --whitespace=error-all $patch
$helper = 'D:/迅雷下载/CS2/scripts/analysis/neko_cstl_continuation.py'
$slot = Join-Path $run.run '01.profile/save0000.dat'
$writerRoute = Join-Path $run.run 'save-r1398.route.json'
$readerRoute = Join-Path $run.run 'load-r1398-to-r1401.route.json'

$json = python -X utf8 -B $helper route --language $language --startup $startup --slot $slot --to 1398 --save-after
if ($LASTEXITCODE -ne 0) { throw 'NEKO writer route failed' }
$json | Set-Content -LiteralPath $writerRoute -Encoding ASCII
$writerArgv = @($run.argv_prefix[1], $run.argv_prefix[2], '40000', '--route-json', $writerRoute, 'story-events')
# 主线程调度 writer 并记录 PID、起止时间、EXE、$writerEvents 和 $writerHost。
python -X utf8 -B $helper check --route $writerRoute --events $writerEvents --host $writerHost --slot $slot
if ($LASTEXITCODE -ne 0) { throw 'NEKO checkpoint write proof failed' }

$json = python -X utf8 -B $helper route --language $language --startup $startup --slot $slot --resume-at 1398 --to 1401
if ($LASTEXITCODE -ne 0) { throw 'NEKO reader route failed' }
$json | Set-Content -LiteralPath $readerRoute -Encoding ASCII
$readerArgv = @($run.argv_prefix[1], $run.argv_prefix[2], '40000', '--route-json', $readerRoute, 'story-events')
# 主线程在 writer 正常退出后用同 EXE 和私档调度 reader，并记录新的 PID 与日志。
python -X utf8 -B $helper check --route $readerRoute --events $readerEvents --host $readerHost --slot $slot
```

写档端须在 r1398 自然合成后依次验证活 `meswnd btn[4]`、已占用 `btdata[0]`、`confirm2 btn[0]`。唯一 `fes-datasave slot=0 ... written=true` 后等待面板回到 `WAIT`，记录 `neko-checkpoint-written`，用退出后完整 CSS 的长度和 SHA-256 验收。冷读端要求新 PID、同语言，实际开档摘要与写档回执一致。先证明 r1398 已自然合成并恢复活等键，再以唯一正文消费和释放推进一页，停在 r1401。通过后沿用 `--resume-at 1398`，将终点扩到 r1867 或 r3631。现有只读长跑退出后仍须用新候选执行一次写档路线，才能产生 r1398 普通槽。


### 01:36 归档槽自动写入与重开

主线程可用 `--save-at` 直接串联两个进程。`--resume-from` 指向含 `stage.json` 的已验双语批次。运行器按 `--language` 校验并复制相应语言的 r218 槽，写入 r1398 后先验收菜单、写入和 CSS 摘要。通过后用同版候选、同一私档启动新进程，核对 r1398 开档及一页消费，停在 r1401。

```powershell
$exe = 'F:/CS2-build-cache/archive/2026-11/20261121_013649_736495_story_replay_integration/story_replay.exe'
$seed = 'F:/CS2-build-cache/playthrough/2026-11/20261121_000313_563719__363i_cn_neko_language_stage'
python -X utf8 -B 'D:/迅雷下载/CS2/scripts/analysis/run_neko_save_roundtrip.py' --player $exe --language jp --resume-from $seed --save-at 1398 --to 1401
```

每种语言的输出目录包含 `save-checkpoint.*`、`load-checkpoint.*` 和 `scene01_checkpoint_roundtrip.json`。写档证据不足时回执保留首错并停止该语言的冷读。写档通过后用独立进程的 PID、启动和退出时间、同槽 CSS 长度与摘要及 `guarded_edges` 验收。繁中复验将 `--language jp` 改为 `--language cn`。

### 2026-11-21 普通槽推进与首处分支

日文已有一条正常新游戏、普通槽 0 写入、独立进程重开并到 `01/35/485` 的完整回执。此前 `01/95/1398` 的只读接续已验 61 页、60 次物理消费。f7558 的 Save 按钮按键由 `meswnd` 消费，f7561 创建 `saveload`，但当前面板尚未登记时旧回放器先检查了依赖该登记的槽位，最后在实际 `saveload/WAIT` 超时。回放器现先记录已就绪的活面板 owner，再核对占用槽、覆盖确认和实际 CSS 写入。 [新游戏与冷读 r485](F:/CS2-build-cache/playthrough/2026-11/20261121_075603_629974_7959j8vh_neko_language_stage/jp/check.json) · [只读 r1398](F:/CS2-build-cache/playthrough/2026-11/20261121_011006_370171_wfn8zkyg_neko_language_stage/jp/check.json) · [旧写档回执](F:/CS2-build-cache/playthrough/2026-11/20261121_022924_978020_ugqcq83j_neko_language_stage/jp/scene01_checkpoint_roundtrip.json)

同一 08:08 候选的通用队列分别通过日文和繁中冷启动确认、正常 New Game 及两次物理翻页，终点为 `01/2/92`。从 `scene.int::01.cstl` 按实际赢家验证三页前的姓名、正文 UTF-8 字节和 SHA-256。日文列 2 的姓名各 12 字节，正文 69、90、63 字节。繁中列 0 的姓名各 9 字节，正文 30、54、33 字节。两语种合计六页均有同代正文自然完成、正文与等待提示合成回执。 [日文队列](F:/CS2-build-cache/playthrough/2026-11/20261121_081253_626646_r4tnihou_playthrough/01.json) · [繁中队列](F:/CS2-build-cache/playthrough/2026-11/20261121_081403_408242_vt_lqiar_playthrough/01.json)

`--resume-from` 也可接收已完成普通槽写入和冷读的后续批次。每次使用新私档，仅取已核验摘要的普通槽 0。随后先核对原页合成，再按游戏输入推进。08:08 同版候选先写入 `01/95/1398`，独立进程冷读并翻十页到 `01/105/1467`。接续批从 r1398 翻 48 页到 `01/143/1867` 覆盖存档，独立冷读并翻至 `01/144/1871`。下一段可使用 r1867 隔离普通槽推进至 r2616。 [日文 r1398 写入和冷读](F:/CS2-build-cache/playthrough/2026-11/20261121_080958_414255_slqw5vxp_neko_language_stage/jp/scene01_checkpoint_roundtrip.json) · [日文 r1867 写入和冷读](F:/CS2-build-cache/playthrough/2026-11/20261121_090051_249945_7y_o025p_neko_language_stage/jp/scene01_checkpoint_roundtrip.json)

`--to 5386 --follow-branch-pages 3` 使用游戏当前 `#1003/#1004` 决定路线。核对实际分支资源和摘要后，接续 `01_02` 的 r24、r49、r63，或 `H01` 的 r51、r55、r59，各推进两页。两个语种的资源计划已验证，实际运行仍待到达首处分支。片尾及通关状态仍按后文的独立证据验收。

`--to 5386 --follow-scene02-pages 2` 从实际首处分支续跑。若先到 `H01`，核对 `H01/364/1984 → 01_01.cst → 01_02/0/24`。两条路径再于 `01_02/47/347` 正常翻页，核对 `scene.int::02.cst` 和 `02/0/85`，然后进入 `02` 第二个普通页。每次过章都核对实际装载顺序、摘要、新 load ID、输入回执和可翻页后继。章节 02 的第一个等键之前有与章节 01 相同的助手调用。CST 和 CSTL 均须命中固定摘要，且返回到同一主 CST、吻合指令地址和同帧顺序，方可保留首次载入帧。该路线待候选运行验证。

章节 `02/509/r5167` 是原始 CST 空槽。实际 CSTL 日文列追加 59 字节，繁中列追加 50 字节。状态守卫按实际选中列核对所有提交贡献，在 `02/509/5168` 等待正文合成时要求同时存在 r5158 和 r5167。日文、繁中的离线生成各通过 `H01` 365 页、`01_02` 48 页、`02` 512 页。

后续检查点可逐批使用 `--save-at <写档> --to <冷读后继>`，每批从上一回执的目录传入 `--resume-from`。日繁资源都核对了下表的紧邻等键。`3631 → 3697` 的实际回放须验片头影片自然结束，之后可单独在 r3697 写普通档。

| 写档等键 | 冷读后继 |
| --- | --- |
| `01/143/1867` | `01/144/1871` |
| `01/191/2616` | `01/192/2628` |
| `01/239/3153` | `01/240/3155` |
| `01/286/3631` | `01/287/3697` |
| `01/287/3697` | `01/288/3713` |
| `01/335/4178` | `01/336/4180` |
| `01/383/4560` | `01/384/4564` |
| `01/431/5044` | `01/432/5046` |
| `01/479/5328` | `01/480/5330` |

日文 r2616、r3153 的普通槽均已独立重开并验证后继 `01/192/2628`、`01/240/3155`。下一段使用 r3153 隔离普通槽推进到片头前写档，冷读后自然播放片头到 r3697。 [日文 r2616 写入和冷读](F:/CS2-build-cache/playthrough/2026-11/20261121_100224_795355_m5iafgzp_neko_language_stage/jp/scene01_checkpoint_roundtrip.json) · [日文 r3153 写入和冷读](F:/CS2-build-cache/playthrough/2026-11/20261121_102741_864043_rpv8q_f__neko_language_stage/jp/scene01_checkpoint_roundtrip.json)

## 片头衔接

`01/286/3631` 后用一次守卫按键进入同一 CST 的 g287。r3632–r3667 先执行淡出、关窗、绘制与等待，r3668 写 `#617=1`，r3669 请求 `movie.int::nekopara3op.wmv`。这条边须用同一实际影片 handle 核对开始、画面呈现和自然终止，播放期间零输入。r3670–r3688 自然执行后续等待及转场，r3691 的主 CSTL 正文为日文 51／繁中 33 字节，r3696 是零字节尾，首个普通等键是 `01/287/3697`。再接 `01/288/3713`，该页 r3701 为日文 48／繁中 42 字节，r3712 是零字节尾。

10:34 候选实测 `meswnd` 在片头后保持同一实例。r3691 在播放前预读，片头自然结束后才取得 r3697 等键和实际合成。修正后的影片守卫按完成时间、等键标记和正文实际自然合成排序。复跑命令使用 `--resume-from <11:01 批次根目录> --retry-restored-checkpoint --save-at 3697 --to 3713`，省去已验的片头前写档段。 [首轮片头回执](F:/CS2-build-cache/playthrough/2026-11/20261121_110150_439031_cxh4zsjs_neko_language_stage/jp/scene01_checkpoint_roundtrip.json)

11:43 冻结候选已用这条命令通过。写档端确认片头画面推进与自然结束，然后在 `01/287/3697` 写入普通槽。独立读取该槽时游戏再次自然播放同组片头，恢复 r3697 后再翻一页到 `01/288/3713`。后续从 `11:49` 批次根目录复用隔离槽，推进到 r4178。 [片头后写档与冷读回执](F:/CS2-build-cache/playthrough/2026-11/20261121_114906_030324_r9px_qrl_neko_language_stage/jp/scene01_checkpoint_roundtrip.json)

`--fast-text` 仅调整隔离档的游戏原生文字速度。r3697 恢复需自然重放组内片头，回放路线为这个已知起点采用 14400 帧等待上限。验收复用片头实例、PTS、零输入、自然结束与正文等键顺序。11:43 冻结版已用最快文字档覆盖 r4178，再由独立进程冷读并翻到 `01/336/4180`。 [快速文字写档与冷读回执](F:/CS2-build-cache/playthrough/2026-11/20261121_123220_973831_jf835vhg_neko_language_stage/jp/scene01_checkpoint_roundtrip.json)

日文和繁中资源从 r3697 到 r5386 都有 205 个正文等键，含两端，共 204 次场景内翻页。`r3913 → r3960`、`r4685 → r4707`、`r5262 → r5287` 之间会调用已追踪的 `sub_getattm2`。`r4575 → r4668` 有画面淡出和 `fesjs eyecatch 2`，实际资源 `fes.int::eyecatch.fes` 等待画面、计时及语音后自行 `exit 0`。该段以原有按键启动，后继正文合成前只等待。首处分支仍在 `r5386` 读当前 `#1003/#1004`，之后核对实际 CST 加载。

`01/491/5386` 后以 r5387/r5388 的实际变量和加载事件区分 `01_01end_2 → 01_01_2 → 01_02/0/24` 与 `01_01end_1 → H01/0/51`。`01_01end_2`、`01_01_2` 转场均无普通等键。`01_02/0/24` 核对新 CST 的 r7 正文和 r23 零字节尾。

## 进结局的资源路线

下表的页端点来自 `scene.int`，均为 `0x02`。共 5479 个等待标记按现有普通路径计数，包含开场与片尾之后的尾声。章节之间以实际 CST 加载和首次稳定等键作交接。单章内部先验证相邻页，再按同一 context/load 和同一个活窗实例连续跑，按批写终点回执。遇到过场、窗口重建、有多段正文的页、下一条记录不是普通等键，先停在上游已验节点，下一轮只发一键观察。多片段页要验证全部非空正文来源及零字节尾，再以该页最终合成为键前条件。

| 场景 | 首个 → 最后等键 | 离开本段时的资源指令 |
| --- | --- | --- |
| `01` | `0/59 → 491/5386` | `286/3631` 后在 r3669 开始 `nekopara3op`，自然结束后观察 `287/3697`。r5387/r5388 由 `#1003 && !#1004` 决定 `01_01end_1` 或 `01_01end_2`。后者经 `01_01_2` 到 `01_02`。 |
| `01_02` | `0/24 → 47/347` | r384 到 `02`。 |
| `02`、`03` | `0/85 → 511/5187`、`0/69 → 540/4533` | 两章末尾分别经过 eyecatch 3、5，再进入下一章。 |
| `04`、`05`、`06` | `0/53 → 274/2693`、`0/58 → 262/2277`、`0/52 → 401/3668` | 各章末尾分别经过 eyecatch 6、7、8，再进入下一章。 |
| `07`、`07_02` | `0/75 → 320/3557`、`0/6 → 41/196` | `07` 条件尾分流，`07_01end_2 → 07_01_2 → 07_02`，再经 eyecatch 10 到 `08`。 |
| `08`、`H03`、`08_02` | `0/58 → 143/1454`、`0/51 → 94/534`、`0/19 → 29/679` | `08` r1492 到 `H03`。H03 尾部同样按分流状态进入转场，`H03_01end_2 → 08_01_2 → 08_02`，再经 eyecatch 11 到 `09`。 |
| `09`、`10`、`11` | `0/65 → 359/3415`、`0/59 → 523/5632`、`0/71 → 576/4606` | 各章末尾分别经过 eyecatch 12、14、15，再进入下一章。 |
| `12`、`12_02` | `0/12 → 128/931`、`0/3 → 149/1317` | `12` 条件尾分流，`12_01end_2 → 12_01_2 → 12_02`，再经 eyecatch 16 到 `13`。 |
| `13`、`13_02` | `0/61 → 450/3649`、`0/7 → 55/330` | `13` 条件尾分流，`13_01end_2 → 13_01_2 → 13_02`。`13_02` r367 写 `#618=1`，r368 播放 `nekopara3ed #550`，r369 到 `14`。 |
| `14` | `0/52 → 66/491` | r529 派发 `ACHIEVEMENT_2_15`，r530 写 `#550=1`，r532 返回标题。 |

`01` 在 r653 后可先按 `95/1398 → 143/1867 → 191/2616 → 239/3153 → 286/3631` 划分验收，片头后再以 `287/3697 → 335/4178 → 383/4560 → 431/5044 → 479/5328 → 491/5386` 收束该章。这些是资源边界，总段按页确认等待和活接收者，复杂过场可在更近的已验页切分。

`update00.int` 有条件路径的五个有等键场景。分流处读实际 `#1003/#1004`，保持本次 profile 的游戏设置，核对判定与目标 CST 加载。条件路径如下，若实际 `#40=1` 跳到 `rc_end`，在该回想分支留独立回执。

| 条件场景 | 首个 → 最后等键 | 正常剧情合流 |
| --- | --- | --- |
| `H01` | `0/51 → 364/1984` | `01_01 → 01_02` |
| `H02` | `0/51 → 424/2136` | `07_01 → 07_02` |
| `H03_01end_1 → H03_02` | `0/8 → 86/401` | `08_01 → 08_02` |
| `H04` | `0/51 → 516/2251` | `12_01 → 12_02` |
| `H05` | `0/53 → 483/2599` | `13_01 → 13_02` |

剧情资源审计未找到玩家选项指令，变化来自脚本状态。片头和片尾观察同一影片实例自然结束、音视频提交和后继脚本加载。结局验收需要 `14/66/491` 的最后一键、完成标记与全新标题的稳定等待，标题零输入。 [已有场景审计](D:/迅雷下载/CS2/archive/2026-11/20261118_205816_5973_neko_fontind/20261118_neko_first_body_batch.md:36)

## 检查器所需日志字段

沿用 `replay frame=N checkpoint=...`、`verified=...`、host `probe wait` / `guarded press` / `pointer` / `down` / `verified mouse press`、`neko-cstl-load` / `neko-cstl-read`、`task-create`。菜单步骤使用 `menu-plan` 的标签。以下三种事件是接线规格，由运行时的实际状态和文件操作输出。

- `frame=N neko-save-control <单行 JSON>` 对每个菜单按键输出一次，取样帧必须与 `checkpoint` 和 `driver_frame` 一致。公共字段为 `label`、`window`、`owner`、`control`、`block`、`point`、`hit_region`、`candidates`、`receivers`（按候选顺序的 `[region,receiver]` 对，可空接收者）、`region`、`response`、`driver_frame` 和 `ready`。`ready=true` 由控制器在完整帧、聚焦、未暂停、当前 FES 等待且没有挂起处理器或消息时产生。Save 相关三步另有 `retained=["01",24,218,context,load_id,owner]`。槽步骤带 `slot`、`slot_exists`、`globals`、`locals`，Load 槽另带 `ttlflow`。确认步骤带 `slot`、`saveload`，标题 Load 带 `flow`、`ttlflow`。
- `frame=N neko-slot-open native=247 slot=0 file="save0000.dat" handle=<非零> sha256=<实际读入 CSS 字节摘要>`。仅在确认后、实际用于恢复的 CSS 成功解析并分配句柄处输出。现有 `trace-native:247` 只给原始参数，不包括文件名、读到的字节或返回值。若实跑使用另一读取入口，按实测更新此回执契约和检查器。
- `frame=N neko-kitchen-ready <单行 JSON>` 在 r337 终点输出，字段为 `owner`、`background`、`model`、`object`。资源名称来自当前绑定。资料表上的静态名称只用于比对。`--require-kitchen-assets` 要求此事件。

主线程的 `--receipt` JSON 使用 `schema: "cs2-neko-save-roundtrip/v1"`、`language`、`save_confirm`、`reopen_startup`、绝对 `output_prefix` 和 `producer` / `consumer` 两个对象。各对象应由启动方记录 `pid`、`start_ms`、`exit_ms`（同一时钟的 Unix 毫秒）、`exit_code`、`output_prefix`、`sha256`、`source_tree_sha256`、`resource_tree_sha256`、`events`、`host`、`slot_before`、`slot_after`。资源摘要沿用 `run_playthrough_batch.resource_tree_sha256`。日志路径可相对于回执目录。`slot_before` 在写入侧为 `null`，其余三个槽观察为 `slot` 命令返回的 `{size, sha256}`。检查器读取当前 CSS 并比对启动回执，不重新计算整棵资源树。标题观察是另一轮，正式读档进程使用独立日志文件。

同私有 profile 的复验证明同版独立进程可以正常打开槽并恢复。若将槽收入共享 `pc_ordinary_saves.json`，还要在新私有 profile 中只复制该 CSS 文件，按需设置冷启动语言，再独立验证恢复与后继。该方法与当前的 `approved` 准入一致。

18:25 首次日文进程终止前观测到 `01 13/156`，检查 `to-200` 的结果为 `insufficient`。主线程双语重跑的独立档目录为 `F:/CS2-build-cache/playthrough/2026-11/20261120_184943_677509_97mwwni2_neko_language_stage`，验收回执为 `archive/2026-11/20261120_neko_to_r200_receipt.json`。日文 f2988、繁中 f2543 均通过 `01 22/200`，每种语言 23 页的 CSTL 来源全部匹配，24 次物理点击包括语言确认和 New Game，终点零点击。两边正文候选均是 `[3,77,52,57,7]`，命中 77，消费正文的 region 52 归 `meswnd` owner 35。19:06 双语 `to-218` 回执把证据延长到 r213 和 r218。19:39 双语面板检查器均通过，汇总在 `archive/2026-11/20261120_neko_r218_save_entry_receipt.json`。
