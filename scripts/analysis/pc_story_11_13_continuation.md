# 11／13 剧情接续步骤

## 10／11／13／15 合批

### 15 跨窗与普通档入口

01:31 批次在 f3137 显示 `yum_h01 6/19`，f3140 的守卫确认 r18 已提交并合成，region 96 → task 8。`r19` 纯观察补丁已基于同时含 11 影片诊断和 04 普通档控制的 `story_replay.rs` 重基。重基版在接入前通过正向检查，接入后通过反向检查。后续 `r38 → r41` 补丁基于这个重基版。 [01:31 日志](F:/CS2-build-cache/playthrough/2026-11/20261121_013108_331065_y28060zb_playthrough/15.host.log:50) · [重基版](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_15_labyrinth_r19_cross_window_observe_rebase.patch>) · [局部后继](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_15_labyrinth_r38_r41.patch>)

新场景沿用已有标题输入。`6/19` 发一次 Enter，待输入释放和脚本转场自行完成。只在 `7/38` 的已验证 `meswnd` 活窗、同一 CST context/load、精确来源 r37、同代自然完成和投影、`img_wait` 已合成及活 Enter 同时成立时再发一次。只允许在跨 r19 处重新识别窗口和接收者。`8/41` 须有同窗同接收者、递增正文代次和来源 r40，并在该页停键。`view 1 meswnd_h` 是 CST 参数。若实际出现 `meswnd02`，其 FES 无 `img_wait`，保留活窗口诊断并停在 r38。 [局部后继清单](<D:/迅雷下载/CS2/scripts/analysis/pc_story_15_r38_r41_guard.json>)

```powershell
$a = 'D:/迅雷下载/CS2/scripts/analysis'
git apply --check archive/2026-11/20261121_story_15_labyrinth_r38_r41.patch
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 15 --scenarios "$a/pc_story_15_r38_r41_guard.json" --jobs 1
python -B "$a/check_pc_story_15_successor.py" $Run15 --cross-window
```

验收为 `cross_window_successor_verified`。核对 r19 后的 wipe、计时、重绘和 view 命令边界，r19 与 r38 各一次物理消费和同帧 VM 恢复，r41 零下一键。`r38_position_observed` 仍需按 `windows`、`labyrinth15`、CST 来源和接收者核对首个缺项。`r19_enter_verified_r38_pending` 表示已验证 r19 消费和释放，尚待 r38 位置。

普通档首次入口用已实证的 `6/19` 锚定。两版 `meswnd` 的 `btn_sys[4/5]` 进入 `CONF_QSAVE` / `CONF_QLOAD`，普通 `_SYS_SAVE` / `_SYS_LOAD` 则分别发送 19 / 20。`sscript.kcs` 在 `0xa1405` 将 `saveload` 交给原 native 447。父 `saveload/MENU_WAIT_USER@0x6629` 的 `btn_menu_j[0]` 记录 `_SAVEOBJ_`、置 `$800=0` 并启动 `_saveload`，`btn_menu_j[1]` 对应 `$800=1`。写入侧应观察父 `LOOP_SAVELOAD` 和子 `_saveload/SL_WAIT_USER@0x7e2f`。尚缺从正文等键打开父菜单的实际物理输入回执，首个入口探针应在同版活 `6/19` 核对命名控件或键映射、遮罩命中、活接收者和物理消费，再观察父子任务关系、真实普通保存模式、页号与空槽 `btn_data[0]`，以进入子菜单为探针终点。

写槽与冷读分别使用新隔离 profile。写入验收要求游戏普通菜单完成唯一指定槽的写入，退出后核对 CSS 完整性和 SHA-256。冷读进程仅载入这一普通槽，并由实际菜单回到 `yum_h01 6/19` 的同源正文和等键，再核对一次真实消费与后继。取得这两次回执后才登记 15 的普通槽。 [普通槽清单](<D:/迅雷下载/CS2/scripts/analysis/pc_ordinary_saves.json>)

### 01:20 回执后的增量

- **10** 在 f758 载入 `OP01`，f759 用 native 327 读取 g0/r3，目标地址 `0x48553` 紧接着由 native 276 发布到槽 1000，`MES_SETSTRING` 入队 20 字节，epoch 7 于 f778 自然完成。晚些时候仍为 `0/4`，活绑定 region 57 → task 8。现有 FES 来源判断把独立默认色控制当成未标注正文，有损失前缀来源的路径，同版 `STRLAYOUT` 也会清标签。增量限定 PE、`scene.int::OP01.cst`、原消息窗哈希和对应偏移。它记录 r3/r13/r14 的读取，识别 `MES_SETSTRING@0x5b1f` 的默认色控制，并只对已经标注的当前来源保留 `STRLAYOUT@0x57fb` 的标签。f759 具体命中哪个格式门闩由新日志确认。r4 守卫确认活等键后唯一发一次正常 Enter，终点 `1/15` 要求 r13、r14 顺序提交和合成。 [f759 发布](F:/CS2-build-cache/playthrough/2026-11/20261121_012016_752164_adw66c2e_playthrough/10.events.log:2995) · [首次翻页补丁](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_10_op01_first_enter.patch>)
- **11** f1779 原脚本在 r91 调用 `pl 1 stop`，同一循环至 f2700 仍合成，r94 的原门闩要求已停止。独立候选允许已验证背景循环陪同精确 `3/94`，所有正文来源、提交和活接收者约束继续生效。唯一 Enter 后验 `4/97` 并停键。r95–r97 没有重复 stop 命令。只有原实例执行 native 267、有需要时随后执行 268，且失去活跃合成，才能判成功停止。若 r97 仍见同一循环，单独给出 `r97_verified_radio_still_looping`。01:20 的原 dispatch 诊断记录了较早的 `stop_case`，r91 发生时段未触发此前五个位点。 [r91 命令](F:/CS2-build-cache/playthrough/2026-11/20261121_012016_752164_adw66c2e_playthrough/11.events.log:27456) · [r94 守卫](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_11_radio_r94_enter.patch>)
- **13** 01:20 回执确认 `12/139`，守卫记录在 f3186，正文 r138、epoch 56、region 57 → task 8。新候选走 `13/151 → 14/163 → 15/174 → 16/186 → 17/188`。前四步记录 `rdraw 30`、脚本 `wait` 和下一个实际输入标记的间隔，r188 前为相邻正文。r188 停键，r190 有 `pl 0`。 [r139 实跑](F:/CS2-build-cache/playthrough/2026-11/20261121_012016_752164_adw66c2e_playthrough/13.host.log:62) · [r188 增量](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_13_eden_r188_segment.patch>)
- **15** 修过入口白名单的 01:31 批次通过七个活等键，f3140 到 `yum_h01 6/19`，epoch 37、region 96 → task 8。跨窗候选在同一守卫再发一次 Enter，记录消费、同帧恢复、释放及后续活窗口。检查器区分 r38 的位置回执与仍在转场，核对 `wipe wipe18 120`、`wait 30`、`rdraw 120`、`view 1 meswnd_h`。同版资源含 `meswnd.fes`、`meswnd02.fes`，后者尚无活窗口证据。 [r19 实跑](F:/CS2-build-cache/playthrough/2026-11/20261121_013108_331065_y28060zb_playthrough/15.host.log:50) · [跨窗补丁](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_15_labyrinth_r19_cross_window_observe.patch>)

