# 20 首次翻页与 12 标题入口

## 20 · A_01_01

已存单按回放为 `20261120_061929_267759_w16y1hex_playthrough`，同入口的后续零正文输入对照为 `20261120_080615_566666_648kadme_playthrough`。下表帧号用于定位日志。

| CST 等待点 | 文本来源与入队 | 画面及等待 |
| --- | --- | --- |
| `g0/13` | `scene.int::A_01_01.cst` 的 `g0/r12`。现有定向测试从这个胜出成员用 native 327 读取 ordinal 12，校验 GBK，随后将同一返回缓存发布到共享字符串 1000。历史实际回放 f2427 记录 `meswnd01` 任务 30 的 native 630 调用及 GBK 来源入队 18 字节。 | f2488 出现 `[input]`，f2509 调用 `MES_WAITINPUT`。f3440 的 list 10 处于 enabled/submit，任务 30 的 `str[0]` 有 2988 个非零 alpha 像素，`keywait` 有 259 个。对照回放到 f3520 仍为 `g0/13`。 |
| `g1/15` | 下一正文目标是同一成员的绝对 `r14`，读取参数用 `g1` 范围内的 ordinal。历史 f3441 有任务 30 的同型调用、GBK 来源入队 36 字节和 `[message]`／`[input]`。该帧的正文只有 615 个非零 alpha 像素。 | f3484 再调 `MES_WAITINPUT`，f3520 同一 list 10 的正文有 8955 个、提示有 259 个非零 alpha 像素，保持 `g1/15`。f3680 提示为零，f3760 重新可见，正文像素稳定。 |

源身份使用实际胜出资源。`REBELLIONS.exe` SHA-256 为 `a099ac1cee7af60c8f32869485c9df4aab344b5eb91c1018ae8bb7df2c1f39f3`，入口为 `kcs.int/main.kcs`。`scene.int::A_01_01.cst` 的 GBK 覆盖摘要是 `92de81ed6532c41611106715b4d8185cd5210ff0b9a177586e1d4d9f1b839176`。`meswnd01.fes` 原版字节摘要为 `4ff3b5efdad654cb1d581f1cde55cd46fdfa4a6918471377e69779d1f5535022`。当前 Rust 会在 Story Trace 的成功 CST load 上分配 load ID，并将来源沿 native 276 发布和 FES 模板传到 `str[0]`。历史日志只打印了 GBK 入队与画面层，两个端点缺少可串联的 load ID、来源记录与提交代次。源码已有帮助函数可复用，Rebellions 的精确场景分支及来源探针仍待接线。 [GBK 源身份](<D:/迅雷下载/CS2/simulator/crates/desktop/src/cst_context_native.rs:287>) · [定向来源测试](<D:/迅雷下载/CS2/simulator/crates/desktop/src/cst_context_native.rs:1148>) · [正文来源与完成](<D:/迅雷下载/CS2/simulator/crates/desktop/src/story_trace.rs:675>)

### 一次正常 Enter 的放行和验收

1. 完成正常标题、新游戏、A 卡和男主语音确认入口后，只在 `flow/GAME_WAIT` 与唯一 `meswnd01/MES_SETSTATE` 或已证实的 `WAIT` 同时稳定、根 native 71 等待 `0xad0` 时检查正文。要求焦点有效，输入队列空，无暂停、影片、待处理 FES handler 或 modal。
2. 在匹配的 CST load 上确认 `g0/13` 是 kind `0x02`，当前 `str[0]` 的唯一正文来源是同一 context/load 的 `g0/r12` kind `0x20`。跟踪实际读取与共享字符串发布，用当前 FES 语句消费的字节对应。自然完成与提交都要绑定同一 source epoch，无剩余、绘制、待处理文本或强制跳过。正文和提示要在同一有效提交列表上可见且在输出裁剪范围内。
3. 重新从活 `sscript` 找唯一 `bottom button`，核对父子关系、当前注册及键盘扫描映射 `Keyboard(13) → logical 2/group 0`。历史任务 101 和正文任务 30 同属接收者 8。按一次常规 Enter，要求下一采样帧恰一枚物理匹配、非重复、`raw_mask=1/response=0` 的消费，以及同帧唯一根 `0xad0` 恢复 `[2,0,0,0]`。扫描确认释放。
4. 到 `g1/15` 时核对同 owner、context 和 load ID，正文来源为 `g1/r14`，其读取和源代次都晚于派发，epoch 增长。等自然完成、同代提交、提示可见并稳定停下。源身份、接收者或后继任一未知就停在现场。

