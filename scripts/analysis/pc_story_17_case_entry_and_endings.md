# 17 白昼梦｜标题进入 CASE 与结局路线

**PC 实跑证据**到 `prologue 40/221`，19:06 候选在 f5687 提交同代正文 r215，r220 为零字节。19:34 候选在 f5688 消费该页唯一物理 Enter 并恢复 VM，随后转场检查在 f6239 停住。标题停点、标题点击与 CASE 正文仍待实跑回执。本文的分支和结局来自本机资源的离线核对。以下 `g/r` 为 CST 的组号和全场景记录号，FES 行号从 1 开始。 [r221 回执](D:/迅雷下载/CS2/archive/2026-11/20261120_daydream_r221_receipt.json) · [序章转场证据](D:/迅雷下载/CS2/scripts/analysis/pc_story_17_prologue_exit_evidence.json)

19:34 批次的 f6206 已执行 `41/237 initialize_header → 41/238 view 0 default`。`config.int::meswnd.xml` 的 `default/mes/script` 加载 `meswnd.fes`，f6239 的创建事件确认消息窗归根任务 8。此时是 `flow/GAME_WAIT` 与 `meswnd/WAIT`，两者 `WAIT_KEY=false`，当前 CST 候选为空，r221 是历史标记。增量补丁仅在已验证 r221 输入、同一 CST load、观测到 `view 0 default` 和上述活窗口同时成立时让观察继续。之后按原有标题三层等待和 `btn1[0]` 检查收口。 [f6206 命令](F:/CS2-build-cache/playthrough/2026-11/20261120_194052_258849_zs73ddno_playthrough/17/17.events.log:84121) · [f6239 窗口诊断](F:/CS2-build-cache/playthrough/2026-11/20261120_194052_258849_zs73ddno_playthrough/17/17.host.log:372) · [增量补丁](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_r221_tail_guard.patch)

21:10 批次确认 f6239 的 r238 回执。f6240–f6241 同一 owner 13 执行 `meswnd.fes::MES_SHOW` 的布局和 `keywait anm/disp`，两层 FES 仍为 `WAIT_KEY=false`，VM 地址为 `0xd6d`，CST 仍是 context 159、load 1、旧 `40/221`，候选为空。旧 guard 在枚举窗口时只允许默认 `meswnd/WAIT`，因此在 f6241 报错。增量允许同一 r238 创建记录下的 `MES_SHOW` 继续无输入观察，仍要求根任务、CST 身份、VM 等待、无挂起回调和无正文等键。 [f6239–f6241 诊断](F:/CS2-build-cache/playthrough/2026-11/20261120_211052_419283_uibxfbv7_playthrough/17/17.host.log:370) · [f6241 FES 事件](F:/CS2-build-cache/playthrough/2026-11/20261120_211052_419283_uibxfbv7_playthrough/17/17.events.log:84721) · [r238 修复](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_r238_mes_show_guard.patch)

22:08 批次在 f6239 确认同一条 r238 回执，随后越过 f6241，到 f6249 才由 guard 停止。f6248 和 f6249 都是同一套 `meswnd/MES_SHOW` 显隐、`keywait anm/disp` 与 `native 71` 等待。f6249 仍只有 flow 6 和 meswnd 13，两层 `WAIT_KEY=false`，根地址 `0xd6d`，无挂起回调，CST 仍为 context 159 的旧 `40/221` 标记且无候选。f6239 之后到停点未记录新任务创建、菜单、CST load 或 r242 `fesds`。 [r238 命令](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/17/17.events.log:84122) · [owner 创建](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/17/17.events.log:84666) · [f6249 状态](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/17/17.host.log:372)

旧 guard 从每帧截断的宿主诊断缓冲反查 r238 命令和创建记录。该依赖与 f6249 在活状态相同的情况下误停吻合，22:08 的导出日志未记录缓冲裁剪帧。增量将两项事件存入 r221 的待确认动作，并继续核对活任务、CST load、VM、窗口等待和菜单条件。复跑以越过 f6249、标题三层等键成立为准。若再入 `unexpected_branch`，停止行带缓存的 r238 三元组和当时最早的缓冲帧。 [f6249 增量](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_r238_tail_evidence.patch)

22:31 的 CASE 3 END1 场景已越过 f6249。f6272 首次继续执行序章 `41/239–242`，三条粒子停止命令后是 `fesds`。f6273 开始按 r254–260 恢复默认图层，日志中的 plane 27–58 是根任务 8 下的 kind 2 对象。活消息窗仍是 r238 的 owner 13，FES 到达 `MES_SETSTATE`，且 `WAIT_KEY=false`。此段 FES 资源的 670–686 行设置按钮状态并调用 `SETBUTTON`。旧 guard 的消息窗状态列表在这里拒绝继续。停帧仍保留 CST 159、load 1 的旧 r221 标记和根 VM 等待 `0xd6d`。 [r242 命令](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/17/17.events.log:85499) · [默认图层](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/17/17.events.log:85565) · [f6273 活状态](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/17/17.host.log:373)