13 普通槽以 r188 稳定等待作为候选锚点。原 `meswnd.fes` 的 `_SYS_SAVE` 和 `_SYS_LOAD` 分别发送 19 和 20，`saveload.fes` 的菜单循环为 `LOOP_SAVELOAD`。首次观察只定位活菜单、实际槽位注册、启用状态、目标任务和遮罩。后续在隔离 profile 用空普通槽验证确认流程和真实 CSS 写盘，归档槽号、文件大小及哈希、资源树与播放器身份、存档前的 `BLAOP1_END 17/188` 回执。独立进程仅暂存该槽，从游戏的读取菜单回到同一正文，再用守卫推进到另一个活等键，才登记为批准槽。自动批跑沿用 `--ordinary-saves`，当前 13 尚无可用条目。 [普通槽清单](<D:/迅雷下载/CS2/scripts/analysis/pc_ordinary_saves.json>)

```powershell
$a = 'D:/迅雷下载/CS2/scripts/analysis'
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 10 11 13 --scenarios "$a/pc_story_10_11_13_15_next_waits.json" --jobs 1
python -B "$a/check_pc_story_10_op01.py" $Run --first-turn
python -B "$a/check_pc_story_11_13_successor.py" 11 $Run --radio-r94-enter
python -B "$a/check_pc_story_11_13_successor.py" 13 $Run --eden-r188
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 15 --scenarios "$a/pc_story_15_r19_cross_window_observe.json" --jobs 1
python -B "$a/check_pc_story_15_successor.py" $Run15 --after-r19
```

### 00:44 回执后的增量

- **10** 已在 f750 加载 `start`。恢复包的 SHA-256 为 `fe0f4a3d6d7c65368b201a79fa0352482f2a4f1a41dff3e1606f69a43fb64a8b`。`start` 的 r31 进入 `OP01`，首个正文候选为 `0/4`，来源 r3。新清单只保留标题点击，观察到 f2400。验收同时要求 `OP01` 加载、当前 CST 来源、同代自然完成和提交、正文与等键提示已合成、root native 71 `0x9f6` 等待和活 Enter 绑定。f2400 是观察上限，超时按首个缺失条件定位。 [f750 回执](F:/CS2-build-cache/playthrough/2026-11/20261121_004435_921525_l1eui_c8_playthrough/10.events.log:2493) · [观察补丁](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_10_djibril_op01_observe.patch>)
- **11** 23:55 实跑确认 f1779 到 `00_001 3/94`，f2700 的 handle 384、plane 31、playback 1 仍循环合成。新补丁在 `LoggingHost::call` 唯一一次原 dispatch 返回后追加 `radio-plane-command-gate`，记录 native/next PC `21/0x44be9`、`213/0x44c68`、`311/0x44ce4`、`514/0x44d28`、`19/0x468b9`。沿脚本顺序定位第一个未通过的条件。r94 仍须同一实例经过真实 267/268、退出活跃合成并进入当前正文等键。后置诊断补丁取代旧的 `radio_plane_command_gates` 候选。 [后置诊断补丁](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_11_radio_post_dispatch.patch>) · [f2700 回执](F:/CS2-build-cache/playthrough/2026-11/20261120_235848_203818_u4liwo6v_playthrough/11.host.log:63)
- **13** 00:44 的八个活等键全部通过，末点 f3995 为 `BLAOP1_END 11/121`，epoch 51、r120、region 57 → task 8。新段在此消费一次 Enter，经过脚本退窗、`wipe wipe07 60` 和 `rdraw 30` 的自然等待，停在 `12/139`。验收 r138 来源和新代已提交合成，沿用同一窗口、CST load 与接收者要求，检查唯一消费、同帧 VM 恢复、释放扫描和终点停键。 [00:44 回执](F:/CS2-build-cache/playthrough/2026-11/20261121_004435_921525_l1eui_c8_playthrough/13.host.log) · [r139 补丁](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_13_eden_r139.patch>)
- **15** 00:24 的已验上限为 `yum_h01 0/7`。主检出已有 r9 守卫，新增量沿相邻等键 `1/9 → 2/11 → 3/13 → 4/15 → 5/17 → 6/19`，各步检查当前记录和来源哈希、自然完成、投影、唯一消费和释放。r19 后有退窗、fade、`wipe wipe18 120`、`rdraw 120` 和 `meswnd_h` 切换。下一候选 `7/38` 来源 r37，须先用新批次核对新窗口的标记、提交及接收者。 [00:24 回执](F:/CS2-build-cache/playthrough/2026-11/20261121_002446_022100_phfaym7b_playthrough/15.host.log:131) · [r19 补丁](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_15_labyrinth_r19.patch>)