后续接线给现有离线验证器提供 `probe: Rebellions` 的 `cst-source-record` 和两个端点的 `body_generation`、`body_page`。来源探针限定上述 PE、实际胜出成员、`g0/r12`、`g1/r14` 与选中的读取 ABI。扩充后的观察场景同时追踪 native 327 和 1037，用首段实际调用核对 ABI。严格验收目前按 327 的原样 GBK 返回及发布准备。旧日志可复核历史单按，输出 `successor_observed`。 [第一次文本入队](<F:/CS2-build-cache/playthrough/2026-11/20261120_061929_267759_w16y1hex_playthrough/20.events.log:10818>) · [下一段入队](<F:/CS2-build-cache/playthrough/2026-11/20261120_061929_267759_w16y1hex_playthrough/20.events.log:19054>) · [物理回执](<F:/CS2-build-cache/playthrough/2026-11/20261120_061929_267759_w16y1hex_playthrough/20.host.log:144>) · [下一段合成](<F:/CS2-build-cache/playthrough/2026-11/20261120_061929_267759_w16y1hex_playthrough/20.host.log:310>)

## 12 · Hatsumira 标题入口

- **PE 与入口。** 对本地 `Hatsumira.exe` 直接计算的 SHA-256 是 `c48d383c335c5b283bb0c17bd9f14b2b8bfef4a2e3d635365157b1c69498df78`，与 f234 和 f720 的运行诊断一致。入口日志加载 `kcs.int/main.kcs`，封装 6390 字节，code 22610、data 3101、stack 4096。静态报告的 `fes.int::flow.fes` 和 `fes.int::title.fes` 资源摘要已写入观察配置，进入标题前核对当场胜出者和运行 FES。 [入口](<F:/CS2-build-cache/playthrough/2026-11/20261120_072454_248150_awf1fxqe_playthrough/12.events.log:14>) · [PE 与标题状态](<F:/CS2-build-cache/playthrough/2026-11/20261120_072454_248150_awf1fxqe_playthrough/12.host.log:40>)
- **标题接收者。** 任务树中 flow 为 6，title 为其 kind-34 子任务 8，region 15 对应 title 下的 kind-6 `btn_def`。历史 `[640,443]` 的掩膜命中 15，候选顺序 `[3,15,37,7]`。单次左键的逻辑键 0 和 2 均向 15/8 传播，所有响应均为 1。根 `0xc29` 同帧恢复 `[2,640,443,0]`，随后同编号 8 出现新的普通 `sscript` 生命周期，f256 载入 `start`，f260 载入 `op001`。 [标题子对象](<F:/CS2-build-cache/playthrough/2026-11/20261120_072454_248150_awf1fxqe_playthrough/12.events.log:59>) · [历史路由](<F:/CS2-build-cache/playthrough/2026-11/20261120_053051_059695_jwz8j23y_playthrough/12.host.log:5>) · [根恢复](<F:/CS2-build-cache/playthrough/2026-11/20261120_053051_059695_jwz8j23y_playthrough/12.events.log:783>) · [op001 载入](<F:/CS2-build-cache/playthrough/2026-11/20261120_053051_059695_jwz8j23y_playthrough/12.events.log:1244>)
- **下一次标题动作的门槛。** 从同一轮读取 `flow/LOOP_TITLE`、`title/WAIT_USER`、local 0 默认页、真实 `btn_def` bank 到 `BTN_START` 的活绑定，以及掩膜和候选。还需核验标题处理归还到 flow 并创建新 `sscript` 的路径。历史成功样本为传播加根恢复，使用该协议取证。观察配置保持纯移动，直到标题分支和接收者映射齐备。

## 验收命令

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
$runs = 'F:/CS2-build-cache/playthrough/2026-11'
python -B "$analysis/check_pc_body_successor.py" 20 "$runs/20261120_061929_267759_w16y1hex_playthrough" --receipt-receiver 8
python -B "$analysis/check_pc_body_successor.py" 20 "$runs/20261120_080615_566666_648kadme_playthrough"