独立增量要求在 r238 创建回执之后观测到 r242，才允许同一活窗口的 `MES_SETSTATE` 继续无输入观察。标题仍由三层等键和活 `btn1[0]` 核验。CASE 3 END1 仍以片尾自然结束和进入 `c99_10` 时的根旗标核验。 [r242 修复](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_r242_mes_setstate_guard.patch)

## r242 之后的实跑判据

23:11 的 CASE 3 END1 场景已越过 f6273。f6276 执行 `41/272 drawdef → 273 wipedef → 274 wait`，f6277 记录 `GetLine error!! [prologue(41,53)]`。组 41 共 53 条记录，组内索引 53 正好是末条之后，批次 `first_engine_error=null`。f6278 的活窗口首次记录 `flow/START_TITLE`，同一 owner 13 仍在 `meswnd/MES_SETSTATE`，两者 `WAIT_KEY=false`，且保留 r238、r242、CST 159/load 1 的旧 r221 标记及根 VM `0xd6d`。 [末条命令](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/17/17.events.log:85676) · [组末读取](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/17/17.events.log:85697) · [f6278 活状态](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/17/17.host.log:373)

本次增量在同一 r238 窗口的 r242 回执之后保留精确的 `prologue(41,53)` 记录，只允许已观察到的 `START_TITLE + MES_SETSTATE` 组合继续无输入等待。下一批仍以三层标题等待和活 `btn1[0]` 作为标题输入门槛。 [归档补丁](D:/迅雷下载/CS2/archive/2026-11/20261120_daydream_r274_start_title_guard.patch)

23:40 批次通过 f6278，f6279 首次见 `contexts=[]`。`flow/START_TITLE` 和同一 owner 13 的 `meswnd/MES_SETSTATE` 仍在等帧，两层 `WAIT_KEY=false`，根 VM 仍在 `0xd6d`。增量先核对 r238 默认窗口、r242 和组末的时序及活窗口条件，再允许这个组合在 CST 已释放时继续无输入观察。正常标题就绪要求空 CST、`flow → ttlflow → title` 活父链和原有的三层等键及 `btn1[0]` 绑定。引擎以执行 `execfes` 的 FES task 创建子任务，父链来自游戏资源与该创建逻辑，回标题后的实际 owner 待本批后继实跑记录。 [f6279 诊断](F:/CS2-build-cache/playthrough/2026-11/20261120_234020_622897_k1wd750v_playthrough/17/17.host.log:373) · [FES 子任务创建](D:/迅雷下载/CS2/simulator/crates/desktop/src/fes_tasks.rs:2709) · [归档补丁](D:/迅雷下载/CS2/archive/2026-11/20261121_daydream_f6279_title_handoff_guard.patch)

| 阶段 | 复跑时核对的证据 |
| --- | --- |
| 默认消息窗收尾 | 对照 `41/243–253` 的停音和清理，以及 `254–271` 的图层恢复、色调和等待。复用 r238 的 owner、parent、context/load，保留 `WAIT_KEY=false`、根等待 `0xd6d` 和无新 CST 候选的检查。`meswnd.fes` 265–266 提供 `WAIT` 的资源依据，635–662 与 670–686 提供 `MES_SHOW` 和 `MES_SETSTATE` 的行为依据。 |
| 转交标题 | 依次找 `41/272 drawdef → 273 wipedef → 274 wait`。`flow.fes` 67–69 在子任务退出后进入 `START_TITLE`，26–32 执行 `endfes/endkcs → wait 1 → execfes ttlflow → TITLE_WAIT`。`ttlflow.fes` 30–37 启动 `title` 后才到 `FES_WAIT`。如在窗口交接期间停下，先核对 r274 与旧消息窗是否退出、活任务父链和三个 FES block。 |
| 标题可输入 | `title.fes` 120–121 在 `$542=1` 时进 `IN2`。正常入口由 `SETBTN00` 显示并启用 `btn1[0]`，256–284 清 `$541` 后进入 `WAIT`。当 `flow/TITLE_WAIT`、`ttlflow/FES_WAIT`、`title/WAIT` 同帧稳定，且当代按钮启用、可见、命中区域归 title owner，才允许唯一标题左键。`IN2` 与 `SETBTN00` 的画面只用于定位阶段。 |

这些是资源与现行 guard 的交叉核对。若出现未登记的 FES block、`WAIT_KEY=true`、不同 owner/load 或新的菜单，保留停止结果，记录首个改变的帧、最后一条 CST 命令、活窗口的 block 和等待对象。预检的 JSON 提供两种语言一致的 r242–274 命令序列和四份 FES 的资源哈希及关键行。 [离线证据生成器](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case_evidence.py) · [标题 guard](D:/迅雷下载/CS2/simulator/crates/desktop/src/story_trace.rs:986)