```powershell
$a = 'D:/迅雷下载/CS2/scripts/analysis'
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 10 11 13 15 --scenarios "$a/pc_story_10_13_15_next.json" --scenarios "$a/pc_story_11_radio_stop.json" --jobs 1
python -B "$a/check_pc_story_10_op01.py" $Run
Select-String -LiteralPath "$Run/11.events.log" -Pattern 'radio-plane-command-gate'
python -B "$a/check_pc_story_11_13_successor.py" 11 $Run --radio-stop
python -B "$a/check_pc_story_11_13_successor.py" 13 $Run --eden-r139
python -B "$a/check_pc_story_15_successor.py" $Run --continuation
```

10 由主线程在批跑选中的游戏目录提供已校验的 `scene.int`。11 若仍以 `frame_limit` 结束，用 `--radio-blocker` 核对停点和影片实例。上述接续回执均属于 PC 中间进度。

### 2026-11-21 PC 接续

- **10 已恢复安装产物。** 旧记录中的 f750 失败应用于 FWP 原件。KIF 目录可索引，但启动配置登记的是 `scene.int`，常规 KIF 读取没有安装阶段的 FWP 载荷转换。离线脚本按三个输入文件的 SHA-256 验证同版来源，以 CST 签名和安装清单 CRC 核对转换参数。15 的已安装对照包逐字节一致。10 的产物 SHA-256 为 `fe0f4a3d6d7c65368b201a79fa0352482f2a4f1a41dff3e1606f69a43fb64a8b`，264 个 CST 的封装、组表和记录边界均通过。现有 f755 清单只做首次启动复测，验收 `sscript loaded > start`、无加载错误和零正文键。 [转换器](../convert/restore_pc_story_10_scene.py) · [已验归档](F:/CS2-build-cache/archive/2026-11/20261121_story_10_verified_scene.int) · [入口清单](<D:/迅雷下载/CS2/scripts/analysis/pc_story_10_start_repro.json>)
- **11 停在影片停止分支。** 同实例 handle 384 在 f2700 仍 active 并合成。原 `sscript` 在 `0x44bde` 比较命令族、`0x44c5d` 解析目标、`0x44cd9` 查询运动状态，停止分支 `0x46905` 进入例程，随后条件成立才有 `0x19d63` 的 native 267。新归档增量记录精确调用点的返回值，既有 r94 清单可直接用。验收到同一 handle 真停、无合成和 r94 活等键。循环实例等待脚本停止，后续独立非循环片段以同实例自然结束回执验收。 [诊断补丁](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_11_radio_plane_command_gates.patch>) · [f2700](F:/CS2-build-cache/playthrough/2026-11/20261120_235848_203818_u4liwo6v_playthrough/11.host.log:63)
- **13 从已验 `5/107` 接续。** 既有七步清单封包已经包含 `5/107 → 6/109` 及后续到 `11/121` 的相邻等键。用原检查器 `--continuation`，每步须同一上下文、load、窗口及接收者，新代正文已提交并合成，每个 Enter 唯一消费并同帧恢复 VM。保留已验的首个输入回执。 [接续清单](<D:/迅雷下载/CS2/scripts/analysis/pc_story_11_13_continuation.json>) · [r107 实跑](F:/CS2-build-cache/playthrough/2026-11/20261120_235848_203818_u4liwo6v_playthrough/13.host.log:19)
- **15 从已验 `0/7` 接续。** 00:24 批次 f4159 片头 handle 228 自然释放，f4419 提交 epoch 7，f5600 同一 context 129 / load 5 的 r6 已合成且 region 96 → task 8 有效。新守卫只在这套活状态确认后发一次 Enter，下一页 `1/9` 必须是同一 load 的 r8、新代自然提交、相同窗口和绑定，并在终点停键。新检查器对 00:24 日志返回 `opening_wait_verified`。新守卫待构建实跑。 [守卫补丁](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_15_yum_h01_r9_guard.patch>) · [r9 清单](<D:/迅雷下载/CS2/scripts/analysis/pc_story_15_yum_h01_r9_guard.json>) · [00:24 回执](F:/CS2-build-cache/playthrough/2026-11/20261121_002446_022100_phfaym7b_playthrough/15.host.log:131)

```powershell
$a = 'D:/迅雷下载/CS2/scripts/analysis'
$game10 = 'D:/迅雷下载/CS2/games/戦国天使ジブリール'
python -B "$a/../convert/restore_pc_story_10_scene.py" $game10 --output "$game10/scene.int"
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 10 --scenarios "$a/pc_story_10_start_repro.json" --jobs 1
Select-String -LiteralPath "$Run10/10.events.log" -Pattern 'sscript (loaded|load failed) > start'
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 11 --scenarios "$a/pc_story_11_radio_stop.json" --jobs 1
Select-String -LiteralPath "$Run11/11.events.log" -Pattern 'radio-plane-command-gate'
python -B "$a/check_pc_story_11_13_successor.py" 11 $Run11 --radio-stop
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 13 --scenarios "$a/pc_story_11_13_continuation.json" --jobs 1
python -B "$a/check_pc_story_11_13_successor.py" 13 $Run13 --continuation
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 15 --scenarios "$a/pc_story_15_yum_h01_r9_guard.json" --jobs 1
python -B "$a/check_pc_story_15_successor.py" $Run15
```

