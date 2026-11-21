# 09／20 首段回执与 12 标题观察

以下日志位于 `F:/CS2-build-cache/playthrough/2026-11`。帧号用于定位已存证据。路由根据当场状态放行输入。

## 18:25 候选的 09 标题复跑

`20261120_185401_182700_di10nlsc_playthrough/09` 在 f314 已到活的 `flow(6)/TITLE_WAIT`、`title(8)/WAIT` 与根 native 71 `0xcae`。路由的启用 region 14 命中检查已通过。随后专用标题目标检查拒绝，零输入。旧成功基线 f380 的 `(140,354)` 命中数组为 `[14,220,7,74,341,318,375,1,1,1,0,0,0,0,1]`，候选为 `[3,14,23,7]`。数组下标 11 为 0，表示矩形命中。旧守卫要求这一位非零且存在位图注册，排除了已成功点击的目标。f314 日志没有记录完整数组，正反配置现在会在动作前打印同帧命中、候选和活接收者。

基线 f381 的物理左键先由 region 3／receiver 0 返回 1，再由 region 14／receiver 8 返回 0。根同帧在 `0xcae` 恢复 `[2,140,354,0]`。原守卫要求只有一条按下回执，也会拒绝这条已证实的传播链。离线补丁 `archive/2026-11/20261120_09_title_hit_and_receipt.patch` 使用输入 API 已核验的命中，仅在 09 允许该前置传播。唯一消费、根恢复、资源身份、`$541=0` 和 `btn1[0]` 活绑定都保留。定位与验证证据为基线 `20261120_080409_332646_k4wfxgkt_playthrough/09.host.log:3`、`:5`、`09.events.log:1645`。

主线程在仓库根目录执行。先核对补丁，再用应用补丁后生成的同一 EXE 串行复跑。批跑器首行输出每轮的日志目录。

```powershell
$analysis = Join-Path $PWD 'scripts/analysis'
$patch = 'archive/2026-11/20261120_09_title_hit_and_receipt.patch'
git apply --check -- $patch

# $PatchedStoryPlayer 指向主线程构建的补丁后 EXE
$sha = (Get-FileHash -Algorithm SHA256 -LiteralPath $PatchedStoryPlayer).Hash
$batchArgs = @('--player', $PatchedStoryPlayer, '--indices', '9', '--jobs', '1', '--ram-per-job-gib', '2', '--reserve-ram-gib', '4')
$positive = @(python -B "$analysis/run_playthrough_batch.py" @batchArgs --scenarios "$analysis/pc_story_09_title_to_r43_guarded.json")
python -B "$analysis/check_pc_body_successor.py" 09 $positive[0] --player-sha256 $sha

$negative = @(python -B "$analysis/run_playthrough_batch.py" @batchArgs --scenarios "$analysis/pc_story_09_title_to_r43_receiver_negative.json")
python -B "$analysis/check_pc_body_successor.py" 09 $negative[0] --player-sha256 $sha --expect-receiver-mismatch
```

- 正例退出 0，输出 `successor_verified`。标题唯一消费后到 `sc01 g0/r41`，`60/8` 消费一次 Enter，根以 `[2,0,0,0]` 恢复。释放扫描结束后，停在同 CST load 的 `g1/r43`，绝对 r42 已自然完成并提交。
- 反例退出 0，输出 `receiver_mismatch_verified`。到 r41 后，配置要求 `60/6`，实测 `60/8`，在 `KeyDown` 前结束。18:54 已有反例因可用 RAM 比 6 GiB 门槛低约 12.8 MiB，检查器输出 `resource_deferred`、退出 1。

## 20 下一次单按

历史日志支持从 `A_01_01 g0/r13` 一次 Enter 到 `g1/r15`。f3440 的活 `flow/GAME_WAIT`、`meswnd01/MES_SETSTATE` 和根 `0xad0` 同时在等键，正文 2988 个非零 alpha 像素，提示 259 个。f3441 的 `101/8` 唯一消费后虽然已标记 r15，正文只有 615 个像素。f3484 活窗再次执行 `MES_WAITINPUT`，f3520 正文有 8955 个像素且提示可见。f3680 提示处于灭相，正文仍为 8955。后续在同一构建绑定 r13 和 r15 各自的 CST 来源、自然完成与合成代次，再验收这次单按。18:25 的 Rebellions 状态分支只识别标题和选章，正文状态路由仍待接线。r15 后的 `g2/r26` 目前只有静态记录。

