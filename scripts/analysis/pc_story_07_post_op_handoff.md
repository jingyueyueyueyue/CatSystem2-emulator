# 07 片头自然结束后的首次翻页

## 22:31 回执与第二次翻页

22:31:13 批次使用 22:24:35 同源候选。f2778 同一片头实例 255 自然释放，f3136 唯一物理 Enter 由 region 64 → task 8 以 logical 2、group 0、response 0 消费，根同帧恢复 `[2,0,0,0]`。f3298 到 `com01 1/62`，同一 owner 12、context 156、load 1，正文 61 完成并提交，松键已确认。 [物理回执](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/07/07.host.log:14) · [根恢复](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/07/07.events.log:35476) · [停点](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/07/07.host.log:18)

第二次翻页使用含 `20261120_07_post_op_r68_increment.patch` 的同源 EXE 和 `pc_story_07_post_op_continue.json`。路线从正常入口重走，验收首翻之后，在 `1/62` 的正文、提示层和接收者通过活检查时再发一次 Enter。精确 CST 的 abs 63–66 依序是 `bg 5 bg022a 0 0 0 1`、`bg 5 scale 100% 100%`、`rdraw 30`、`wait`。abs 67 是非空正文，68 是真实的 `0x02` 等键。两次 Enter 分别需要唯一消费、同帧恢复和松键。到 `2/68` 时需要同一窗口和 CST 加载、r67 来源及新代自然完成与合成，然后停止。第二次翻页待实跑。 [续跑增量](D:/迅雷下载/CS2/archive/2026-11/20261120_07_post_op_r68_increment.patch) · [续跑清单](D:/迅雷下载/CS2/scripts/analysis/pc_story_07_post_op_continue.json)

```powershell
python -X utf8 -B 'D:/迅雷下载/CS2/scripts/analysis/run_playthrough_batch.py' --player $player --scenarios 'D:/迅雷下载/CS2/scripts/analysis/pc_story_07_post_op_continue.json' --indices 7 --jobs 1 --timeout 1200
```

## 17:56 历史回执

17:56 候选播放器 SHA-256 为 `cf2ac89ef4e9d6e16ac188e76e8ad9d1c33cfd1bc1747f8cb616166f1d62b24a`。该批次只有一次新游戏点击。证据目录为 `F:/CS2-build-cache/playthrough/2026-11/20261120_181234_719918_7k21ziou_playthrough`。

| 日志 | 可核对的事实 |
| --- | --- |
| `07.host.log:33`、`:35`，`07.events.log:3758` | 标题 `(176,256)` 命中遮罩 20，候选顺序 `[3,20,31,10,7]`。f1141 物理 `Mouse(1)` 由标题任务 11 消费，同帧根恢复 `[2,176,256,0]`。 |
| `07.events.log:4152`、`:4156`、`:4264`、`:4273` | `flow` 任务 6 下创建普通脚本任务 8，任务 8 下创建 `meswnd` 12 和 `bottom button` 64，随后加载 `com01`。 |
| `07.events.log:4477`、`:4478`、`:6096`、`:6142`、`:28446`、`:28465` | abs 56 在 f1274 已读取并送入 `SetString`。同一 `OPmovie.mpg` handle 255 先加载并单次播放，f2766 释放，场景立即执行 `wait 60`。f1500 与 f2400 的合成 PTS 均向前。 |
| `07.events.log:33998`–`:34006`，`07.host.log:204`、`:594` | f3096 读出 abs 56 的正文与 abs 57 的等待。采样时窗口 12 为 `MES_SHOW`、局部 `$100=1`，`keywait` 已提交。epoch 4 的来源 f1274 早于片头，f3123 才自然完成并提交。同版日志没有提供正文来源和 CST load ID 的绑定。 |

历史批次 `20261120_034451_363895_n4klau1__playthrough/07.host.log:10` 的单次 Enter 依序到 region 3 → receiver 0（response 1）、29 → 12（1）、64 → 8（0）。三条回执的 raw mask 都为 1，均是 `Keyboard(13)`、logical key 0、group 0、非重复按下。同一历史批次 `07.events.log:12698` 的根恢复为 `[2,0,0,0]`。当时片头正在播放，因此这组数据只提供接收者预期。新路线用活 `meswnd` 的父任务解析接收者，再检查活 `bottom button` 绑定与预期 `64→8`。