PC 通关回执需要实际经过指定路线的结束卡或制作名单，并核对游戏通关状态已写入且重开可读。首等键、进入片尾或自然释放影片均只登记为中间进度。四条线路按实际游戏状态推进，平板在 PC 验收之后。

主线程同版播放器通过 `--player` 传入。专用清单使用单槽，沿用批跑器的资源预算。13 选单步 `legacy-11-13-enter:13`，若布局完成恢复，只验证 `4/103 → 5/107`。15 以 f5300、f5600 为无正文键的观察点。

```powershell
$a = 'D:/迅雷下载/CS2/scripts/analysis'
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 10 11 13 15 --scenarios "$a/pc_story_10_11_13_15_batch.json" --jobs 1
python -B "$a/check_pc_story_11_13_successor.py" 11 $Run --radio-stop
python -B "$a/check_pc_story_11_13_successor.py" 13 $Run --eden-blocker
python -B "$a/check_pc_story_11_13_successor.py" 13 $Run
Select-String -LiteralPath "$Run/15.host.log" -Pattern 'labyrinth15='
Select-String -LiteralPath "$Run/15.events.log" -Pattern 'labyrinth15-text|sscript loaded > yum_h01|fes-text-sourced-queued'
```

13 的两个检查分别用于 `frame_limit` 和 `completed`。`--eden-blocker` 返回 `eden_scroll_blocker_reproduced` 时读取 `eden-scroll-gate`，定位第一个失败的条件。单步通过时检查返回 `successor_verified`，再用七步清单验收后续页。 [合批清单](<D:/迅雷下载/CS2/scripts/analysis/pc_story_10_11_13_15_batch.json>)

15 在 22:31 批次 f4393 加载 `yum_h01`，f4394 native 327 读取 `g0/r6` 并经 `MES_SETSTRING` 以 CP932 入队，f4454 停 Draw task 30，f4455 进入 `g0/r7`。f4800、f5000 均见 `local100=Some(1)`、`composited=true`、`enter=Some((96, 8))`，但正文来源为 `None`，代次 7 带 `forced=true` 和 `committed_epoch=None`。下一次从 `labyrinth15-text` 核对 `MES_SETSTRING` 输出、`STRLAYOUT` 清除前的来源及随后的 skip/draw。首等键验收要求 r6 来源、当前代完成和提交、活绑定、零正文键和零截图。r8／r9 留待该验收通过。 [15 入队](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/15.events.log:14222) · [15 停点](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/15.host.log:131) · [15 独立诊断增量](<D:/迅雷下载/CS2/archive/2026-11/20261120_story_15_yum_h01_body_diagnostics.patch>)

23:11 候选的 15 在 f4825 自然释放片头句柄 228，较 22:31 批次晚 629 帧。f4827 加载 `opback_yum`，f5000 仍为 `opback_yum 0/11`。本组清单将观察点移至 f5300、f5600。上述两点为基于片头时差的观察窗口，等待验收仍以实际 `yum_h01`、r6 来源和 r7 活等键为准。 [23:11 片头回执](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/15.events.log:14768) · [f5000 停点](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/15.host.log:105)


## 22:08 实跑与接续

- **11 已验收至 `00_001 2/90`。** f1526、f1740 两次正文 Enter 由 region 62 → task 8 唯一消费并同帧恢复 VM。f1778 同一 `cinema` owner 13、CST context 139 / load 2 已提交正文 r89，三个等键都有影片 handle 384 的合成 PTS。日志检查返回 `continuation_verified`。 [11 终点](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/11.host.log:56)
- **11 下一次停 `3/94`。** r91 执行 `pl 1 stop`，随后是正文 r93、等键 r94。新增 `legacy-11-radio-stop`，从已验收的 r90 守卫只多发一次正文 Enter。检查器要求同一影片句柄的 267／268 调用、三次唯一物理消费和同帧恢复，以及同一窗口和 CST load 的新正文代次。句柄若仍登记，终点须显示未播放且无合成 PTS。r94 零下一键、零截图，上限 2700 帧。 [11 增量](<D:/迅雷下载/CS2/archive/2026-11/20261120_story_11_radio_stop_r94.patch>) · [11 清单](<D:/迅雷下载/CS2/scripts/analysis/pc_story_11_radio_stop.json>)
- **13 已复现 `4/103` 布局完成阻断。** f1610 片头自然结束，f1612 加载 `BLAOP1_END`，f2420 owner 25 认领 r102。f2421 两次 `MES_SCROLLSPD@0x6809` 均为 `layout_skip=false`。f2481 记下 `[input]`，f5600 仍是 `forced=true`、`committed_epoch=None`、`eden_layout_complete=None`，活 Enter 为 region 57 → task 8，正文键和截图均为零。使用原清单复现。诊断增量记录系统处理器、跳过标志、FES 指纹及正文代次的各项条件。后续仍用原七步守卫验收。 [首个跳过事件](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/13.events.log:9343) · [13 终点](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/13.host.log:14) · [13 诊断增量](<D:/迅雷下载/CS2/archive/2026-11/20261120_story_13_eden_scroll_gate_diagnostics.patch>)