## 从标题停到首个实际正文等键

00:10 批次 f6281 创建 `ttlflow`，f6285 创建 title owner 10 与 `btn1[0]` 的 region 15，f6318 开始显示按钮，f6339 三层 FES 均进入标题等待。旧回放在读取 `InputButtonState.rectangle=[0,0,0,0]` 时退出。FES 每帧把绘制位置和尺寸写入活输入区域表，类型六按钮缓存只在首次绑定或相应原生命令更新。该批未读取活表矩形，实际边界待复跑记录。增量在三层 FES 完成初始化后读取同一 `btn1[0]` 的唯一活区域。只有边界为正、中心的实际掩码命中指向同一区域且该区域在鼠标按下候选中，才记标题检查点并按路线发唯一左键。等待超时保留缓存矩形、活表矩形和命中探针。 [标题创建](F:/CS2-build-cache/playthrough/2026-11/20261121_001028_352834_hso1dr1h_playthrough/17/17.events.log:85761) · [按钮显示](F:/CS2-build-cache/playthrough/2026-11/20261121_001028_352834_hso1dr1h_playthrough/17/17.events.log:85928) · [f6339 停点](F:/CS2-build-cache/playthrough/2026-11/20261121_001028_352834_hso1dr1h_playthrough/17/17.host.log:371) · [区域投影](D:/迅雷下载/CS2/simulator/crates/desktop/src/fes_task_platform.rs:1991) · [归档补丁](D:/迅雷下载/CS2/archive/2026-11/20261121_daydream_title_live_region_guard.patch)

1. 用 r221 转场场景在标题停住。日志应见 `flow/TITLE_WAIT → ttlflow/FES_WAIT → title/WAIT`，根状态 `$542=1`、`$541=0`。用当前 title owner 的按钮表定位 `btn1[0]`。确认按钮启用、可见，活 region 的父任务及输入 receiver 都是该 owner。接续批次以同样的标题等键为前置。 [标题停点场景](D:/迅雷下载/CS2/scripts/analysis/pc_story_17_r221_to_title_observe.json) · [标题等键检查](D:/迅雷下载/CS2/simulator/crates/desktop/src/story_trace.rs)
2. 仅对这个活按钮发一次物理左键，核对当前命中、消费回执和 `#btn1.PUSH_L` 处理器进入第 0 项。`fes.int::title.fes` 602–607 跳入 `GO_STORY`，405–418 写 `strvar 200 start` 后 `exit 0`。`ttlflow.fes` 37–40、84–98 接管返回，`flow.fes` 32–35、60–69 启动 `sscript` 并进入 `GAME_WAIT`。 [FES 行与资源哈希](D:/迅雷下载/CS2/scripts/analysis/pc_story_17_prologue_exit_evidence.json)
3. 首次本篇按新档 `#399=0` 核对。`update02.int::start.cst`、`c00_00.cst` 各在 r0 跳到下一场景，再由 `brunch.cst` r0 选 `brunch_A`。`brunch_A` r0 执行 `rand 299 1 3`，r1–3 分别去 `c01_01`、`c02_01`、`c03_01`。三者各在 r0 跳到同编号的 `_01_01`。用日志中的分支结果或实际加载顺序确定走到哪一项。本段资源在标题后到首正文前均无 `fselect`。 [CST 跳转与资源哈希](D:/迅雷下载/CS2/scripts/analysis/pc_story_17_prologue_exit_evidence.json)
4. 自然等待新 CST 的同一次 load 和当前消息窗完成正文。记录实际 owner、context/load、正文来源记录、自然显完帧及合成提交帧，确认 `0x02` 等键仍成立，期间无额外输入、无活动菜单。到任一表中终点停住。若日志先出现选择窗、其他输入等待或资源与目标不符，停在观察点并记下帧、场景、窗口和候选命中。 [离线首正文检查](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case_evidence.py)

| `#299` | 加载场景与首正文等键 | 同次 load 的正文记录 | 最后一次 `view` 与配置消息窗 |
| --- | --- | --- | --- |
| 1 | `c01_01_01 0/66` | `[65]` | r62 `cinema` → `cinema.fes` |
| 2 | `c02_01_01 0/66` | `[64,65]` | r48 `case02` → `meswnd.fes` |
| 3 | `c03_01_01 0/62` | `[60,61]` | r50 `cinema` → `cinema.fes` |

繁中首正文来自 `update03.int`，日文来自 `update02.int`，两版上述 `g/r` 与正文记录一致。`config.int::meswnd.xml` 84–91 明确 `case02/mes/script=meswnd.fes`，174–181 配置 `cinema.fes`。CASE 2 的 r63 是 `0x21` 人名。本次增量对 CASE 2 的 `c02_01_01 0/66` 使用 `meswnd` 等键探针，要求活 owner、当代正文与等键图层均有画面提交证据。 [消息窗映射检查](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case_evidence.py)