# 主线程用同一选定的播放器合批
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --scenarios "$analysis/pc_story_rebellions_body_wait_observe.json" --jobs 1
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 12 --scenarios "$analysis/pc_story_12_title_observe.json" --jobs 1
```

## 21:10 批次接线与短复验

21:05 候选的两条专用路线只接受 `observe_r69`／`enter_once` 和 `observe_r13`／`enter_once`。21:10 两项在解析 `opening_segment` 时返回 `scenario_error`，`last_frame=null`。控制器已有两条路线接线。主检出随后合入了下表中的两份首错修补与两份连续段增量。两条专用路线现在均声明 `OpeningSegment`，各自的绝对等键表和 CST 来源记录已接入。 [12 解析错误](<F:/CS2-build-cache/playthrough/2026-11/20261120_211052_419283_uibxfbv7_playthrough/12/12.host.log:4>) · [20 解析错误](<F:/CS2-build-cache/playthrough/2026-11/20261120_211052_419283_uibxfbv7_playthrough/20/20.host.log:4>) · [21:05 构建身份](<F:/CS2-build-cache/playthrough/2026-11/20261120_211052_419283_uibxfbv7_playthrough/build_identity.json>)

离线检查 EXE 字节，21:20 版也缺 `opening_segment`。21:25 版含 `opening_segment`、`hatsumira-body-setstring` 和标题 `btn_1i[0]` 检查，EXE SHA-256 为 `fad833f66d0769b07e1457715649d58fe9dcf10bce0e49ddbdd21b4a0b16a5b8`。该次构建前后唯一变动为 `perf_emote_rows_bench.rs`，构建回执标为 `diagnostic-mixed-source`。12／20 尚待新候选的短实跑。 [21:25 构建记录](<D:/迅雷下载/CS2/archive/2026-11/20261120_212513_390720_story_replay_integration/build.json:3119>)

| 编号 | 首轮修复 | 20:55 实跑后的修复 | 连续段增量 |
| --- | --- | --- | --- |
| 12 | native 15 缓存拼接保留 `\cd` 后的正文来源 | [`MES_SETSTRING` 独立 `\cd` 控制](<D:/迅雷下载/CS2/archive/2026-11/20261120_story_12_body_mes_setstring_color_reset.patch>) | [`op001 g14/r179`](<D:/迅雷下载/CS2/archive/2026-11/20261120_story_12_opening_r179.patch>) |
| 20 | 活接收者与 FES 银行绑定 | [标题 `btn1[0]` 命中与 `btn_1i[0]` 显示](<D:/迅雷下载/CS2/archive/2026-11/20261120_story_20_title_hit_layer.patch>) | [`A_01_01 g10/r48`](<D:/迅雷下载/CS2/archive/2026-11/20261120_story_20_opening_r48.patch>) |

- **12 短验收。** 20:55 的 f129 缓存回执为 task 8、context 126、load 2、g0/r68，46 字节正文加 3 字节控制。f545 验收首等键，f546 唯一 Enter 由 region 109 → task 8 消费并使根 VM 恢复。后继同帧能在 139 字节缓存找到 g1/r71，136 字节也已送入 FES，最终 `g1/r72` 仍缺当前代正文来源。原版 `meswnd.fes` 的 `MES_SETSTRING` 执行 `str apend $str1000`。新增量将该入口的独立三字节 `\cd` 记录成空来源片段，并打印 `hatsumira-body-setstring` 的前缀、共享串和追加来源，供核对 136 字节追加能否继承 g1/r71。通过条件为同一 load 的 r71 自然完成并提交至 `g1/r72`，一次正文 Enter，终点零下一键。 [r68 缓存回执](<F:/CS2-build-cache/playthrough/2026-11/20261120_205533_491752_7jzmubr0_playthrough/12/12.events.log:1443>) · [r71 入队](<F:/CS2-build-cache/playthrough/2026-11/20261120_205533_491752_7jzmubr0_playthrough/12/12.events.log:4616>) · [终点](<F:/CS2-build-cache/playthrough/2026-11/20261120_205533_491752_7jzmubr0_playthrough/12/12.host.log:16>)
- **20 短验收。** 20:55 的 f2033 记录 `live=true enabled=true receiver=Some(8)`、`btn1` bank 0 → region 16，显示检查失败。标题资源让 `BUTTON btn1[0]` 接受命中，由独立 `IMAGE btn_1i[0]` 显示并淡入。新增量分别校验 BUTTON 绑定和 IMAGE 显示。标题通过需要 `[3,16,28,7]`、region 3 传播、16 → 8 唯一消费并同帧恢复 `[2,480,667,0]`。观察清单继续核对 A 卡及确认按钮，停在 `A_01_01 g0/r13` 的真实等键，正文 Enter 数为 0。 [20:55 分项](<F:/CS2-build-cache/playthrough/2026-11/20261120_205533_491752_7jzmubr0_playthrough/20/20.host.log:2>) · [历史标题消费](<F:/CS2-build-cache/playthrough/2026-11/20261120_061929_267759_w16y1hex_playthrough/20.host.log:3>)
- **10 资源缺口。** 实际目录仍只有 1,490,940 字节的 `scene.int.FWP`，SHA-256 `cbe602d9f74025153d000731d05c2da7786257572542a3f8a99817cb46153b40`。`install.dfn` 第 53 行声明输出 `scene.int`，常规 KIF 解出的 `start.cst` 开头为 `c18e3f8e995fbba0`，有效 CST 头应为 `CatScene`。等同版安装产物及有效的 `start.cst` 可读后再安排 10。 [FWP 审计](<D:/迅雷下载/CS2/archive/2026-11/20261119_073547_566427_pc_story_08_10_14_sidecar/20261119_10_fwp_identity.txt>) · [安装声明](<D:/迅雷下载/CS2/games/戦国天使ジブリール/install.dfn:53>)

短复验命令由主线程在合批后按槽执行。`$Run12` 和 `$Run20` 分别指向对应批次目录。

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 12 --jobs 1 --scenarios "$analysis/pc_story_12_op001_r72.json"
python -B "$analysis/check_pc_story_12_op001.py" $Run12 --mode enter --require-cache-reset
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --jobs 1 --scenarios "$analysis/pc_story_rebellions_state_r13_observe.json"
python -B "$analysis/check_pc_body_successor.py" 20 $Run20 --expect-r13-observe
```

## 21:32 实跑和 22:05 合入核对