只读解包确认 `fes.int` 和 `UPDATE00.INT` 中的 `meswnd.fes` 都是 4971 字节，SHA-256 均为 `24d7b9d39471a48939c6ad5329e5f21d2431d8cc4b2168992c1e9cf0ee9ee205`。`MES_SCROLLSPD` 在 `0x67f9` 调用 `STRLAYOUT`，随后在 `0x6809` 执行 `str skip`。旧实跑 f2421 的 `STRLAYOUT` 返回记录显示 `skip=false`，第一条 `str skip` 随即报告 `layout_skip=false`。新批优先核对第一条 `eden-scroll-gate` 的处理器、四个跳过标志和运行中 FES 匹配，再核对同帧重复调用有无覆盖资格。 [f2421 事件顺序](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/13.events.log:9342) · [当前判据](<D:/迅雷下载/CS2/simulator/crates/desktop/src/fes_text.rs:783>)

```powershell
$a = 'D:/迅雷下载/CS2/scripts/analysis'
$Known = 'F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough'
$KnownPlayer = 'D:/迅雷下载/CS2/archive/2026-11/20261120_220537_815819_story_replay_integration/story_replay.exe'
python -B "$a/check_pc_story_11_13_successor.py" 11 $Known --continuation
python -B "$a/check_pc_story_11_13_successor.py" 13 $Known --eden-blocker
python -B "$a/run_playthrough_batch.py" --player $PlayerWithR94Patch --indices 11 --scenarios "$a/pc_story_11_radio_stop.json" --jobs 1
python -B "$a/check_pc_story_11_13_successor.py" 11 $Run11 --radio-stop
python -B "$a/run_playthrough_batch.py" --player $KnownPlayer --indices 13 --scenarios "$a/pc_story_11_13_continuation.json" --jobs 1
python -B "$a/check_pc_story_11_13_successor.py" 13 $Run13 --eden-blocker
```

## 23:21 同批结果与增量

- **10** 在 f711 由 region 39 → task 11 消费标题键，在 f750 读 `start` 失败。主线程续跑复现了资源缺口，继续使用同版已安装资源作为修复前提。 [标题回执](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/10.host.log:4) · [加载失败](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/10.events.log:2493)
- **11** 在 f1779 由 region 62 → task 8 消费第三次 Enter 并恢复 VM。同帧输出 `pl 1 stop`，f1780 记 `3/94`，epoch 16 于 f1809 自然显完并提交。f2700 仍登记一项影片，r94 守卫未通过。同一影片为 handle 384、平面 31、playback 1。日志缺 267／268 的停止回执和终点影片样本。本组清单追加 69、70、702、989 跟踪，归档诊断增量在该路线的帧上限输出影片是否活跃及其合成 PTS。检查器的 `--radio-blocker` 验收此阻断，`--radio-stop` 仍要求实际停止和 r94 活等键。之后才核对 `4/97`。 [CST 命令](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/11.events.log:27443) · [f2700](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/11.host.log:62) · [11 归档增量](<D:/迅雷下载/CS2/archive/2026-11/20261120_story_11_radio_stop_terminal_diagnostics.patch>)
- **13** f3784 的第一条 `MES_SCROLLSPD@0x6809` 有精确 r102 来源、`live_fes=Some(true)`、四个跳过标志全关，却因外层 handler 判据失败。当前检出已改为精确 FES 指令加跳过标志，并允许同代隐藏重排。新归档增量在此基础上处理 f3845 的可见调用，须先有同代隐藏布局回执。换面后须重新提交同代像素。检查器接受相同来源、代次及正文哈希的重复布局回执。正例要求 r102 当前代已提交并合成，region 57 → task 8 唯一消费 Enter，同帧 VM 恢复，停在 `5/107` 的 r106 正文。单步通过后用七步清单到 `11/121`。 [首个失败判据](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/13.events.log:13432) · [可见重排](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/13.events.log:13792) · [13 归档增量](<D:/迅雷下载/CS2/archive/2026-11/20261120_story_13_eden_scroll_layout_completion.patch>)

```powershell
$a = 'D:/迅雷下载/CS2/scripts/analysis'
$Known = 'F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough'
python -B "$a/check_pc_story_11_13_successor.py" 11 $Known --radio-blocker
python -B "$a/check_pc_story_11_13_successor.py" 13 $Known --eden-blocker
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 11 13 --scenarios "$a/pc_story_10_11_13_15_batch.json" --jobs 1
python -B "$a/check_pc_story_11_13_successor.py" 13 $Run
python -B "$a/check_pc_story_11_13_successor.py" 11 $Run --radio-stop
```

## 23:55 候选复跑

- 13 的单步检查返回 `successor_verified`。f3421 的 `4/103` 已提交 epoch 16，f3422 的 Enter 由 region 57 → task 8 消费并同帧恢复，f3426 在 `5/107` 验到同一 context 174 / load 3 的 r106 和 epoch 21。七步用 `pc_story_11_13_continuation.json`，从入口复跑到 `11/121`，用检查器 `13 $Run13 --continuation` 验收。 [单步日志](F:/CS2-build-cache/playthrough/2026-11/20261120_235848_203818_u4liwo6v_playthrough/13.host.log:19)
- 11 的 `--radio-blocker` 返回 `radio_stop_blocker_reproduced`。f1779 有 `pl 1 stop`，同帧未见 267、268、69、70、702 或 989。f2700 的 handle 384 / plane 31 / playback 1 仍为 `active=true`，合成 PTS 为 13.8 s。r94 守卫继续保持，第四次正文 Enter 待实际影片停止。 [命令](F:/CS2-build-cache/playthrough/2026-11/20261120_235848_203818_u4liwo6v_playthrough/11.events.log:27445) · [终点影片](F:/CS2-build-cache/playthrough/2026-11/20261120_235848_203818_u4liwo6v_playthrough/11.host.log:63)

## 15 首个正文等待