| 编号 | 已存证据 | 可验收的下一站 |
| --- | --- | --- |
| 09 | `080409_332646_k4wfxgkt` 的 `09.host.log:178,194,196`，f900 正文 7876 个非零 alpha 像素，提示 567，透明度 255。`09.events.log:1995,2123,2124,2978` 对应 `sc01` 载入、g0 绝对 r40 读取与 CP932 入队、r41 等待。窗体 12 的 epoch 4 在 f662 自然完成并提交。外层 PE 是 BoxLoader，已核验实际运行 PE 为 `c264b246…`。 | 从 `sc01 g0/41` 一次 Enter，验 `g1` 绝对 r42 原始 CP932 正文的自然提交，以及 `g1/43` 提示。历史 Enter 只见传播与根恢复，尚无此站回执。 |
| 20 | `061929_267759_w16y1hex` 的 `20.host.log:142–144` 记录 f3440 一次 Enter，f3441 唯一消费回执为 `Keyboard(13), key=2, group=0, raw_mask=1, region=101, receiver=8, response=0`。`20.events.log:19054,19063–19064,19082` 有 GBK 正文入队、CST 消息和等待、根 `0xad0` 恢复包 `[2,0,0,0]`。`20.host.log:310,325,328` 见 r15 与正文 alpha 8955、提示 259。`080615_566666_648kadme` 对照到 f3520 保持 `g0/13` 和 alpha 2988。 | 历史回放从 `A_01_01 g0/13` 到 `g1/15`。精确状态路由验收需将 `g0/r12`、`g1/r14` 的正文来源绑定到各自提交代次。f3441 的 r15 标记已出现，画面仍在逐字呈现。 |
| 12 | `072454_248150_awf1fxqe` 的 `12.host.log:40–41,81–82` 记录标题 `flow/LOOP_TITLE` 与 `title/WAIT_USER`，无 CST 或影片。`[640,443]` 命中 region 15，f720 仍在标题。 | 保持 hover-only。`state=unknown` 且 `trace_enabled=false`，活动 `BTN_START` 与默认标题页条件尚未同轮核验。 |

## 09 和 20 的运行时接口

专用模块 `simulator/crates/desktop/src/story_trace_09.rs` 已保存以下接口。

- `host_kanmi_exact_title_owner(&self) -> Option<u32>` 用实际 PE 与启动脚本识别 09，核对当前 FES 胜出资源和活程序，要求 `flow/TITLE_WAIT` 与唯一子窗 `title/WAIT`，输入队列为空，无影片、CST、待处理回调或暂停。
- `host_kanmi_exact_new_game_target(&mut self) -> Option<(u32, u32)>` 返回 `(region, owner)`。核对标题共享槽 `$541=0`、真实 `btn1[0]` 对象绑定、`[140,354]` 的启用命中及候选顺序 `[3,14,23,7]`。标题消费要求 `Mouse(1)`、`key=0/group=0/response=0`、region 14。`080409` 日志中 f381 的 region 3 先传播，region 14 随后唯一消费，f448 产生新 `sscript` 子任务。标题 ID 与正文接收者 ID 均可为 8，分别核验其生命周期。
- `host_kanmi_exact_body(&self) -> Option<(u32,u32,u32,u32,u64,usize,u64,u64)>` 依次返回正文 owner、键盘 region、接收者、CST context、load ID、绝对正文记录、source epoch、source frame。专用 `observe(self, frame)` 还返回 `scene/group/record/window`。它要求唯一 `flow/GAME_WAIT` 和 `meswnd/WAIT`，根 native 71 `0xcae`，原始 CP932 记录、自然完成且已提交的当前代次、实际提交的正文与提示、活动 `sscript` 子任务和 `bottom button`。

共享 `story_trace.rs` 已声明 `story_trace_09`，并从专用 `host_replay_kanmi_position` 提供标题和两个确切页。`story_route` 的 `kanmi_exact` 分支已调用这些接口。通用诊断仍会显示 `active`，精确路由页用回放标准输出的 `replay frame=… state=page` 核验。

共享控制器按 `kanmi-title-r43/v1` 验固定三步，校对正反场景的资源摘要，禁止页级回退。标题物理回执与 `GO_STORY/exit 0 → flow START_GAME → sscript child` 同轮关联。Enter 前先独立解析现场 receiver 和 region，再和配置比对。正例期望 `60/8`，反例配置 receiver 6 仍须通过解析，并在 r41 且 `KeyDown` 前报 `checkpoint_mismatch`。正文只允许一次 `Keyboard(13)`，受验消费为 `key=2/group=0/raw_mask=1/response=0`，同一帧根 `0xcae` 有且只有一次 `[2,0,0,0]`。需等设备扫描确认释放，保持相同 owner/context/load，r42 的 epoch 增长，source frame 在派发之后。见 r43 即结束。`20261119_191133_436732_playthrough/09.host.log:17` 的旧 Enter 包含 `60/8 response=1`，`09.events.log:5216` 有根恢复，无 r43。