### 17 专用回放场景

`pc_story_17_title_to_first_case.json` 从新隔离档复用已确认的序章链。r221 的唯一 Enter 释放后，先核对 `view 0 default` 与同代 `meswnd` 创建，再等标题三层 FES 与 `btn1[0]` 的活区域。点击点取当前区域矩形中心，日志记录区域、矩形、掩码命中和候选。中心未命中时继续无输入等待并保留超时诊断，出现竞争回执、选择菜单或路线外的场景时停止。三条 CASE 都停在正文已自然显完并合成的首个 VM 等键。 [场景](D:/迅雷下载/CS2/scripts/analysis/pc_story_17_title_to_first_case.json) · [共用 Rust 增量补丁](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case_start_player.patch)

主线程在合入 r221 转场 guard 的源码上核对并应用增量，用包含该补丁的集成 EXE 调度 17。

```powershell
Set-Location 'D:/迅雷下载/CS2'
$daydreamPatch = 'D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case_start_player.patch'
git apply --check $daydreamPatch
git apply $daydreamPatch
python -B 'D:/迅雷下载/CS2/scripts/analysis/generate_daydream_prologue_exit.py' --check

$daydreamPlayer = '<本次集成 story_replay.exe 的绝对路径>'
python -B 'D:/迅雷下载/CS2/scripts/analysis/run_playthrough_batch.py' --player $daydreamPlayer --indices 17 --jobs 1 --scenarios 'D:/迅雷下载/CS2/scripts/analysis/pc_story_17_title_to_first_case.json'

$daydreamRun = '<17 批次目录或 17.json 的绝对路径>'
python -B 'D:/迅雷下载/CS2/scripts/analysis/check_daydream_prologue.py' --to case $daydreamRun
```

通过时检查器输出 `status=verified`，附 r238 窗口创建、唯一标题左键和 `terminal_case`。正文等键的记录、来源和消息窗必须与上表一致，`body_inputs=0`。00:10 批次 f6339 已到三层标题等待，活区域探针和 CASE 尚待复跑。 [日志检查器](D:/迅雷下载/CS2/scripts/analysis/check_daydream_prologue.py:248)

### 首正文再走一页

| 场景 | 相邻正文等键 | 新正文来源 | 中间命令 |
| --- | --- | --- | --- |
| `c01_01_01` | `0/66 → 1/68` | `[67]` | 无 |
| `c02_01_01` | `0/66 → 1/71` | `[70]` | `rdraw`、`pcm K_c02_01_002`，r69 为人名 |
| `c03_01_01` | `0/62 → 1/65` | `[64]` | `se 0 se4029` |

日文和繁中资源的相邻等待、命令和正文记录一致。新增场景复用原有标题到首正文的整段探针，只在首正文自然显完、同代合成、相邻记录核验通过且实况确认根任务的 `bottom button` 时按一次物理 Enter。回执须为唯一 `key=2/group=0/response=0` 消费及同帧 VM 恢复 `[2,0,0,0]`。第二页须沿用同场景、context、load 和窗口 owner，以新正文代自然显完并合成，终点零输入。 [相邻资源校验](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_prologue_exit.py:232) · [CASE 后继补丁](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case_successor_player.patch)

合批后换用 `pc_story_17_first_case_advance.json` 和检查器参数 `--to case-next`。 [合批场景](D:/迅雷下载/CS2/scripts/analysis/pc_story_17_first_case_advance.json) · [单步回执检查](D:/迅雷下载/CS2/scripts/analysis/check_daydream_prologue.py:333)

## 本篇两轮与 CASE 0

以下使用同一次正常本篇流程，初始按新档 `#399=0`，阶段完成旗标和标题重看标记均为 0。标题 `btn1[6..8]` 分别在 `$1001..1003` 成立时可用，跳 `replay_01/02/03` 并设置 `#212/#222/#232`，进入对应 CASE 时再置 `#213/#223/#233`。`btn1[9]` 在 `$1000` 成立时可用，走 `replay_00` 并写 `#202=1`。正常本篇一律沿 `btn1[0]` 起步。CST 存档进度和 FES 菜单状态均以当次日志为准。