23:21 批次 f4079 自然释放片头 handle 228，f4276 加载 `yum_h01`，f4338 标记 `[input]`。f5300 和 f5600 均停在 `0/7`，`meswnd` owner 12、context 129 / load 2、`local100=1`、活绑定 region 96 → task 8，正文及 `img_wait` 均已合成。epoch 7 的来源仍为 `None`，且 `forced=true`、`committed_epoch=None`。 [终点](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/15.host.log:131)

f4277 的两次 `MES_SETSTRING@0x55c3` 先发布未标记的前缀，再将 CP932 的 28 字节 r6 正文入队。epoch 5 已丢失来源。原 `sscript` 的默认色分支以 `0xfcc4`、`0x101bd` 的 native 13 拷贝独立 `\cd`，经 `0xe4f` 发布到共享槽 1000、`0xee4` 调用 `MES_SETSTRING`。原版两次 `MES_SETMARK` 各执行一次 `str apendmark`，对应 epoch 6、7。f4278 的首次 `STRLAYOUT@0x53a0` 已见 `provenance_before=None`，f4339 首次可见重排也用同一 epoch 7。归档增量按实值 `\cd`、本作 PE、活 FES 和精确当前 r6 来源限定修补，复用 13 的隐藏布局和可见提交判据。补丁原向检查通过，主检出纳入同一增量后又通过反向检查。 [正文入队](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/15.events.log:13874) · [首次布局](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/15.events.log:13905) · [可见重排](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/15.events.log:14295) · [归档补丁](<D:/迅雷下载/CS2/archive/2026-11/20261121_story_15_yum_h01_body_layout.patch>)

```powershell
$a = 'D:/迅雷下载/CS2/scripts/analysis'
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 15 --scenarios "$a/pc_story_15_yum_h01_observe.json" --jobs 1
Select-String -LiteralPath "$Run15/15.events.log" -Pattern 'movie_naturally_released|sscript loaded > yum_h01|labyrinth15-color-reset|labyrinth15-text .*op=apendmark|labyrinth15-text .*op=skip.*layout_skip=true'
Select-String -LiteralPath "$Run15/15.host.log" -Pattern 'labyrinth15=Some|: down '
```

本轮只验 `yum_h01 0/7`。核对 `labyrinth15-color-reset`、两次 mark 保留 r6、`MES_SCROLLSPD@0x5baa` 的同代隐藏及可见 `layout_skip=true`。终点应有当前 load 的 `g0/r6`、`layout_complete` 与 `committed_epoch` 同代、`forced=false`、`committed=true`、`composited=true`、`local100=Some(1)`、`enter=Some((96, 8))`。只有 f692 和 f821 两个入口点击，零正文键、零截图。若色控回执或同代布局回执缺失，保留该等键。r8 正文和 `1/9` 的下一等待已从 CST 静态核对，待首段新日志验收。 [独立清单](<D:/迅雷下载/CS2/scripts/analysis/pc_story_15_yum_h01_observe.json>)

## 10 启动停点复现

历史实跑 f711 由 region 39 → receiver 11 消费标题左键。f750 调用 native 325 读取 `start`，随后输出 `sscript load failed > start`。新清单沿用 f708 悬停和 f710 点击，要求活 region 39，f755 停且零截图。复现须同时看到标题消费和启动读取失败。若当前播放器已读出 `sscript loaded > start`，记录新状态并按清单停止。 [10 输入回执](F:/CS2-build-cache/playthrough/2026-11/20261119_184206_965642_playthrough/10.host.log:5) · [10 加载失败](F:/CS2-build-cache/playthrough/2026-11/20261119_184206_965642_playthrough/10.events.log:7952)

```powershell
python -B "$a/run_playthrough_batch.py" --player $StoryPlayer --indices 10 --scenarios "$a/pc_story_10_start_repro.json" --jobs 1
Select-String -LiteralPath "$Run10/10.host.log" -Pattern 'region: 39, receiver: 11, response: 0'
Select-String -LiteralPath "$Run10/10.events.log" -Pattern 'native-call index=325|sscript (load failed|loaded) > start'
```

现有目录只有 `scene.int.FWP`，1,490,940 字节，SHA-256 `cbe602d9f74025153d000731d05c2da7786257572542a3f8a99817cb46153b40`。同目录 `install.dfn` 声明输出 `scene.int`。常规 KIF 解出的 `start.cst` 首八字节为 `c18e3f8e995fbba0`。进入正文前须拿到同版安装产物和可解析的 `CatScene` 成员。 [FWP 审计](<D:/迅雷下载/CS2/archive/2026-11/20261119_073547_566427_pc_story_08_10_14_sidecar/20261119_10_fwp_identity.txt>) · [10 清单](<D:/迅雷下载/CS2/scripts/analysis/pc_story_10_start_repro.json>)

2026-11-20 只读复核覆盖当前与旧 CS2 工作区、F 盘构建和发布缓存、游戏库、备份目录及 Windows 卸载项。确认两份同版目录，分别位于当前工作区及 `D:/cs2-workspace-126cd016dea0`。两份 `Djibril5.exe` 均为 2.6.1.30，SHA-256 均为 `012c71635932c241cde4df986377516279a50b907166757e89ea7b86ff3f813f`。两树中的 `.int*` 经 KIF 目录逐包核对，唯一的 `start.cst` 候选来自同字节的 `scene.int.FWP`，偏移 `1039169`、长度 `247`。常规解包后的成员 SHA-256 为 `385d8fbb035eb41769c0fb93a3ec3c287eafeb5187edb66add17a3466fe9d5e5`，`expand_plain_cst` 校验失败。其余同名包或松散成员属于别的游戏、开发套件或测试夹具。资源选择修复缺少可用候选，现有入口复现清单仍适用。 [资源与安装产物对照](<D:/迅雷下载/CS2/reports/pc_story_10_14_resource_sources_20261119.md:18>)