20 可复用物理消费和释放检查，运行时从活动 `sscript` 下的 `bottom button` 解析接收者。`061929` 的 `20.events.log:10762` 记录了 101/8 的创建，而正文 owner 是 30。`MES_WAITINPUT` 在 `0x4de73` 的 native 630 操作目标为 `meswnd01`。等待壳 `MES_SETSTATE` 与提示闪烁都可能在逐字期间出现。`story_trace` 的 Rebellions 分支还需补 `confirm2` 与精确正文状态，再使用 `pc_story_rebellions_state_once_pending.json`，并按最终共享接口对齐该文件的键盘字段。二次等待占位场景暂缓。

## 11／13 精确来源核查

- **11**。`050932_697698_izegy8_a` 的 `11.events.log:51696,221879,221884,221892,221899` 把上下文 139 的 `00_001` 载入、g0 的 1037 调用 r78／79／80、`[input]` 关联起来。源成员 `scene.int::00_001.cst` 的摘要是 `9e4efc626b072ff3de336665e3f23bb993e9da4f79f9a59e5bee5ba183b8fe78`。r78 是说话者，r79／80 是 CP932 正文，1037 给出 UTF-8。`080615_566666_648kadme/11.host.log:196,213–214,276–277` 显示正文 alpha 17343，`cinema/MES_SHOW`，`radio_wave.mpg` 仍在循环，根等待 `0xd6d`。该回放的 CST 位置为空。`g0/81` 由记录与等待证据对应，下一候选为 `g1/86` 的 r84／85。当前代码的 `August` 来源探针只接受 g0/r78–80，两个端点的实时提交与下一页物理回执仍待同轮。
- **13**。`074717_962322_u6tvk97k` 的 `13.events.log:7561,11307–11310` 给出 `BLAOP1_END` 载入与上下文 174、g4 序号 10／11 的相邻 327→276。两个读都用 `0x4ec27`，分别发布到共享槽 1001/1000。绝对记录为 r101 说话者（GBK 4 字节）、r102 正文（GBK 97 字节），下一等待为 r103。成员摘要为 `c09bcb90152977cd9d21b68ec019d627ccc80136036503a5ce376a2ae6020561`。同批 `13.host.log:58,72` 显示 alpha 25036、`MES_SHOW`、无影片，`trace_enabled=false`。当前 `Eden` 探针已按成员与 PE 限定 r101／102，已有回放缺少来源到 FES 提交代次的完整回执。成对按顺序关联 327 和 276，避免将复用的临时地址当作记录身份。

## 现有播放器的观察配置

- 09 使用 `pc_story_kanmi_enter_binding_observe.json`（一次已证标题点击，正文零输入）。`pc_story_09_title_to_r43_guarded.json` 和 `pc_story_09_title_to_r43_receiver_negative.json` 留给新一轮共享接线产物，分别合批。
- 20 使用 `pc_story_rebellions_body_enter_control_no_key_0538.json`（三个正常入口动作，正文零输入）。较长观察也可用 `pc_story_rebellions_body_wait_observe.json`。`pc_story_rebellions_body_enter_once_0538.json` 仅用于追溯历史单按证据。
- 12 使用 `pc_story_12_title_observe.json`。只移动指针，按键、点击、读档和截图均为零。

在主线程已选定的 `$StoryPlayer` 上以 `--jobs 1` 逐项排队。纯日志核验可直接运行。

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
$logs = 'F:/CS2-build-cache/playthrough/2026-11'
python -B "$analysis/check_pc_body_successor.py" 09 "$logs/20261120_080409_332646_k4wfxgkt_playthrough"
python -B "$analysis/check_pc_body_successor.py" 20 "$logs/20261120_061929_267759_w16y1hex_playthrough" --receipt-receiver 8
python -B "$analysis/check_pc_body_successor.py" 20 "$logs/20261120_080615_566666_648kadme_playthrough"

python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 9 --scenarios "$analysis/pc_story_kanmi_enter_binding_observe.json" --jobs 1
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --scenarios "$analysis/pc_story_rebellions_body_enter_control_no_key_0538.json" --jobs 1
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 12 --scenarios "$analysis/pc_story_12_title_observe.json" --jobs 1