只读重读原始 `scene.int::com01.cst`，其 SHA-256 为 `bccd3463db8d0f173101609a1ad6e57f4edc5d7f2c084763a17826863d8ce086`。abs 56、61 是非空 `0x20`，57、62 是 `0x02`。g1 的 abs 58–60 依序是 `pl 1 fade 40 0 255`、`pl 9 fade 40 255 0`、`wait`。

## 22:08 批次与版本配套

22:08:18 批次实际使用 22:05:37 候选。落盘的 `07.route.json` 已设 `keyboard_press.key=2`，`steps` 和 `page_scenes` 均为空。旧候选的 `Contract::valid()` 仅接受 `key=0`，于是读入路线时返回 `scenario_error`，回放帧为空。通用错误 `07 requires the post-OP route without generic steps` 同时覆盖这项按键契约校验。 [运行结果](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/07.json) · [落盘路线](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/07/07.route.json) · [构建回执](D:/迅雷下载/CS2/archive/2026-11/20261120_220537_815819_story_replay_integration/build.json)

| 主线程下批范围 | 必需配套 |
| --- | --- |
| 07 单独翻页，或用合批清单仅选 07 | 包含 22:06 logical Enter 增量的同源 EXE，以及当前 `key=2` 的 07 路线。 [22:06 增量](D:/迅雷下载/CS2/archive/2026-11/20261120_07_post_op_logical_enter_increment.patch) · [07 单独路线](D:/迅雷下载/CS2/scripts/analysis/pc_story_07_post_op_once.json) |
| 七项合批 `02/04/05/06/07/16/19` | 在 22:06 增量基础上合入 22:08 连续翻页增量，同版支持 02、06、16、19 的 `legacy-*-continue`。 [22:08 增量](D:/迅雷下载/CS2/archive/2026-11/20261120_02_06_16_19_continuation_increment.patch) · [合批清单](D:/迅雷下载/CS2/scripts/analysis/pc_story_02_04_05_06_07_16_19_batch.json) |
| 07 无正文输入探针 | 路线的 `kamikaze_post_op={}`，解析后无 `keyboard_press`，22:05 和合入 22:06 增量的候选均可读取，验收终点为 `com01 0/57`。 [探针清单](D:/迅雷下载/CS2/scripts/analysis/pc_story_07_post_op_receiver_probe.json) |

21:45:39 批次在自然片尾后 f3049 已实测 `Keyboard(13)` 由 region 64 → task 8 以 logical 2、group 0、response 0 唯一消费，同帧根恢复 `[2,0,0,0]`。同帧的 logical 0 对该接收者返回 1。翻页清单保留 `key=2`，主线程下一批以 `com01 1/62` 正文完成并合成为通过标准。 [消费回执](F:/CS2-build-cache/playthrough/2026-11/20261120_214539_755664_j5fiox2__playthrough/07/07.host.log:14) · [根恢复](F:/CS2-build-cache/playthrough/2026-11/20261120_214539_755664_j5fiox2__playthrough/07/07.events.log:33894)

## 接入与回放

补丁位于 `archive/2026-11/20261120_pc_story_07_post_op.patch`。播放器在 native 268 释放时向影片宿主查询同一播放实例的完成状态，释放成功后写出 `movie_naturally_released`。路线核对单次播放和释放、实际装入的原版 CST 与当前正文的 load ID、活接收者、物理按下、根恢复和松键。标题入口先核对遮罩和接收者，必要时只移动鼠标，在悬停确认后重新核对并点击。原始 PE、`sscript.kcs`、`meswnd.fes`、CST 与它们的摘要见 `legacy_story_wait.rs`。输入走现有宿主路径。补丁应用、编译与新候选的回放由主线程在构建窗口安排。