2026-11-21 补查 D 盘下载及网盘目录，F 盘 GAL、汉化备份、项目备份、部署快照和发布缓存，以及另一份 CS2 工作区和缓存核查树。按日文标题、Djibril、FrontWing、下载日期别名及资源文件名查找，仍无同版可读 `scene.int` 或松散 `start.cst`。两份已确认目录在各自工作区的 `games/戦国天使ジブリール` 下。该资源缺口继续作为 f750 的阻断条件。新资源入场先核对同版来源、KIF 目录与 `CatScene` 解压及记录表，再用现有入口清单验证 `sscript loaded > start`。 [f750](F:/CS2-build-cache/playthrough/2026-11/20261120_232125_718634_act2o1sx_playthrough/10.events.log:2493)

记录以 CST 的组号和绝对记录号表示。表内计时是脚本参数。回放期间由游戏自行处理计时、绘制、声音和转场，Enter 只由已确认的等键状态触发。

## 11 未来ラジオと人工鳩

| 路段 | 脚本顺序与证据 | 回放动作 |
| --- | --- | --- |
| `00_001 0/81 → 1/86` | r82 启动语音 `C_com_001_002`，正文 r84、85。循环的 `radio_wave.mpg` 继续在场景平面 31 合成。 | 沿用原单按守卫，逐端点查当前代正文和合成 PTS。 |
| `1/86 → 2/90` | r87 启动语音 `Z_com_001_001`，正文 r89。两等键之间没有计时或影片切换命令。 | 同一守卫再收一次 Enter，`2/90` 停。 |
| `2/90 → 3/94 → 4/97` | 离开 r90 后第一条 r91 是 `pl 1 stop`，然后依次是正文 r93、96。 | 边界待观察。同一循环实例的停止或释放、停止后的实际影片数、消息窗口和 Enter 绑定都需实时核对。 |
| `4/97 → 5/111 → 6/114 → 7/117` | r98–103 启三条循环环境音并淡入 30。r104–108 清平面、切 `bg92d`、执行 `rwipe w_bt` 后用无参数 `wait` 等待。三个正文依次为 r110、113、116。 | 等擦除结束并确认首个活等键。 |
| `7/117 → 8/121 → 9/124 → 10/129 → 11/131 → 12/133` | r118 `pl m2fade 240 255 0`。r125–127 三轨 `se end 300`。对应正文 r120、123、128、130、132。 | 每页靠自然显完和实际等键确认。混音和淡出可以与正文重叠。 |
| `12/133 → 00_002 → 00_002A 0/11` | `00_001` r134–140 退窗，`rwipe w_cine 40 1`、`wait 30` 和无参数 `wait`。r141–178 重置绘制、平面和音频等状态，r179 `next 00_002`。日期卡场景全部是控制记录，r45/49/53 三次擦除各 120，r47/51 `wait 20`，r55/57 `wait 120`，r59 擦除 180，r61 `wait 30`，r63 `next 00_002A`。后一场 r9 `rwipe w_exp 120`，r10 正文、r11 `0x02`。 | 日期卡靠脚本自然通过。到新 CST 按新 load、活窗口、接收者和实际正文等键重新核验。 |

## 13 灰色的乐园

| 路段 | 脚本顺序与证据 | 回放动作 |
| --- | --- | --- |
| `BLAOP1_END 4/103 → 5/107` | r104–105 播放 `ser129`、音量 25，正文 r106。前置片头使用已有同实例自然结束证据。 | 沿用原单按守卫。 |
| `5/107 → 6/109 → 7/111 → 8/113 → 9/115` | 正文 r108、110、112、114。段内没有计时、擦除、影片或切窗命令。 | 每个等键只按一次，逐段核验 VM 恢复和更新的正文代次。 |
| `9/115 → 10/119 → 11/121` | r116–117 再播 `ser129`、音量 25，正文 r118、120。 | 沿用同一 `meswnd` 守卫，`11/121` 停。 |
| `11/121 → 12/139` | r122–123 `frameoff fade 0`、`wait`。r124 音效结束 40，r126–127 `wipe wipe07 60 0`、`wait`。r129 切立绘，r134–135 `rdraw 30`、`wait`，r136 语音，r138 正文。 | 退窗后需确认实际 `meswnd` 生命周期和接收者，再核对 r138 的同代提交。 |
| `12/139 → 19/230` | r151、163、174、186、188、208 是相邻 `0x02`，其间多个换立绘段各有 `rdraw 30` 和 `wait`。到 r230 这一页由 r219、229 两段正文组成，中间 r220 `wait 60`，r227–228 `rdraw 30`、`wait`。 | 单页两段正文必须在同一已提交代次内核对，等到 r230 才有下一次输入候选。 |
| `83/975 → 84/988 → 85/1011` | r976–986 退窗、BGM 结束 60、`rdraw 60`、`auto on 0 0`、`mesdraw 0`、`view 1 dummy`、`wait 1`。r988 有 `0x02`。随后 r996 `fesjs eyecatch`，r999–1002 恢复手动正文和 `meswnd`，r1004 `wait 60`，r1010 正文。 | r988 留给自动模式。眼幕是否等键、接收者归属及退出顺序待现场日志识别。 |
| `269/3304 → 270/3318 → 273/3331` | r3305 退窗 30，r3308 `rdraw 60`，r3309 `wait 150`，r3310–3311 `novel on`／`view 1 novel`，接着进窗和图层淡入各 30。首个 novel 段结尾 r3318。r3331 是本成员首个 `0x03`，引擎将该种类记为 `[page]`。同类后续点还有 r3351、3362、3376、3392、4264、4280。 | 切换到 novel 后须重新识别窗口、分页提示及消费链。 |
| `368/4373 → bla_001B` | r4374 退窗 60，r4376 BGM 结束 60，r4383 `rdraw 60`，r4384 `wait 120`，随后检查 `#710`、执行 `end_of_kaisou 1` 和 `next bla_001B`。新场景 r42–55 又有自动模式、`dummy` 和 `eyecatch`，r56–59 恢复手动 `meswnd`，首个恢复后的正文等待是 `1/61`。 | 实测区分正常新游戏和回想入口。眼幕若进入未知菜单或要求操作，停在现场边界保留日志。 |