| 阶段 | 从资源复核分流 |
| --- | --- |
| `#399=0` | `brunch_A` 随机进入任一 CASE 的前半段。 |
| `#399=1` | `brunch_B` r0–2 按已经完成的 `#214/#224/#234` 跳 `brunch_B_23/_13/_12`。每个子场景重新抽 `#299`，抽到已完成 CASE 就回到该子场景再抽。 |
| `#399=2` | `brunch_C` r0–2 对比 `#214==#224`、`#214==#234`、`#224==#234`，在恰有两个 1 的可达状态分别指向剩余的 CASE 3、2、1。 |
| `#399=3/4/5` | `brunch_2 0/11` 才出现真正的 CASE 选择窗。r12–14 分别在 `#215/#225/#235==0` 时列出选项 `10/11/12 → c01_04/c02_04/c03_04`。每完成一个 END1，该选项消失。 |
| `#399=6` | 第三个 END1 后 `c99_16 83/476 → c00_op`。`c00_op` 播 `op_c00.mpg`，`c00_op_x 0/0..4` 写 6、清当前 CASE 标记，跳 `c00_01`。 |

三部 CASE 前半段的完成写入分别在 `c01_09_10 244/1121 → #214=1`、`c02_03 268/1611 → #224=1`、`c03_03_03 103/706 → #234=1`。本篇沿 `c99_try` 继续。前两次经 `c99_try_01 → c99_try_00`，后者 r0–9 重置当前 CASE `#211/#221/#231`，按已完成的组合把 `#399` 改为 1 或 2，再回 `brunch`。第三次经 `c99_try_02 → c99_try_a/b/c → c99_try_x → c99_fin`，`c99_fin 0/0..4` 把 2 改为 3 并回 `brunch`。这段是在同一轮本篇里自然串接。

前半段中盘还有对话菜单。`c99_02_a_01/b_01/c_01` 的 `0/1` 是 `fselect`，退出本轮问答选 `3/6/9 → c99_02_x_01`。其余可见选项进短段后回到原菜单。`c99_02_c_01` 的选项 8 还要求 `#290=1`，可由选项 7 的目标 `c99_02_c_01_a 12/78` 解锁。离线检验走 `3/6/9`。这些编号是资源选项身份，`config.int::meswnd.xml` 将 `fselect` 配给 `select.fes`，该脚本 271–278、232–240 用实际按钮处理器返回显示中的索引。回放需先核对当次选项表、活 `select` owner 和命中，再确认返回值对应的目标 CST。

| 结局 | 可核对的片尾和旗标 |
| --- | --- |
| CASE 1 凛 END1 | `c01_06_h 522/2370 → c01_ed`，`c01_ed 0/1` 播 `ed_c01.mpg`，`c01_ed_x 0/0..2` 写 `#760/#1001/#215=1`。 |
| CASE 2 奥莉维娅 END1 | `c02_06_h 133/620 → c02_ed`，`c02_ed 0/1` 播 `ed_c02.mpg`，`c02_ed_x 0/0..2` 写 `#760/#1002/#225=1`。 |
| CASE 3 桃乃内 END1 | `c03_06_h 176/877 → c03_ed`，`c03_ed 0/1` 播 `ed_c03.mpg`，`c03_ed_x 0/0..2` 写 `#760/#1003/#235=1`。 |
| CASE 0 世凪 TRUE END1 | `c00_12 644/3123 → c00_ed`，`c00_ed 0/1` 播 `ed_c00.mpg`，`c00_end 72/405..407` 在尾声写 `#1000=1`、清 `#202` 并执行空参数 `next`。 |

三条 `c0N_ed_x` 的 r4–6 按 `#399=3/4/5` 汇入 `c99_10/c99_13/c99_16`。`c99_10 31/181..185` 写 4 并回菜单。`c99_13 → c99_c → c99_13_x`，后者 `48/254..258` 写 5 并回菜单。最后由 `c99_16` 去 CASE 0。END1 的日志以当次片尾实例自然结束、后继 CST 和对应旗标为准。TRUE END1 再核对标题重新稳定和 `$1000` 对应入口启用。

### 首次正式结局回放

00:24 批次使用 00:23 候选。f6339 的 `btn1[0]` 活区是 `[537,274,743,325]`，中心 `[640,299]` 首个掩码命中 region 15，两个启用位均为 1。f6340 的唯一左键回执由 region 15 交给 title owner 10。随后自然加载 `start → c00_00 → brunch → brunch_A → c01_01 → c01_01_01`。f6459 的首错是 `native 1037 input=1 record=0` 严格 Cp932 解码拒绝。离线取到的 `update03.int::c01_01_01.cst` 同一组内索引映射为绝对 r67，GBK 校验通过。 [标题探针与回执](F:/CS2-build-cache/playthrough/2026-11/20261121_002446_022100_phfaym7b_playthrough/17/17.host.log:372) · [首错事件](F:/CS2-build-cache/playthrough/2026-11/20261121_002446_022100_phfaym7b_playthrough/17/17.events.log:86464)