```powershell
Set-Location -LiteralPath 'D:/迅雷下载/CS2'
git apply --reverse --check --whitespace=error 'D:/迅雷下载/CS2/archive/2026-11/20261120_07_post_op_logical_enter_increment.patch'
```

把 `$buildReceipt` 指向接入补丁后生成的同源 `build.json`。

```powershell
$build = Get-Content -Raw -LiteralPath $buildReceipt | ConvertFrom-Json
$player = @($build.artifacts.PSObject.Properties | Where-Object { (Split-Path -Leaf $_.Name) -eq 'story_replay.exe' })
if (-not $build.source_consistent -or $player.Count -ne 1) {
    throw 'Expected one story_replay.exe from a source-consistent build'
}
$candidate = @{ player = $player[0].Name; sha256 = $player[0].Value.sha256 }
if ((Get-FileHash -LiteralPath $candidate.player -Algorithm SHA256).Hash -ne $candidate.sha256) {
    throw 'Replay candidate SHA-256 mismatch'
}
python -X utf8 -B scripts/analysis/run_playthrough_batch.py --player $candidate.player --indices 7 --scenarios scripts/analysis/pc_story_07_post_op_receiver_probe.json --jobs 1 --timeout 1200
```

- **探针通过**需要 `completed`、唯一新游戏点击、见同一影片实例自然完成并释放、随后打印 `07 com01 0/57 body=56` 及其 owner、context、load、epoch、region 和 receiver。需同时见完全自然显字、实际正文和提示合成、局部 `$100=1`。探针模式的正文按键数为零。

探针通过并确认实测为 `region=64 receiver=8` 后，再单独执行。

```powershell
python -X utf8 -B scripts/analysis/run_playthrough_batch.py --player $candidate.player --indices 7 --scenarios scripts/analysis/pc_story_07_post_op_once.json --jobs 1 --timeout 1200
```

- **单键通过**需要同样的入口与首文证据、一次 `Keyboard(13)` down、恰好一条消费回执，logical key 2、group 0、response 0，同帧唯一根恢复 `[2,0,0,0]`。两次 fade 和 wait 自然执行，最后日志出现 `07 com01 1/62 body=61`，确认同一 context/load 和窗口生命周期、新 epoch、`release=observed` 以及 `replay_status=completed`。终点正文输入数仍为一次。
- **接收者负例**可将单键清单中的 `receiver` 改为 12 后独立检查。期望在已经证明的 0/57 打印预期 12、实测 8，并报 `checkpoint_mismatch`，正文按键数为零。旧播放器对新清单会报 `scenario_error`。

把 `$run` 指向单槽输出目录后可按以下顺序查阅原始回执。

```powershell
rg -an '07 OP naturally finished|07 com01|down Keyboard|verified 07 Enter' "$run/07/07.host.log"
rg -an 'movie_(load|naturally_released)|index=268 |probe: Kamikaze|pl [19] fade 40|message-resume.*\[2, 0, 0, 0\]|replay_status=' "$run/07/07.events.log"
```

## 02 / 04 / 05 / 06 的同版观察

```powershell
python scripts/analysis/run_playthrough_batch.py --player $candidate.player --indices 2 5 4 6 --scenarios scripts/analysis/pc_story_02_05_entry_no_keyboard.json --scenarios scripts/analysis/pc_story_04_06_07_wait_observe.json --jobs 1 --timeout 600
```

- **02** 从现有入口移除 f850/f905 的历史键。能自然到 `0_01_01` 时对齐 abs 100/101。若在前序等待停住，取样须写清实际接收窗口和等待原因。旧 `103,104/123` 是历史单键后继。
- **05** 保留三处已知遮罩并删除所有历史键。先记录能否进入 `00_00` 和再进入 `00_01`，到达后对齐 abs 48/49 和 kind 47 正文自然完成。旧 `53/54` 是历史单键后继。
- **04、06** 沿用已知入口遮罩，增加 71、104、105、327 日志。分别对齐 `__tar01` abs 53/54、`C01_01` abs 61/65。06 的 abs 64 为空。若当前候选仍打印 `profile=None`，这两份只读结果用于确认窗口、等待和接收者的注册迹象。