22:05 同源构建的播放器摘要为 `527b5c49bb54ffd5a45c59708b95be08df89c354da4840964365f823437e4762`。涉及 09、12、20 的九个 Rust 文件与构建回执逐个哈希一致。09 的 r47 增量与 20 的标题、连续段、confirm2 增量均在当前源码中。12 的 `12_opening_r179` 提供连续段，`12_wipe04b_r92` 在其基础上增加短停点及 native 530 修复。隔离副本已核对这个应用顺序。 [22:05 构建回执](D:/迅雷下载/CS2/archive/2026-11/20261120_220537_815819_story_replay_integration/build.json) · [09 r47 增量](F:/CS2-build-cache/archive/2026-11/20261120_story_09_sc01_r47_continuation.patch) · [12 wipe04b 增量](D:/迅雷下载/CS2/archive/2026-11/20261120_story_12_wipe04b_r92.patch) · [20 confirm2 增量](D:/迅雷下载/CS2/archive/2026-11/20261120_story_20_confirm2_mes_setstate.patch)

| 样本 | 已确认的起点 | 接续及终点 |
| --- | --- | --- |
| 09 | 20:55 的 f599 实际将 Enter 交给 region 60 → task 8，f653 到 `sc01 1/43`。21:32 的播放器只识别旧 r43 清单，在新游戏前拒绝 r47。 | 22:05 用 `kanmi-title-r47/v1` 从新游戏走 `0/41 → 1/43 → 2/45 → 3/47`。后两页核对同一 CST 的正文 r44、r46。三次正文 Enter 后零下一键。 |
| 12 | 21:32 的 f615 已验 `op001 1/72` 的 r71，同一窗、同一 load。f616 的第二枚 Enter 由 region 109 → task 8 消费并恢复根 `0xc29`。f617 进入第一个 `rwipe wipe04b 15`。f1715 仍在等特效，r92 当时只在 CST 候选字段里。 | 短验收停 `2/92`。两次 wipe 及脚本等待自然结束后，核对 r91 的 120 字节 CP932、当前代提交和同一接收者。长路线接 `3/100 → 4/109 → 5/148 → 6/151 → 7/154 → 8/157 → 9/161 → 10/164 → 11/167 → 12/170 → 13/174 → 14/179`。r96、105 有 `rdraw 60`，r106 有 `wait 90`，r112–144 有脚本驱动的波纹动画及等待。 |
| 20 | 21:32 已核对标题和 A 卡两次点击。f5195 的 `0000` 仍有 `flow/GAME_WAIT`、`confirm2/WAIT` 和空闲 `meswnd01/MES_SETSTATE`。修补允许这一实际组合。历史 f2414 的 `[723,418]` 由 region 15 → confirm owner 12 独占消费。历史首翻已经到 `A_01_01 1/15`。 | 短验收仅翻到 `1/15`。长路线接 `2/26 → 3/30 → 4/32 → 5/34 → 6/38 → 7/40 → 8/42 → 9/44 → 10/48`。r15→r26 有退窗 15、`rwipe WIPE01A 120`、进窗 10 和等待。每个端点核对同一 GBK CST load、新正文、真实提示和活 Enter 接收者。 |

12 的 r69–179 与 20 的 r13–48 已逐行核对实际胜出成员。每个端点都是 kind `0x02`，前一条是该页唯一非空正文。检查器按每页核对原字节长度。12 在短、长两种模式都检查 wipe04b 的 530 配置、531 启用、520 轮询及第二次擦除。20 可用同一个检查器核对 r13、r15 或完整连续段。 [21:32 的 09 解析日志](F:/CS2-build-cache/playthrough/2026-11/20261120_213230_279191_n8ay5wi2_playthrough/09/09.host.log:1) · [12 旧擦除停点](F:/CS2-build-cache/playthrough/2026-11/20261120_213230_279191_n8ay5wi2_playthrough/12/12.events.log:5395) · [20 输入对照](F:/CS2-build-cache/playthrough/2026-11/20261120_061929_267759_w16y1hex_playthrough/20.host.log:17)