日期归档增量在资源挂载时验证白昼梦 `update03.int` 的大小和整包 SHA-256。只有来源为这一实际归档的 CST 走已有的严格 GBK 加载及发布流程。`update03.int` 的 196 个 CST 成员离线通过严格 GBK 校验，其中 113 个在首个 CASE 3 END1 的六种路径内。运行时继续逐记录验证。下一候选用同一 END1 场景，在 CASE 1 分流核对 `cinema 0/66` 的同代合成，单次受证据约束的 Enter 后核对 `cinema 1/68`。CASE 2/3 依各自窗口和记录验收。 [GBK 归档补丁](D:/迅雷下载/CS2/archive/2026-11/20261121_daydream_update03_cst_gbk.patch)

新档路线选择桃乃内 CASE 3 END1。前三段按游戏抽签覆盖六种顺序，按每页的实际等键、正文来源与合成回执推进。三处问答按各场景所属选项取 `3/6/9 → c99_02_x_01`。前三段完成且 `#399=3` 后，核对 `brunch_2` 的活 `select.fes`，只点选项 12 进入 `c03_04`。从 `c03_06_h 176/877` 进入 `c03_ed`，等待同一 `ed_c03.mpg` 实例自然释放，沿 `c03_ed_x` 在 `c99_10` 首次加载处停止。终点根变量要求 `#760=#1003=#235=1`、`#399=3`、`#214=#224=#234=1`、`#215=#225=#1000=#1001=#1002=0`，终点发送零按键。 [场景](D:/迅雷下载/CS2/scripts/analysis/pc_story_17_case03_end1.json) · [CASE 3 END1 增量](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case03_end1_player.patch) · [生成与验收](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case03_end1.py)

00:37 批次确认 GBK 首错已过。f6459 的 CST context 35369 刚载入 `c01_01_01`，未提交候选为 `(6459, 0x02, 2, 70)`，已标记正文页为空。活任务为 `flow/GAME_WAIT` 与 `meswnd/MES_SETSTATE`，两者 `WAIT_KEY=false`，根 VM 等待 `0xd6d`，无 CASE 按键。原 guard 将这个候选当作分歧。增量仅在 CASE 1 首个 Enter 尚未发出、未标记正文页、候选来自本次加载帧且精确符合 `2/70` 时忽略该候选。换帧重新查询、正式页标记和 FES 菜单仍按原条件判断。点击和正文推进继续要求原有回执与同代合成。 [停帧诊断](F:/CS2-build-cache/playthrough/2026-11/20261121_003709_063298_j4j2sjks_playthrough/17/17.host.log:380) · [归档增量](D:/迅雷下载/CS2/archive/2026-11/20261121_daydream_case_entry_load_probe_guard.patch)

00:48 批次已经核对 CASE 1 `0/66 → 1/68`。f6770 的首正文自然合成后，f6771 唯一 Enter 恢复根 VM，f6858 在同一 context 35369、load 7 确认 `1/68`。END1 接管后继续逐页推进，最后一次 Enter 是 f7958，对应 `11/93`。随后 `view case01 / view 0 case01` 切入 `meswnd.fes`，f8154 出现 `12/111` 的真实等待，f19957 以 `unrecognized_wait` 停止。此时 `meswnd` owner 30 的 `MES_SHOW`、`WAIT_KEY=true`、同 load 的 r110 来源均已核对，`composited=false`。 [正文及 Enter 回执](F:/CS2-build-cache/playthrough/2026-11/20261121_004847_358673_adbxvzks_playthrough/17/17.host.log:379) · [切窗命令](F:/CS2-build-cache/playthrough/2026-11/20261121_004847_358673_adbxvzks_playthrough/17/17.events.log:110385) · [r111 停帧诊断](F:/CS2-build-cache/playthrough/2026-11/20261121_004847_358673_adbxvzks_playthrough/17/17.host.log:455)

旧 `body_generation` 诊断对 Daydream 查找 `cinema`，没有报告实际 `meswnd` 代次。资源中 `meswnd.fes` 635–663 的 `MES_SHOW` 显示正文层 `str`，同时第 659 行执行 `keywait disp 0`。共享合成门槛曾要求这两个图层都提交可见像素。增量仅在 `c01_01_01 12/111` 的已确认 `meswnd` 等键、实际 `keywait` 隐藏时允许正文独立提交。正文自然完成、同代提交、来源和活键绑定仍必查。新增诊断分别记实际代次、提交、正文及提示像素、提示可见性与 Enter 绑定，区分提示隐藏与正文提交失败。 [布局守卫增量](D:/迅雷下载/CS2/archive/2026-11/20261121_daydream_case01_r111_meswnd_submission.patch)

复跑沿现有 END1 路线。验收依次是 `Daydream END1 body ... group=12 record=111`、一次物理 Enter 及同帧 VM 恢复 `[2,0,0,0]`、同 load 自然执行 r112–119（包括 r115 `rdraw`），最后确认 r120 来源自然合成并进入 `13/121`。r115 是绘制命令，等待记录为 r121。 [END1 场景](D:/迅雷下载/CS2/scripts/analysis/pc_story_17_case03_end1.json)