## 接线与验收

接续方案从正常入口重放。11 合计两次正文 Enter，停在 `2/90`。13 合计七次，停在 `11/121`。每一按前重新读取 live body、根等待和绑定，前一个物理按键须释放。后继需要同一窗口、CST load 和接收者，以及新正文代次。统一停点状态和任一未知菜单以现场日志为准。当前检查的 CST 片段未定位到可确认的选项及选择接收者。

22:05 集成构建含相邻等待补丁。已核对播放器内的 `legacy-11-13-continue:`、按键释放回执和 Eden 色控诊断标记，四个相关 Rust 文件的哈希也与构建输入相符。该版播放器 SHA-256 为 `527b5c49bb54ffd5a45c59708b95be08df89c354da4840964365f823437e4762`。11 的帧上限为 2700，13 为 5600。 [22:05 构建](<D:/迅雷下载/CS2/archive/2026-11/20261120_220537_815819_story_replay_integration/build.json>) · [离线增量](<F:/CS2-build-cache/archive/2026-11/20261120_story_11_13_adjacent_waits.patch>)

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
$Baseline = 'F:/CS2-build-cache/playthrough/2026-11/20261120_213230_279191_n8ay5wi2_playthrough'
python -B "$analysis/check_pc_story_11_13_sources.py"
python -B "$analysis/check_pc_story_11_13_sources.py" --eden-publication-run $Baseline
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 11 13 --scenarios "$analysis/pc_story_11_13_continuation.json" --jobs 1
python -B "$analysis/check_pc_story_11_13_successor.py" 11 $ContinuationRun --continuation
python -B "$analysis/check_pc_story_11_13_successor.py" 13 $ContinuationRun --continuation
```

验收为 `continuation_verified`、退出码 0 和零截图。保留所有相邻页的物理消费及同帧 VM 恢复回执、11 的每个等待点的同实例影片合成证据、13 的前置片头自然结束证据。时限内未形成精确等待时，回放输出 `frame_limit`，保留最后一次等待诊断。

- **11** 要有 `0/81 → 1/86 → 2/90` 三个实等键，两次正文 Enter 分别获得唯一消费者和同帧 VM 恢复。核对 r83 的 UTF-8 `+9` 与 r88 的 `+12` 拷贝都经 `$str1001` 到同一 `cinema/MES_SETNAME`，三个等待点都有同一循环影片实例的合成 PTS。r90 终点零下一键。
- **13** 要有 `4/103 → 5/107 → 6/109 → 7/111 → 8/113 → 9/115 → 10/119 → 11/121` 八个实等键和七次对应输入。先核对 BLAOP1 自然结束。每页的 GBK 正文须来自同一 CST load，并按当前代的自然完成或该页独立的 `MES_SCROLLSPD layout_skip=true` 回执验收。r121 终点零下一键。

## 只读来源

- 11 的 `scene.int` 成员 `00_001.cst` SHA-256 `9e4efc626b072ff3de336665e3f23bb993e9da4f79f9a59e5bee5ba183b8fe78`，`00_002.cst` 为 `ea6891e694001ae2d3cc2e7bbf884dd63885e463b41b5cfb3ce0b9c4894ec1e1`，`00_002A.cst` 为 `9b38ddecce2b7038ec7367246ae3653e2137cf8168f1bf9ce3d16b07e949ab5f`。
- 13 的 `scene.int` 成员 `BLAOP1_END.cst` SHA-256 `c09bcb90152977cd9d21b68ec019d627ccc80136036503a5ce376a2ae6020561`，`bla_001B.cst` 为 `c8a6d7a3002ae8efaa0371f53627f579e3173e16f4a5f618a0183ec5b2e02fb0`。
- 21:32 回放已验证 11 在 f1526 由 region 62 → task 8 消费 Enter，同帧从 `0xd6d` 恢复。native 13 复制 r83 的 UTF-8 `+9` 后缀，经 `$str1001` 送到 `cinema/MES_SETNAME`。f1739 取得 `1/86` 的同代正文 r84、85。同一影片 handle 384、plane 31 的合成 PTS 从 1.88 到 5.12 秒。下一轮验证 r88 的 `+12` 说话人及 r89 正文。 [11 日志](F:/CS2-build-cache/playthrough/2026-11/20261120_213230_279191_n8ay5wi2_playthrough/11.host.log:48)
- 21:32 回放已验证 13 在 f3028 从同一 CST load 读出 r102 的 97 字节 GBK，f3405 经 native 276、630 入队。f5600 的 `4/103` 有活 Enter 目标 region 57 → task 8，正文仍为 `forced=true`、`sources=None`，该批正文输入为零。接续补丁认领 `MES_SETSTRING` 偏移 `0x640f` 的独立 `\cd` 为零条正文来源。第一按前须核对 r102 当前代来源、`MES_SCROLLSPD` 自己的 `layout_skip=true`、可见提交及片头自然完成。 [13 入队日志](F:/CS2-build-cache/playthrough/2026-11/20261120_213230_279191_n8ay5wi2_playthrough/13.events.log:12261) · [13 停点](F:/CS2-build-cache/playthrough/2026-11/20261120_213230_279191_n8ay5wi2_playthrough/13.host.log:14)