`$Run09`、`$Run12`、`$Run20` 为对应命令输出的批次目录。三项都保持终点零下一键、零截图。12 短验收为 `r92_verified`，20 短验收为 `successor_verified`。09 为 `continuation_verified`，12 和 20 长段为 `opening_segment_verified`。

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
$StoryPlayer = 'D:/迅雷下载/CS2/archive/2026-11/20261120_220537_815819_story_replay_integration/story_replay.exe'
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 09 --scenarios "$analysis/pc_story_09_title_to_r47_guarded.json" --jobs 1
python -B "$analysis/check_pc_story_09_continuation.py" $Run09
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 12 --scenarios "$analysis/pc_story_12_op001_r92.json" --jobs 1
python -B "$analysis/check_pc_story_12_op001.py" $Run12 --mode r92 --require-cache-reset
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --scenarios "$analysis/pc_story_rebellions_state_r15.json" --jobs 1
python -B "$analysis/check_pc_story_20_opening_segment.py" $Run20 --mode r15
```

连续段将 12 或 20 的 `--scenarios` 换成 `pc_story_12_20_opening_segments.json`，检查器用 `--mode opening`。22:08 已排入的 20 使用了此前的诊断列表。新列表补充了擦除 native 日志，20 页验收仍使用实际接收者及正文回执。

## 22:08 批次后续交接

09 的 `3/47` 已实跑至 f825，正文 r46，owner 12、context 155、load 1、epoch 16，入队在 f781，最早来源读取在 f599。三次 Enter 由 region 60 → task 8 独占消费，同帧恢复根 `0xcae`。后续 `4/49 → 5/51 → 6/53` 已核对实际 `sc01.cst`，正文分别为 r48 的 140 字节、r50 的 46 字节和 r52 的 100 字节，均为严格可解码的 CP932，三个等待均为 `0x02`。这三段中只有正文和等待。r54 起先后停粒子、淡出消息窗并等待，故短路线在 `6/53` 停键。新契约沿用相同的标题、读源、正文合成和 Enter 接收者检查。 [09 实跑回执](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/09.json) · [09 r53 清单](D:/迅雷下载/CS2/scripts/analysis/pc_story_09_title_to_r53_guarded.json) · [09 增量](F:/CS2-build-cache/archive/2026-11/20261120_story_09_sc01_r53_continuation.patch)

12 的 `2/92` 在 f681 确认 r91 当前代正文，同一 owner 110、context 126、load 2、epoch 12。f682 由 region 109 → task 8 消费并恢复根 `0xc29`。f683 创建 kind 38、id 13 的 `rdraw`，f685 起反复 native 520，f1781 根仍在等 `rdraw`，r100 只出现在 CST 候选字段。指纹为 `1932f5e6f308b5e4676c6b2437d98e191bfaebcb9a8f50f64d78bffb1feccab5` 的 `kcs.int::sscript.kcs` 在 `0x7463a` 将脚本时长减 2，在 `0x746d4`、`0x74701` 写入默认 mode 0、深度 45000，在 `0xb1da5`、`0xb1e44`、`0xb1eaa` 分别配置 527、启用 528 和查询 520。原 PE 的 527 包装器 `0x5066a0` 将 `[task, 58, 0, 0x472fc800]` 转为 Draw 接收记录 `[0, 0x472fc800, 58]`。旧回放用第 3 个记录 DWORD 作时长，把深度位模式 `0x472fc800` 当成了 1194313728 帧。精确 PE 修补沿用原输入与等待顺序。r100 短清单额外记录 527 和 528，验收 r99 的 30 字节 CP932 当前代正文和零下一键。 [rdraw 日志](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/12/12.events.log:6172) · [12 停点](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/12/12.host.log:30) · [12 r100 清单](D:/迅雷下载/CS2/scripts/analysis/pc_story_12_op001_r100.json) · [12 增量](F:/CS2-build-cache/archive/2026-11/20261120_story_12_rdraw527_r100.patch)

20 在 f2494 同帧收到 region 101 → task 8 的唯一 Enter、恢复根 `0xad0` 并向 owner 30 入队 r14 的 36 字节 GBK 正文。同一 CST load 2 在 f2402 已预读 r14，后续同源读取沿用该最早回执。f2545 的 `1/15` 已有 epoch 10，自然完成于 f2537，来源和合成均匹配。旧路线把 `read_frame <= started` 判为新鲜度失败。修补以 `source_frame` 晚于发键帧为准，继续要求同一活任务、CST load、递增 epoch 与严格正文合成。短路线仅走 `13 → 15`，长路线再经过含退窗、擦除和进窗的 `15 → 26` 后继续至 r48。 [r14 预读](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/20/20.events.log:8396) · [r14 入队](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/20/20.events.log:9311) · [20 停点](F:/CS2-build-cache/playthrough/2026-11/20261120_220818_455427_u4tzujax_playthrough/20/20.host.log:94) · [20 增量](F:/CS2-build-cache/archive/2026-11/20261120_story_20_prefetched_body_successor.patch)

三份增量均已在主检出通过 `git apply --reverse --check`。隔离副本复核了 09 和 12 的应用及 Rust 语法。09 旧 r47 批次通过新版检查器，删掉终点来源时被拒。12 用真实日志的 r92 前缀复核旧等待及擦除，r100 检查器另用离线配参和轮询样例核对。20 原批次仍判为未完成，旧日志中的同帧消费、入队和自然合成已复核。

22:24 主线程构建的六个相关 Rust 文件与三份增量合入后的主检出逐项摘要一致，EXE SHA-256 为 `1f529b9b1ec2c17b4d76ed3fb977bc8659d28bbfbd7b81f41158330958cbe8c7`。22:29 的 09、12、20 均因可用内存约 0.77 GiB 低于批次所需 4 GiB 报 `resource_deferred`。三只检查器都用退出码 2 报延期。各 `$Run` 变量指向主线程分槽输出的批次目录。 [22:24 构建回执](D:/迅雷下载/CS2/archive/2026-11/20261120_222435_833279_story_replay_integration/build.json) · [22:29 批次](F:/CS2-build-cache/playthrough/2026-11/20261120_222905_280211_bg7d968y_playthrough/09.json)

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
$StoryPlayer = 'D:/迅雷下载/CS2/archive/2026-11/20261120_222435_833279_story_replay_integration/story_replay.exe'
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 09 --scenarios "$analysis/pc_story_09_title_to_r53_guarded.json" --jobs 1
python -B "$analysis/check_pc_story_09_continuation.py" $Run09 --mode r53
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 12 --scenarios "$analysis/pc_story_12_op001_r100.json" --jobs 1
python -B "$analysis/check_pc_story_12_op001.py" $Run12 --mode r100 --require-cache-reset
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --scenarios "$analysis/pc_story_rebellions_state_r15.json" --jobs 1
python -B "$analysis/check_pc_story_20_opening_segment.py" $Run20 --mode r15
```