后续 CASE 复用同一提示规则。首个 END1 的六条资源路径中，`case01`、`case02`、`case03`、`default` 的 `view` 均指向同一 `meswnd.fes`。日文与繁中覆盖中，前三类命令分别出现 35、18、28 次。比如 `c01_01_01` 在 r102、r105 切 `case01`，到 `13/121` 之间没有新 `view`，后续还有 r260 的同类切换。通用守卫核对同一活 owner、`MES_SHOW` 程序内的 `keywait disp 0` 和实际隐藏状态。CST 来源、正文自然完成与同代提交、正文像素及 live Enter 仍由原路径验证。停点诊断用 `daydream_meswnd` 报告这些值。 [通用提示守卫](D:/迅雷下载/CS2/archive/2026-11/20261121_daydream_meswnd_hidden_wait_guard.patch)

01:31 回执确认任务号 30 从 `cinema` 换成了 f8121 新建的 `meswnd`，epoch 随实例从 52 重置到 2。01:51 复跑在 f8216 接受该创建事件，f8217 唯一 Enter 恢复 VM。同一 CST context 35369、load 7 在 f8276 确认 `13/121`，f8360 确认 `14/123`。连续实例仍比较 epoch。 [生命周期增量](D:/迅雷下载/CS2/archive/2026-11/20261121_daydream_end1_body_window_lifetime.patch) · [01:51 复跑日志](F:/CS2-build-cache/playthrough/2026-11/20261121_015136_430417_3rdory9m_playthrough/17/17.host.log:453)

### 普通槽恢复验收

普通保存入口可用独立模式生成。它沿 END1 的正常路径到 `c01_01_01 13/121`，通过正文与同代合成门槛后，才探测活区域中的 `meswnd btn[6]`。按钮身份、接收者、正面积、实际掩码和鼠标候选均匹配时发一次左键。只有该左键有匹配回执、原 CST 和 r120 来源保留、`saveload/WAIT` 已进入普通 Save 页 0、槽 0 为空且 `btn_data[0]` 可点击，才记入口检查点。该场景在空槽处结束，写槽与冷读另验。 [保存入口增量](D:/迅雷下载/CS2/archive/2026-11/20261121_daydream_case01_ordinary_save_entry.patch) · [场景生成器](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case03_end1.py)

```powershell
$daydreamSaveRoute = '<主线程本次批次的 Save 场景 JSON 绝对路径>'
python -B 'D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case03_end1.py' --save-entry |
    Set-Content -Encoding ASCII -LiteralPath $daydreamSaveRoute
```

`fes.int::cinema.fes` 266–270、309–320 和 `meswnd.fes` 286–290、329–340 都将 `btn[6]` 绑定到普通 Save、`btn[7]` 绑定到普通 Load。写槽首选在已验证的 CASE 正文等键，核对实际窗口的 Save 按钮、候选命中及左键回执。`saveload.fes` 546–563 用页号 `\100` 和 `btn_data` 下标算普通槽号 `\100*12+@`，写入模式 `\110=0`。首个候选用页 0 的空槽 `btn_data[0]`，按实际区域和接收者核对。等 809–823 行的 `datasave \102 _SAVEOBJ_` 获得唯一成功回执并正常退出，记 `save0000.dat` 的 CSS 完整性、长度和 SHA-256。

冷读使用单独的私有进程，只预置同一普通槽。`title.fes` 602–610 走 `btn1[1] → GO_LOAD`，`ttlflow.fes` 160–170 创建 `saveload` 并等待返回槽号。普通读槽模式 `\110=1`，`saveload.fes` 824–849 还要求槽存在并可能按 `$981` 进入确认。只有实况确认该菜单和活确认控件后才输入。验收要求成功读取相同槽、恢复原 CASE 场景和正文记录、重新自然完成并合成，随后以一次已核对的物理 Enter 到下一条合法等键。恢复后的 context 和 owner 以冷读进程为准。00:37 私档没有普通槽，当前共享清单也没有已批准项。 [普通槽批处理约束](D:/迅雷下载/CS2/scripts/analysis/run_playthrough_batch.py:122) · [普通槽清单](D:/迅雷下载/CS2/scripts/analysis/pc_ordinary_saves.json)

普通档往返候选 v2 复用原空槽探针。写端点击一次空槽，要求唯一物理回执和唯一成功 `fes-datasave`，确认回到同一 Save 面板并记录 CSS 摘要。冷读阶段从已验写端复制 `save0000.dat` 及同批次产生的系统初始化文件 `savegen.dat` 到新私档。正常标题要求 `$542=1`，实际 Load 面板按 `$981` 决定是否等待 `conf/WAIT` 的活 `btn[0]`。读端核对 native 247 打开的摘要、`13/121` 的 r120 来源及自然合成、唯一物理 Enter 和同一 CST load 中 `14/123` 的 r122 来源及自然合成。私档目录新建且独占，可保留回执后单独清理，写档批次可用于重新暂存。 [v2 归档增量](D:/迅雷下载/CS2/archive/2026-11/20261121_daydream_case01_ordinary_roundtrip_v2.patch) · [往返验收脚本](D:/迅雷下载/CS2/scripts/analysis/check_daydream_save_roundtrip.py)

