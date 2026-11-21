# 09、12、20 PC 剧情接续

## 20 从 r61 到换窗、翻页和恢复普通窗

00:24 批独立验收到 `A_01_01 12/61`（f3291）。`r189` 补丁的等键表已在主检出。下表共用从新游戏出发的入口，每档在目标等键零输入结束。表中 Enter 数只计正文，入口另有三次游戏菜单点击。 [r61 实跑批](F:/CS2-build-cache/playthrough/2026-11/20261121_002446_022100_phfaym7b_playthrough)

| 配置 | 停点 | 正文 Enter | 合入依赖 |
| --- | --- | ---: | --- |
| [r189](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_20_r189.json) | 普通窗 `26/189` | 26 | 当前树中的 r189 路线 |
| [novel_guard_r211](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_20_novel_guard_r211.json) | novel 首个活等键 `27/211` | 27 | r189 + novel/恢复增量 |
| [novel_guard_r215](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_20_novel_guard_r215.json) | 首个翻页等键 `29/215` | 29 | 同上 |
| [restored_r301](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_20_restored_r301.json) | 四处翻页后恢复普通窗 `47/301` | 47 | 同上 |
| [restored_r610](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_20_restored_r610.json) | 首场最后等键 `122/610` | 122 | 同上 |

新增量以当前 r189 为基线，并覆盖旧 r211 观察补丁的用途。合批选新增长线补丁，旧 r211 观察补丁保留归档。第二场 GBK 补丁的覆盖从 `A_01_02` 开始，这五档均在 `A_01_01`。新增量以已钉定的 CST 及 native 327 的来源链开启 novel 追踪，累计 NVSTRING 保留原样，每次只给本轮 `MES_SETSTRING` 记一代。 [长线 Rust 增量](F:/CS2-build-cache/archive/2026-11/20261121_story_20_rebellions_novel_and_restore.patch)

| 边界 | 等待及验收 |
| --- | --- |
| `12/61 → 13/150 → 26/189` | `WIPE31 90` 后继续沿用普通窗活键、当前 GBK 来源和原接收者。 |
| `26/189 → 27/211` | 经 `WIPE50 120`、`rdraw 120`、`novel on`、`view 0 novel01`、`frameon fade 30`。等真实 novel FES、r210 新代自然显完并合成、`keywait2` 和模式 1。 |
| `27/211 → 46/282` | `29/215、32/225、37/238、41/249` 是 `0x03`。必须见 `[page]`、native 630 的 `MES_WAITPAGE`、模式 2 及已提交的 `keywait`。其余页等 `[input]`、`MES_WAITINPUT`、模式 1 与 `keywait2`。普通等键与翻页均要求物理 Enter 唯一消费及扫描释放。 |
| `46/282 → 47/301 → 122/610` | 经 `novel off`、`view 0 wnd01`、`rdraw 120` 后，重新确认活 `meswnd01/MES_SETSTATE`、普通提示层和新正文代次。两次换窗均维持 CST context/load 和原 bottom button、sscript 接收者，按新 FES 重新验 owner。 |

从工作区根执行。调度参数沿用组内单槽。首次合入可用 `novel_guard_r211`，最长用 `restored_r610`。检查器以批次路径为输入并核对构建身份。

```powershell
$archive = 'F:/CS2-build-cache/archive/2026-11'
$patch = "$archive/20261121_story_20_rebellions_novel_and_restore.patch"
git apply --check --whitespace=error $patch
$mode = 'novel_guard_r211'
python -B scripts/analysis/run_playthrough_batch.py --player $StoryPlayer --indices 20 --scenarios "$archive/20261121_pc_story_20_$mode.json" --jobs 1 --timeout 3000
python -B scripts/analysis/check_pc_story_20_novel.py --run $Run20 --mode $mode
```

## 09 首次制作名单路线

当前实跑锚点为 `sc01 27/134`。已有 `sc02_r547` 候选。下一处 `sc02 101/562` 需要按序核对 r555、r560、r561 的来源与合成，随后继续到 `264/1379`。尾部 r1388/r1389 按 `#759` 选择两条片头路径。记录实际分支，等影片自然结束并确认新场景 `sc03 0/7`。这是可安排的跨场存档候选。 [sc02 配置](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_09_sc02_r547.json)