短路线的目标回执依次为 `continuation_verified`、`r100_verified`、`successor_verified`。12 和 20 可在短路线通过后用 `pc_story_12_20_opening_segments.json` 分槽，分别用 `--mode opening` 核对至 r179 和 r48。

## 22:31 实跑与下一段

| 样本 | 实跑结果 | 下一段与判据 |
| --- | --- | --- |
| 09 | `6/53` 于 f1144 通过。owner 12、context 155、load 1、epoch 28，r52 100 字节，来源读取 f826，当前代正文 f1034。全段六次正文 Enter，region 60 → task 8，同帧恢复 `0xcae`。 | 从 `6/53` 经 r54–76 的淡出、两次 `wipe02c 100`、合计 810 的显式脚本等待及音频转场，至 `7/78` 的 r77（157 字节），再到 `8/80` 的 r79（92 字节）。两页各有一条 CP932 正文。每次发键前沿用同一接收者、同一 CST load 与当前代自然完成和合成判据。`8/80` 收尾，下一页 r81 已出现 `0x21` 人物记录。转场留 1600 帧等待预算，发键仍由活等键触发。 |
| 12 | 两次 `rdraw 60` 的实际 527 参数均为 `[13,58,0,0x472fc800]`。第一段 f684 启用、f743 停用、f744 发布等键，f762 通过 `3/100`。第二段 f765 启用、f824 停用、f854 发布等键，f856 通过 `4/109`。最终 f1213 完成 `14/179`，owner 110、context 126、load 2、epoch 72，十四次 Enter、零截图。 | 旧检查器用 `source_frame` 作为 Draw 查找上界，实跑 r99 已在 f682 入队。修正为以 `marked` 查完整 527→528→520→528 停用链，并逐次验两段。新短线从 r179 自然执行 r188 `rwipe wipe05 40` 和后续镜头、语音，停 `15/195`，正文 r194 为 92 字节。下一段 `16/206` 前还有 `rwmos 60 10 0`，留在短线终点之后。 |
| 20 | f2545 通过 `1/15`，证实预读修补。f2546 唯一 Enter 由 region 101 → task 8 消费，r25 的 14 字节已入队。f2565 native 530 实参 `[12,118,0,0x472fc800,339]`，f2566 启用，到 f5545 同任务仍轮询 520，停止于 `g2/r26 did not settle`。 | 原版 native 表 `0x314240` 的 530 包装器 `0x49d2a0` 将传参时长移到接收记录第 3 个 DWORD。独立补丁复用 PE 与 `sscript.kcs` 验证，只在 Rebellions 这一路径把 Wipe 格式改为 `[mode,depth,118,mask,0]`。短线只停 `2/26`，须看到同一擦除任务完成、r25 同代自然显完并合成及零下一键。通过后沿现有 `opening_segment` 至 `10/48`。 |

09 与 12 的真实完整回执分别通过 `--mode r53` 和 `--mode opening --require-cache-reset`。移除第二段 Draw 停用回执时，12 检查器会拒绝。20 的真实 r15 前缀独立核对通过，旧 Wipe 日志通过参数核对并因缺少停用回执被拒。三份增量已在隔离副本依序应用，Rust 语法检查通过。 [09 终点](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/09/09.events.log:4292) · [12 终点](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/12/12.host.log:107) · [20 配参](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/20/20.events.log:9993) · [20 停点](F:/CS2-build-cache/playthrough/2026-11/20261120_223113_422529_85q5bgjh_playthrough/20/20.host.log:161)