## EPILOGUE、TRUE END2 与 BONUS SCENARIO

- `fes.int::title.fes` 245–254、275–280 以 `$1000` 控制 `btn1[5]`（EPILOGUE）与 `btn1[9]`（CASE 0 重看）。`btn1[5]` 走 629–631 → `GO_EPG`，571 写 `strvar 200 c00_ep`。`c00_ep → c00_h_03 → c00_ep_h → c00_ep_123` 才到组 0 的选择窗。`c00_ep_123` r1 为 `fselect`，r2–4 按 `#216/#226/#236==0` 显示 `13/14/15 → c00_ep_01/02/03`。
- 凛 END2 在 `c01_ep 411/2016..2018` 写 `#1014/#216=1`，接 `c00_ep_01_x`。奥莉维娅 END2 在 `c02_ep 152/1012..1014` 写 `#1012/#226=1`，接 `c00_ep_02_x`。桃乃内 END2 的终点在 `c03_ep_h2 21/174..176`，写 `#1013/#236=1`，接 `c00_ep_03_x`。相应 `_x` 先检查 `#216 + #226 + #236 == 3`，达到时都进 `c00_ep_123_x`。CASE 3 若最先完成，`c00_ep_03_x` 在 `#216==#226` 时先经 `c00_ep_03_y` 再回菜单。
- 世凪 TRUE END2 走 `c00_ep_123_x → c00_h_04 → c00_ep_ed`，`c00_ep_ed 0/1` 播 `ed_c00_ep.mpg`。`c00_ep_ed_x 0/0..3` 写 `#1005=1` 并清 `#216/#226/#236`。之后再进 EPILOGUE，`c00_ep_123 0/5` 另列选项 16，可直达 `c00_ep_123_x`。
- `SPECIAL` 由 `title.fes` 223–227、272–274 的 `$760` 控制，首个 END1 写入该旗标。`btn1[3] → OMAKE/SETBTN02`，然后 `btn3[4] → GO_EX_OMK / exit 23 → ttlflow → flow/EX_OMAKEMODE → omake.fes`。`omake.fes` 86–96、223–241 按对应角色的 END2 旗标接受 BONUS SCENARIO 按钮。三个角色 END2 都完成时五项已具备解锁条件，结束整条 EPILOGUE 并回标题即可按攻略复核全部。 [随附攻略](D:/迅雷下载/CS2/games/白昼夢の青写真/日文攻略.txt)

| BONUS SCENARIO | `omake.fes` 按钮身份 | 条件 | 场景 |
| --- | --- | --- | --- |
| CASE 1 上／下 | `btn[0]`／`btn[1]` | `$1014=1` | `c01_h_03`／`c01_h_04` |
| CASE 2 上／下 | `btn[2]`／`btn[3]` | `$1012=1` | `c02_h_02`／`c02_h_04` |
| CASE 3 | `btn[4]` | `$1013=1` | `c03_h_03` |

## 离线复核

```powershell
python -B 'D:/迅雷下载/CS2/scripts/analysis/generate_daydream_prologue_exit.py' --check
python -B 'D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case_evidence.py'
```

第二条输出 `offline_verified`。按 CST 控制记录验证首轮 CASE 的 6 种顺序与 END1 的 6 种选择顺序交叉共 36 组，另验首次 EPILOGUE 的 6 种顺序和完成后选项 16。繁中覆盖资源与日文资源的相关控制记录须一致。`question_branches` 列出问答的可见选项、回环目标和选项 8 的解锁顺序。`ending_checkpoints` 列出三种首个 END1 在 `c99_10` 入口的根旗标，另核对第二、三个 END1 入口的累计旗标、各 END2 的回写及 TRUE END1/2 的终点。`title_replays` 列出四个标题重看入口的解锁条件和首个真实 CST 后继。检查使用符号化抽签和菜单选择，实际影片自然释放和点击仍由实跑日志验收。 [离线检查代码](D:/迅雷下载/CS2/scripts/analysis/generate_daydream_case_evidence.py)

主线程拿到标题批次后，可以把批次目录或其中 `17.json` 传给 [标题回执检查器](D:/迅雷下载/CS2/scripts/analysis/check_daydream_prologue.py) 的 `--to title`。成功时会给出 `terminal_title` 的帧、三层 owner、活 region 和 `title_inputs=0`。完整标题单击和 CASE 停点使用上面的 `--to case` 核对。