从 `sc03 → sc04 → sc05 → sc06 → sc07 → sc07s → sc08 → sc09`。在 `sc09 125/764` 验收正文后，单次 Enter 打开 r765 菜单。观察原编号 1、r766、目的地 `sc10`，再沿 `sc12 → sc13 → sc14 → sc15 → sc16`。第二个可写档候选是 `sc16 10/32`。消费正文后等 r34 菜单的原编号 1、r35、目的地 `scra01`。每个菜单先独立验证可见按钮、实际命中、活接收者及唯一消费，再按原编号点选。

`scra01 → … → scra09 124/653` 后走 `sced01`。首次 `#761 != 1` 进入 `sced01_a`，记录 r24 的制作名单图层 `vd_staff_roll`、原脚本计时和 r82 的结束标志层合成。接着 `sced01_c` 写 `#760=1、#761=1`。首次 `#763 != 1` 走 `sced01_d`，在 r0 写 `#763=1`，核对 r2 的 `vfa_6a` 合成、60 帧淡入、40 帧淡出以及 r17 自然返回标题。制作名单段和结束段都没有正文输入等待。 [控制记录和各场来源表](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_09_12_credits_audit.json)

## 12 首次制作名单路线

从已验 `op001 20/222` 接当前 `r619 → OPMOVIE_END 0/42` 候选。后续可先用单来源守卫推进，最早异形等键 `10/90` 绑定 r86，随后是 `57/479` 绑定 r468。经 `417/3759 → op002 0/61 → op002_a → OP003`。建议优先验证 `op002 0/61` 的普通档写入与冷读。 [片头后首等键配置](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_12_post_op_r42.json)

从 `air_com001…013` 经 `yuk_ev001a…d → yuk_c004 → mel_com001…008`，继而 `yuk_ev002a…d → ria_com001…007 → yuk_ev003a → yuk_ev003b → yuk_ev003b_end`。控制表列出夹在编号之间的子场景及其非相邻、多来源或 `0x03` 等键。首个明显的小说页门槛是 `air_com007a_c 0/61`，须独立核对当轮来源组与翻页提示。

首次通关在 `yuk_ev003b_end` r16 读取 `#525=0` 后进 `yuk_ev003c`，r37 设 `#170=0x0001`，沿 Yuk 子场景走到 `yuk_003_e 87/883`。此处是分支前可写档候选。按键后要求 `#900=0` 和真实 `yuk_003_select`，选原编号 0（r1，去 `yuk_ED1`）。如果实际 `#525=1`，记录回到 `select_root_cha` 的菜单，停止首次通关路线。原编号 0、1、2、3 分别对应 Yuk、Air、Mel、Ria，并各有实际解锁条件待现场验收。 [分支及末段控制表](F:/CS2-build-cache/archive/2026-11/20261121_pc_story_09_12_credits_audit.json)

`yuk_ED1 96/706` 后写 `#711` 位 0、3 及 `#525=1`，再进 `YUK_EDMOVIE1`。首次确认 `#849=0` 且 `#612` 位 1 未设置，由 r3 播 `ed1 0 100`。依影片 handle 核对创建、播放、自然结束、释放和制作名单画面，核对 r5 设位以及 `EDMOVIE_ENDYUK1` r0 返回标题。第一次从该路走通后，在新进程验证通关旗标和标题可见变化。结束画面资源和实际首末帧留待回执确认。

## 普通档及验收

批准清单当前为空，09/12 现有回执可作新游戏锚点。候选正文页须先验证无活动影片、派发队列已清、正文和原接收者均已稳定。通过游戏自己的保存界面写一个隔离普通槽，记录游戏 PE、资源树、槽号、大小和摘要。新进程仅暂存经审核的单个普通槽，经游戏标题加载，核对恢复场景、group/record、新提交正文及活接收者，再消费一次并命中不同的真实后继。分支前档记录入档时控制值，制作名单后另做冷启动的通关状态验收。 [普通档准入清单](</D:/迅雷下载/CS2/scripts/analysis/pc_ordinary_saves.json>)

09、12 的制作名单路线已离线核对控制记录、菜单编号、存档候选和媒体出口。现阶段可运行的先导仍由既有小段候选提供。其余场景需逐段实跑确认来源、选择按钮和媒体结束守卫。 [控制记录审计脚本](</D:/迅雷下载/CS2/scripts/analysis/audit_pc_story_09_12_credits.py>)