三个 Rust 增量供主线程合入，`$StoryPlayer` 指向包含对应增量的构建产物。 [09 r80](F:/CS2-build-cache/archive/2026-11/20261120_story_09_sc01_r80_continuation.patch) · [12 r195](F:/CS2-build-cache/archive/2026-11/20261120_story_12_op001_r195_continuation.patch) · [20 r26](F:/CS2-build-cache/archive/2026-11/20261120_story_20_wipe530_r26.patch)

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 09 --scenarios "$analysis/pc_story_09_title_to_r80_guarded.json" --jobs 1
python -B "$analysis/check_pc_story_09_continuation.py" $Run09 --mode r80
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 12 --scenarios "$analysis/pc_story_12_op001_r195.json" --jobs 1
python -B "$analysis/check_pc_story_12_op001.py" $Run12 --mode r195 --require-cache-reset
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --scenarios "$analysis/pc_story_rebellions_state_r26.json" --jobs 1
python -B "$analysis/check_pc_story_20_opening_segment.py" $Run20 --mode r26
```

## 23:04 至 23:11 回执与接续

22:54 构建的源码前后摘要均为 `dcf7049844ba7fef14686b475faf9b125cf62ba23082b6d977addd5df2f366df`，实际 EXE 摘要为 `e3753fa87e3d3825e25ec32579097640d88028a917f1a080d31f9b3b812d67bb`。本节三份回执使用该候选。 [构建回执](D:/迅雷下载/CS2/archive/2026-11/20261120_225418_816218_story_replay_integration/build.json)

### 09 · 第一处运行分歧

23:11 批 `sc01 6/53` 在 f1144 成立，Enter 在 f1145 由 region 60 → task 8 唯一消费，根 VM 同帧恢复。r77 在 f977 已预读。f1179 进入 r61 第一条 `rwipe wipe02c 100`，kind 39、parent 8、task 28，native 530 参数为 `[28,98,0,0x472fc800,260]`。f1180 启用后持续查询同一 task 到 f2744，没有停用或第二道擦除。首处运行分歧是 530 参数的时长位置。宿主未识别该版的时长前置布局，kind 39 从接收记录第 3 个 DWORD 读取 `0x472fc800` 作为时长。 [按键回执](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/09/09.host.log:55) · [首条擦除](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/09/09.events.log:4639) · [末帧轮询](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/09/09.events.log:10921)

原始 `kcs.int::sscript.kcs` SHA-256 `17de548390b8c2b15a4f413418bbf1b408d6dee6ab830879f09cf305f5c128fd`，指令区 SHA-256 `7a068090a8d37c7a446a74e98448458e3d6ed75d94bb1a00022eaeb07d7a3921`，在 `0xb478e` 调用 530。独立增量复用启动时已匹配的 Kanmi PE、main 和 sscript 身份，并再次核对调用 VM 指令区和 kind 39，将该包转换为 `[0,0x472fc800,98,260,0]`。先跑已合入的 r80 路线以验两道 530/531/520 回执，再接已合入的 r102 路线。后段为 `8/80 → 9/83 → 10/85 → 11/87 → 12/89 → 13/91 → 14/93 → 15/96 → 16/98 → 17/100 → 18/102`。r81、r94 为人物记录，清单保留两者原字节摘要。任一页须由同次 CST load 的非空正文自然完成并合成，原接收者唯一消费，到终点停键。 [530 增量](F:/CS2-build-cache/archive/2026-11/20261120_story_09_wipe530_bus.patch) · [r102 清单](D:/迅雷下载/CS2/scripts/analysis/pc_story_09_title_to_r102_guarded.json) · [检查器](D:/迅雷下载/CS2/scripts/analysis/check_pc_story_09_continuation.py)

### 12 · 从 rwmos 到片头前后

23:04 短线通过 `r195_verified`。f1265 的 `15/195` 为 owner 110、region 109 → task 8、context 126、load 2、epoch 77，r194 于 f1214 进入当前代，f1264 自然完成。新候选有 r206 单页和 r222 五页两档。原始 r196–202 依次退窗、等待、清场、`rwmos 60 10 0` 和等待，r203 语音、r204 人物、r205 正文。接续 `16/206 → 17/210 → 18/214 → 19/218 → 20/222`，正文 r205、209、213、217、221 的 CP932 字节数依次为 22、120、18、72、30。 [r195 回执](F:/CS2-build-cache/playthrough/2026-11/20261120_230431_238274_o9xwg_kj_playthrough/12/12.host.log:114) · [r206 清单](D:/迅雷下载/CS2/scripts/analysis/pc_story_12_op001_r206.json) · [r222 清单](D:/迅雷下载/CS2/scripts/analysis/pc_story_12_op001_r222.json) · [路线增量](F:/CS2-build-cache/archive/2026-11/20261120_story_12_op001_rwmos_r222_continuation.patch)

`sscript.kcs` 的 `0xb0232` 为登记 `rwmos` kind 50 的 native 66。`0xb240e`、`0xb24fd`、`0xb2592` 依次为 853 配置及第一帧、854 第二帧、855 播放，`0xb25f3` 查询 520，结束分支 `0xb0fdc` 用 855 停用。`--mode r206` 和 `--mode r222` 预期在 r206 发布等键前看到同一任务的这条链，并按 853 包里的时长核对 855 播放至停用的帧间隔。帧预算只限定观察时长。正文推进仍由同代提交、合成、真等键及 Enter 回执控制。 [r206 检查器](D:/迅雷下载/CS2/scripts/analysis/check_pc_story_12_op001.py:205)

| 下一边界 | 从原始脚本取得的顺序 | 运行验收 |
| --- | --- | --- |
| r222 → 片头前 | r226、r230、r237…直到 `61/619`。r619 前为 r618、144 字节。`30/279`、`35/341`、`37/380`、`39/406`、`43/448`、`56/591` 的正文分别是 r276、333、359、395、437、587，紧邻各等键的是空文本记录。 | 首个可按相邻正文扩展的观察终点为 `29/261`。到 `30/279` 需用 r276 实际来源并核对 r278 的空记录处理。继续到 r619 前逐页审计来源、同代合成以及原绑定。 |
| 末页 → OPMOVIE | r620–631 退窗、等待、`bgm 0 end 60`、清场、`rdraw 60`、等待、`next OPMOVIE`。片头 CST 共 8 条命令。`#849==0` 时按 `#612&1` 选择 `movie hatsuop 0 100` 或 `movie hatsuop 3 100`，随后更新 `#612` 并 `next OPMOVIE_END`。 | 只从已确认的 `61/619` 发一枚 Enter。跟踪 `trace-native:265` 至 `269`，记录同一 `movie.int/hatsuop.mpg` handle 的实际开始、结束和释放，确认自然结束。片头期间零输入。 |
| 自然结束 → 片后首等键 | `OPMOVIE_END` r0–12 退窗、眼幕及重开，r34 `rwipe wipe97 40` 并等待，r40 人物、r41 正文 126 字节，首等待 `0/42`。后两页为 `1/46`、`2/51`。 | 首跑观察停 `0/42`。核验新 CST load、r41 自然完成并合成、当下 FES 消息窗和活 Enter 绑定。收齐后才从 `0/42` 试翻一次，核对 r45 的 78 字节和新的消费回执。 |