```

以上三份历史日志依次输出 `observe_only`、`successor_observed`、`observe_only`，均退出 1。09 补丁后的正反例和判定见本文首节，构建与回放由主线程排队。

## 19:08 候选的 09 正文失败与 20 单按接线

09 的 `09.host.log:6` 证实 f315 的标题点击已消费并恢复根 VM。`:12` 显示 f914 位于 `sc01 g0/r41`，消息窗为 `WAIT`，`WAIT_KEY=true`，正文代次 4 的来源帧为 f385，自然完成与提交为 f596。`09.events.log:1406`、`:1478`、`:2160`、`:2165` 是同一 CST 句柄 155、同一输出地址、g0/r40 的四次 native 327 读取。f552 的读取覆盖了此前缓存的回执，随后 `exact_source` 要求读取帧不晚于 f385，因而拒绝这代正文。f382 已有 r42 预读，反例检查器以错接收者判定帧为界，允许该预读并拒绝之后的新读取。 [09 宿主日志](<F:/CS2-build-cache/playthrough/2026-11/20261120_190801_610883_287t7_2w_playthrough/09/09.host.log:12>) · [r40 重读](<F:/CS2-build-cache/playthrough/2026-11/20261120_190801_610883_287t7_2w_playthrough/09/09.events.log:2160>)

09 补丁在每次 CST load 中保留至多 32 条去重后的精确读取回执，两条正文记录共同使用该容量。守卫仍核对来源记录、正文代次及活接收者。若合批后另有守卫拒绝，可加归档中的 `20261120_09_body_gate_diagnostics.patch`，失败时的 `kanmi_body_gate` 字段会报告首个失败项。 [09 来源修复](<D:/迅雷下载/CS2/archive/2026-11/20261120_09_body_source_read_history.patch>) · [09 分段诊断](<D:/迅雷下载/CS2/archive/2026-11/20261120_09_body_gate_diagnostics.patch>)

20 补丁从标题 New Game、A 卡、`confirm2` 的 No 依次进入 `A_01_01`，每步检查当场等待、按钮绑定、命中顺序、唯一物理消费和同帧 VM 恢复。正文检查活 `flow/GAME_WAIT`、`meswnd01/MES_SETSTATE`、`WAIT_KEY` 与 `MES_WAITINPUT`，核对胜出 GBK CST 的 r12／r14 来自当前 load。正文及提示提交后才放行 r13 的一次 Enter，消费端从活 `bottom button` 与其 `sscript` 父任务解析，根恢复包为 `[2,0,0,0]`。松键扫描通过后，r15 须在相同消息窗、接收者、区和 CST load 上自然完成并提交新代。r15 提示若在灭相，可等待下一次显相。静态解包证实原 `sscript.kcs` 有 327 直接调用，未发现 1037 或 1038 直接调用。首轮仍以实际 `cst-source-record` 日志核对 ABI。 [20 离线补丁](<D:/迅雷下载/CS2/archive/2026-11/20261120_20_rebellions_r13_r15_route.patch>)

主线程按顺序合入两份补丁，20 复用 09 的回执保留逻辑。以下命令以工作区为当前目录，`$StoryPlayer` 指向合批后冻结的 `story_replay.exe`。

```powershell
$p09 = 'D:/迅雷下载/CS2/archive/2026-11/20261120_09_body_source_read_history.patch'
$p20 = 'D:/迅雷下载/CS2/archive/2026-11/20261120_20_rebellions_r13_r15_route.patch'
git apply --check $p09
if ($LASTEXITCODE -ne 0) { throw '09 patch check failed' }
git apply $p09
if ($LASTEXITCODE -ne 0) { throw '09 patch apply failed' }
git apply --check $p20
if ($LASTEXITCODE -ne 0) { throw '20 patch check failed' }
git apply $p20
```

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 9 --scenarios "$analysis/pc_story_09_title_to_r43_guarded.json" --jobs 1
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 9 --scenarios "$analysis/pc_story_09_title_to_r43_receiver_negative.json" --jobs 1
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --scenarios "$analysis/pc_story_rebellions_state_r13_observe.json" --jobs 1
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --scenarios "$analysis/pc_story_rebellions_state_r15.json" --jobs 1

python -B "$analysis/check_pc_body_successor.py" 09 $Run09Positive
python -B "$analysis/check_pc_body_successor.py" 09 $Run09Negative --expect-receiver-mismatch
python -B "$analysis/check_pc_body_successor.py" 20 $Run20Control --expect-r13-observe
python -B "$analysis/check_pc_body_successor.py" 20 $Run20Positive
```

四份新回执依次要求 `successor_verified`、`receiver_mismatch_verified`、`r13_observe_verified`、`successor_verified`，均退出 0。`$Run*` 由批跑输出目录赋值。20 历史单按与零正文键依次仍为 `successor_observed`、`observe_only`。离线已验证两份补丁顺序应用、Rust 语法解析、配置 JSON 和日志判断。编译与实跑留在主线程。