静态审计脚本读取三份有摘要约束的 CST，逐组输出 r195 后的 46 个等键、实际正文摘要、人物、空记录和命令，再输出片后前三页。影片目录项为 `movie.int::hatsuop.mpg`，108539908 字节。片头及片后边界目前是原始资源审计，运行守卫覆盖至 r222。 [审计脚本](D:/迅雷下载/CS2/scripts/analysis/audit_pc_story_12_op001_movie.py)

### 20 · 复用连续路线

23:11 的 `r26_verified` 证实 task 12 在 f2565 配置 118 帧，f2566 启用，f2685 停用。f2724 的 `2/26` 为 owner 30、region 101 → task 8、context 185、load 2、epoch 14，r25 于 f2716 自然完成，终点无下一键。现有 `opening_segment` 从标题复验前缀，然后推 `2/26 → 3/30 → 4/32 → 5/34 → 6/38 → 7/40 → 8/42 → 9/44 → 10/48`，逐页验同 load、新 epoch、正文合成及唯一 Enter。r48 后的 `frameoff fade 15`、`rwipe WIPE01 120`、`frameon fade 10` 是下一道边界，静态后继为 `11/58 → 12/61`。 [r26 回执](F:/CS2-build-cache/playthrough/2026-11/20261120_231101_690425_x8sj7k9f_playthrough/20/20.events.log:11336) · [现有清单](D:/迅雷下载/CS2/scripts/analysis/pc_story_12_20_opening_segments.json:24)

主检出已通过 09 r102、09 530 和 12 r222 三份增量的 `git apply --reverse --check`。23:18 构建记录中的三份 12 相关 Rust 文件摘要均与主检出一致。该构建的 `render_tasks.rs` 仍为 22:54 版，09 须使用包含新 530 增量的后续 EXE。 [23:18 构建回执](D:/迅雷下载/CS2/archive/2026-11/20261120_231856_384472_story_replay_integration/build.json)

各 `$Run` 指向实际批次目录，`$StoryPlayer` 指向含目标增量的主线程构建。12 的 `$Game12` 可从已完成批次 `12.json` 读取。

```powershell
$analysis = 'D:/迅雷下载/CS2/scripts/analysis'
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 09 --jobs 1 --scenarios "$analysis/pc_story_09_title_to_r80_guarded.json"
python -B "$analysis/check_pc_story_09_continuation.py" $Run09 --mode r80
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 09 --jobs 1 --scenarios "$analysis/pc_story_09_title_to_r102_guarded.json"
python -B "$analysis/check_pc_story_09_continuation.py" $Run09Long --mode r102
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 12 --jobs 1 --scenarios "$analysis/pc_story_12_op001_r206.json"
python -B "$analysis/check_pc_story_12_op001.py" $Run12 --mode r206 --require-cache-reset
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 12 --jobs 1 --scenarios "$analysis/pc_story_12_op001_r222.json"
python -B "$analysis/check_pc_story_12_op001.py" $Run12Long --mode r222 --require-cache-reset
python -B "$analysis/audit_pc_story_12_op001_movie.py" $Game12
python -B "$analysis/run_playthrough_batch.py" --player $StoryPlayer --indices 20 --jobs 1 --scenarios "$analysis/pc_story_12_20_opening_segments.json"
python -B "$analysis/check_pc_story_20_opening_segment.py" $Run20 --mode opening
```
